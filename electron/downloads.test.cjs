'use strict';
const test = require('node:test'), assert = require('node:assert/strict');
const { EventEmitter } = require('node:events');
const { Downloads, MAX_BYTES, blobAllowed, postAllowed, nameSuggestion, mimeAllowed, outsideProfile } = require('./downloads.cjs');
const origin = 'http://127.0.0.1:32123';
test('download URLs exactly match owned blob and original POST artifact routes', () => {
  assert.ok(blobAllowed(origin, 'blob:' + origin + '/12345678-abcd-abcd-abcd-123456789abc'));
  for (const suffix of ['/api/v1/artifacts/1/download', '/api/v1/drc/review/notes/artifacts/2/download', '/api/v1/drc/review/waives/artifacts/3/download']) assert.ok(postAllowed(origin, origin + suffix));
  for (const url of [origin + '/api/v1/artifacts/01/download', origin + '/api/v1/artifacts/18446744073709551616/download',
    origin + '/api/v1/artifacts/1/download?x=1', origin + '@evil/api/v1/artifacts/1/download', 'file:///tmp/x',
    'blob:http://127.0.0.1:32124/12345678-abcd-abcd-abcd-123456789abc']) {
    assert.equal(postAllowed(origin, url), false); assert.equal(blobAllowed(origin, url), false);
  }
  assert.equal(blobAllowed(null, 'blob:null/12345678-abcd-abcd-abcd-123456789abc'), false);
});
test('suggestions are not paths and HTML responses are never downloads', () => {
  assert.equal(nameSuggestion('../../secret'), 'floe-export');
  assert.equal(nameSuggestion('한글/파일\n.png'), '한글파일.png');
  assert.equal(nameSuggestion('a\u0085b.png'), 'ab.png');
  assert.equal(nameSuggestion('a'.repeat(300)).length, 180);
  assert.equal(mimeAllowed('text/html'), false);
  assert.equal(mimeAllowed('application/octet-stream'), true);
});
test('a selected output cannot be saved inside an automatically cleaned profile, including aliases', () => {
  const fs = require('node:fs'), path = require('node:path'), os = require('node:os');
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-export-profile-test-'));
  try {
    const profile = path.join(root, 'profile'); fs.mkdirSync(profile);
    fs.mkdirSync(path.join(profile, 'child'));
    const alias = path.join(root, 'alias'); fs.symlinkSync(profile, alias);
    for (const parent of [profile, path.join(profile, 'child'), alias]) assert.equal(outsideProfile(profile, path.join(parent, 'saved')), false);
    assert.equal(outsideProfile(profile, path.join(root, 'saved')), true);
    assert.equal(outsideProfile(profile, 'relative'), false);
    assert.equal(outsideProfile(profile, path.join(root, 'missing', 'saved')), false);
  } finally { fs.rmSync(root, { recursive: true }); }
});

