'use strict';
const test = require('node:test'), assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), os = require('node:os');
const { DownloadSlot } = require('./download-slot.cjs');
const { once } = require('node:events');
const binary = process.env.FLOE_ELECTRON_DOWNLOAD_BIN;
if (!binary || !path.isAbsolute(binary)) throw new Error('Set absolute FLOE_ELECTRON_DOWNLOAD_BIN');
function deadline(promise) {
  let timer; return Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('Download QA timed out')), 10000); })]).finally(() => clearTimeout(timer));
}
test('real Rust slot publishes a copy as 0600, without replay or staging leftovers', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-download-qa-'));
  const slot = new DownloadSlot(binary, root);
  try {
    await deadline(slot.ready); slot.watch();
    fs.writeFileSync(slot.path, 'synthetic 한글');
    const output = path.join(root, 'saved.json');
    assert.deepEqual(await deadline(slot.publish(output)), { publication: 'saved', cleanup: true });
    assert.equal(fs.readFileSync(output, 'utf8'), 'synthetic 한글');
    assert.equal(fs.statSync(output).mode & 0o777, 0o600);
    assert.deepEqual(fs.readdirSync(root), ['saved.json']);
  } finally { slot.close(); await deadline(slot.finished); fs.rmSync(root, { recursive: true }); }
});
test('racing existing destination and linked staging payload stay untouched', async () => {
  for (const link of [false, true]) {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-download-qa-'));
    const slot = new DownloadSlot(binary, root);
    try {
      await deadline(slot.ready); slot.watch();
      const output = path.join(root, 'existing'); fs.writeFileSync(output, 'preserve', { mode: 0o640 });
      if (link) fs.symlinkSync(output, slot.path); else fs.writeFileSync(slot.path, 'new');
      const dest = link ? path.join(root, 'new') : output;
      assert.deepEqual(await deadline(slot.publish(dest)), { publication: 'unconfirmed', cleanup: true });
      assert.equal(fs.readFileSync(output, 'utf8'), 'preserve');
      assert.equal(fs.statSync(output).mode & 0o777, 0o640);
      assert.deepEqual(fs.readdirSync(root), ['existing']);
    } finally { slot.close(); await deadline(slot.finished); fs.rmSync(root, { recursive: true }); }
  }
});
test('pipelined watch plus EOF cancels without buffered-control deadlock', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-download-qa-'));
  const slot = new DownloadSlot(binary, root);
  try {
    await deadline(slot.ready); fs.writeFileSync(slot.path, 'partial');
    slot.watch(); slot.close();
    assert.deepEqual(await deadline(slot.finished), { publication: 'not_requested', cleanup: true });
    assert.deepEqual(fs.readdirSync(root), []);
  } finally { slot.close(); await deadline(slot.finished); fs.rmSync(root, { recursive: true }); }
});
test('real oversize watchdog reports before cleanup; parent first stops the producer', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-download-qa-'));
  const slot = new DownloadSlot(binary, root);
  try {
    await deadline(slot.ready);
    const fd = fs.openSync(slot.path, 'wx', 0o600);
    try { fs.ftruncateSync(fd, 512 * 1024 * 1024 + 1); } finally { fs.closeSync(fd); } // sparse synthetic payload
    const limited = once(slot, 'limit'); slot.watch(); await deadline(limited);
    assert.ok(fs.existsSync(slot.path), 'do not unlink while Chromium may still write');
    slot.close();
    assert.deepEqual(await deadline(slot.finished), { publication: 'not_requested', cleanup: true });
    assert.deepEqual(fs.readdirSync(root), []);
  } finally { slot.close(); await deadline(slot.finished); fs.rmSync(root, { recursive: true }); }
});
test('unknown staging entry survives failed cleanup and cannot be reported clean', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-download-qa-'));
  const slot = new DownloadSlot(binary, root);
  try {
    await deadline(slot.ready); slot.watch();
    const sentinel = path.join(path.dirname(slot.path), 'unexpected');
    fs.writeFileSync(slot.path, 'partial'); fs.writeFileSync(sentinel, 'preserve');
    slot.close();
    assert.deepEqual(await deadline(slot.finished), { publication: 'not_requested', cleanup: false });
    assert.equal(fs.readFileSync(sentinel, 'utf8'), 'preserve');
    assert.equal(fs.existsSync(slot.path), false);
  } finally { slot.close(); await deadline(slot.finished); fs.rmSync(root, { recursive: true }); }
});
test('an explicit unconfirmed producer stop retains the payload and reports cleanup failure', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-download-qa-'));
  const slot = new DownloadSlot(binary, root);
  try {
    await deadline(slot.ready); slot.watch(); fs.writeFileSync(slot.path, 'still receiving');
    slot.retain();
    assert.deepEqual(await deadline(slot.finished), { publication: 'not_requested', cleanup: false });
    assert.equal(fs.readFileSync(slot.path, 'utf8'), 'still receiving');
  } finally { slot.close(); await deadline(slot.finished); fs.rmSync(root, { recursive: true }); }
});
