'use strict';
const test = require('node:test'), assert = require('node:assert/strict');
const { ClipboardController, activationProbe } = require('./clipboard-controller.cjs');
const origin = 'http://127.0.0.1:32123', owner = {};
const details = { isMainFrame: true, requestingUrl: origin + '/' };
function setup(probe = async () => true) {
  const state = { active: true, origin, probes: 0 };
  const c = new ClipboardController({ origin: () => state.origin, owns: web => web === owner,
    allowed: () => state.active, probe: web => { state.probes++; return probe(web); }, timeout: 10 });
  const request = (web = owner, permission = 'clipboard-sanitized-write', d = details) =>
    new Promise(resolve => c.request(web, permission, resolve, d));
  return { c, state, request };
}
test('only the active owner root can request a transient sanitized write', async () => {
  const { request, state } = setup();
  assert.equal(await request(), true);
  assert.equal(state.probes, 1);
  for (const permission of ['clipboard-read', 'clipboard-write', 'media', 'notifications', '', null]) {
    assert.equal(await request(owner, permission), false);
  }
  for (const d of [undefined, {}, { ...details, isMainFrame: false },
    { ...details, requestingUrl: origin + '/#bootstrap=synthetic' },
    { ...details, requestingUrl: origin + '/other' }, { ...details, requestingUrl: 'http://127.0.0.1:32124/' }]) {
    // Passing undefined to request uses its default; call the controller directly.
    assert.equal(await new Promise(r => setup().c.request(owner, 'clipboard-sanitized-write', r, d)), false);
  }
  assert.equal(await request(null), false); assert.equal(await request({}), false);
  state.active = false; assert.equal(await request(), false);
  state.active = true; state.origin = 'https://external.invalid'; assert.equal(await request(), false);
  assert.equal(state.probes, 1);
});
test('activation must be exactly true; unavailable/throwing probes deny', async () => {
  for (const value of [false, null, undefined, 1, 'true']) assert.equal(await setup(async () => value).request(), false);
  assert.equal(await setup(() => { throw new Error('synthetic'); }).request(), false);
  assert.equal(await setup(async () => { throw new Error('synthetic'); }).request(), false);
  assert.match(activationProbe, /navigator\.userActivation\.isActive/);
  assert.match(activationProbe, /!document\.hidden && document\.hasFocus/);
  assert.doesNotMatch(activationProbe, /clipboard|postMessage|fetch/);
});
test('timeout denies once; a stalled probe keeps its credit until it settles', async () => {
  const resolutions = [], calls = [];
  const { c } = setup(() => new Promise(r => resolutions.push(r)));
  c.request(owner, 'clipboard-sanitized-write', v => calls.push(v), details);
  await new Promise(r => setTimeout(r, 25));
  assert.deepEqual(calls, [false]);
  assert.equal(await new Promise(r => c.request(owner, 'clipboard-sanitized-write', r, details)), false);
  assert.equal(resolutions.length, 1);
  resolutions[0](true); await new Promise(r => setImmediate(r));
  assert.equal(c.pending, null); assert.deepEqual(calls, [false]);
  const second = new Promise(r => c.request(owner, 'clipboard-sanitized-write', r, details));
  await new Promise(r => setImmediate(r));
  resolutions[1](true); assert.equal(await second, true);
});
test('a busy event loop cannot grant after the deadline before the timer runs', async () => {
  const { c, request } = setup(async () => { c.now = () => 11; return true; });
  c.now = () => 0;
  assert.equal(await request(), false);
});
test('overlapping requests deny; invalidation/end fence the pending callback', async () => {
  for (const end of [false, true]) {
    let resolve;
    const { c, request, state } = setup(() => new Promise(r => { resolve = r; }));
    const pending = request(); await new Promise(r => setImmediate(r));
    assert.equal(await request(), false); assert.equal(state.probes, 1);
    if (end) c.end(); else c.invalidate();
    assert.equal(await pending, false); resolve(true);
    await new Promise(r => setImmediate(r)); assert.equal(c.pending, null);
    if (end) assert.equal(await request(), false);
  }
});
test('focus, origin and owner are rechecked after the asynchronous probe', async () => {
  for (const change of [s => { s.active = false; }, s => { s.origin = 'http://127.0.0.1:32124'; }]) {
    let resolve;
    const { request, state } = setup(() => new Promise(r => { resolve = r; }));
    const result = request(); await new Promise(r => setImmediate(r)); change(state); resolve(true);
    assert.equal(await result, false);
  }
  const { c, request } = setup(async () => { c.owns = () => false; return true; });
  assert.equal(await request(), false);
});