// Deterministic controller tests. Real Rust filesystem + Chromium cases live
// in download-slot.test.cjs and the explicit smoke modes, not these fakes.
const tick = () => new Promise(resolve => setImmediate(resolve));
function harness(choose = async () => null, notify = async () => {}) {
  const slots = [], owner = {};
  function createSlot() {
    const s = new EventEmitter();
    Object.assign(s, { phase: 'ready', path: '/synthetic/private/payload', child: { pid: 1 }, publishes: 0 });
    s.ready = Promise.resolve(s);
    s.finished = new Promise(resolve => { s.resolve = result => { s.phase = 'ended'; resolve(result); }; });
    s.watch = () => { s.phase = 'receiving'; };
    s.close = () => { if (s.phase !== 'ended') s.resolve({ publication: 'not_requested', cleanup: true }); };
    s.retain = () => { s.retained = true; s.resolve({ publication: 'not_requested', cleanup: false }); };
    s.publish = destination => {
      s.publishes++; s.destination = destination;
      s.resolve({ publication: 'saved', cleanup: true }); return s.finished;
    };
    slots.push(s); return s;
  }
  const manager = new Downloads({ origin: () => origin, owns: c => c === owner, choose, notify, createSlot });
  function receive(overrides = {}, contents = owner) {
    const item = new EventEmitter(), event = { rejected: false, preventDefault() { this.rejected = true; } };
    Object.assign(item, { bytes: 4, cancelled: 0, getURL: () => 'blob:' + origin + '/12345678-abcd-abcd-abcd-123456789abc',
      getURLChain: () => [item.getURL()], getInitiatorOrigin: () => origin, getMimeType: () => 'application/json',
      getTotalBytes: () => item.bytes, getReceivedBytes: () => item.bytes, getFilename: () => 'synthetic.json',
      setSavePath: value => { item.savePath = value; },
      cancel: () => { item.cancelled++; item.emit('done', {}, 'cancelled'); }, ...overrides });
    manager.receive(event, item, contents); return { item, event };
  }
  return { manager, slots, receive };
}
test('native cancellation and chooser exceptions discard, replenish, and do not report cleanup failure', async () => {
  for (const choose of [async () => null, async () => { throw new Error('closed panel'); }]) {
    const h = harness(choose, () => { throw new Error('closed UI'); });
    const { item, event } = h.receive(); assert.equal(event.rejected, false);
    item.emit('done', {}, 'completed'); await tick();
    assert.equal(h.slots[0].publishes, 0);
    assert.deepEqual(h.manager.results, [{ publication: 'not_requested', cleanup: true }]);
    assert.equal(h.manager.active, null); assert.equal(h.slots.length, 2);
    assert.equal(await h.manager.shutdown(), true);
  }
});
test('duplicate, foreign, redirect, HTML, oversize, or opaque-origin transfers never get a destination', async () => {
  const h = harness();
  for (const overrides of [{ getInitiatorOrigin: () => null }, { getURLChain: () => ['redirect', 'target'] },
    { getMimeType: () => 'text/html' }, { bytes: MAX_BYTES + 1 }]) {
    const { item, event } = h.receive(overrides);
    assert.ok(event.rejected); assert.equal(item.savePath, undefined);
  }
  assert.ok(h.receive({}, {}).event.rejected);
  const first = h.receive(); assert.ok(h.receive().event.rejected);
  first.item.emit('done', {}, 'cancelled'); await tick();
  assert.equal(await h.manager.shutdown(), true);
});
test('size watchdog and interrupted response cancel without asking or resuming', async () => {
  for (const failure of ['limit', 'interrupted', 'bytes']) {
    let choices = 0;
    const h = harness(async () => { choices++; return '/synthetic/new'; });
    const { item } = h.receive();
    if (failure === 'limit') h.slots[0].emit('limit');
    else { if (failure === 'bytes') item.bytes = MAX_BYTES + 1; item.emit('updated', {}, failure); }
    await tick();
    assert.equal(item.cancelled, 1); assert.equal(choices, 0); assert.equal(h.slots[0].publishes, 0);
    assert.equal(await h.manager.shutdown(), true);
  }
});
test('shutdown invalidates a pending destination decision; no late publication or replenishment', async () => {
  let choose;
  const h = harness(() => new Promise(resolve => { choose = resolve; }));
  h.receive().item.emit('done', {}, 'completed'); await tick();
  assert.equal(await h.manager.shutdown(), true);
  choose('/synthetic/late'); await tick();
  assert.equal(h.slots[0].publishes, 0); assert.equal(h.slots.length, 1);
  assert.equal(h.manager.active, null);
});
test('shutdown cancels the Chromium producer and waits for done before Rust EOF cleanup', async () => {
  let cancelled = false;
  const h = harness();
  const { item } = h.receive({ cancel: () => { cancelled = true; } });
  const s = h.slots[0], close = s.close;
  let cleaned = false;
  s.close = () => { cleaned = true; close(); };
  const stopping = h.manager.shutdown(); await tick();
  assert.equal(cancelled, true); assert.equal(cleaned, false);
  item.emit('done', {}, 'cancelled');
  assert.equal(await stopping, true); await tick();
  assert.equal(cleaned, true); assert.equal(h.slots.length, 1);
});
test('unconfirmed producer termination retains staging instead of hanging or unlinking', async () => {
  const h = harness(); h.manager.stopWaitMs = 2;
  const { item } = h.receive({ cancel: () => { throw new Error('Chromium unavailable'); } });
  assert.equal(await h.manager.shutdown(), false);
  assert.equal(h.slots[0].retained, true);
  item.emit('done', {}, 'cancelled'); await tick();
  assert.equal(h.manager.results[0].cleanup, false);
});
test('diagnostic history stays bounded; failed cleanup remains visible through shutdown', async () => {
  const h = harness(async () => '/synthetic/new');
  for (let i = 0; i < 40; i++) { h.receive().item.emit('done', {}, 'completed'); await tick(); }
  assert.equal(h.manager.results.length, 32);
  const s = h.manager.slot;
  s.close = () => s.resolve({ publication: 'not_requested', cleanup: false });
  assert.equal(await h.manager.shutdown(), false);
});
