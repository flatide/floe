'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { runtimeRoot, readNotice, documentFor, route, ORIGIN, CSP, LIMIT } = require('./notices.cjs');
const { Notices } = require('./notices.cjs');
const { EventEmitter } = require('node:events');

test('notice paths come only from the actual runtime layout', () => {
  assert.equal(runtimeRoot('/tmp/runtime/Electron.app/Contents/MacOS/Electron', 'darwin'), '/tmp/runtime');
  assert.equal(runtimeRoot('/tmp/runtime/electron', 'linux'), '/tmp/runtime');
  assert.throws(() => runtimeRoot('electron', 'linux'));
  assert.throws(() => runtimeRoot('/runtime/electron', 'win32'));
});
test('only three exact notice URLs; no external browsing or file lookup', () => {
  for (const r of ['/', '/electron', '/chromium']) assert.equal(route(ORIGIN + r), r);
  for (const value of [null, 2, ORIGIN, ORIGIN + '/../../LICENSE', ORIGIN + '/electron?path=secret',
    ORIGIN + '/electron#x', ORIGIN + '/%65lectron', ORIGIN + ':443/electron', 'file:///LICENSE',
    'https://external.invalid/', 'https://user@floe-notices.invalid/electron']) assert.equal(route(value), null);
  for (const policy of ["default-src 'none'", "script-src 'none'", "form-action 'none'", "base-uri 'none'", 'sandbox']) assert.ok(CSP.includes(policy));
});
test('fixed files, escaped Electron text, complete Chromium body; no directory fallback', async t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-notices-unit-'));
  t.after(() => fs.rmSync(root, { recursive: true }));
  fs.writeFileSync(path.join(root, 'LICENSE'), 'sample <script>bad</script> & 한글');
  const html = '<!doctype html><html><head><title>Credits</title></head><body><pre>test license &amp; data</pre></body></html>';
  fs.writeFileSync(path.join(root, 'LICENSES.chromium.html'), html);
  assert.equal(await readNotice(root, 'LICENSES.chromium.html'), html);
  assert.match(await documentFor(root, ORIGIN + '/electron'), /&lt;script&gt;bad&lt;\/script&gt; &amp; 한글/);
  assert.ok((await documentFor(root, ORIGIN + '/chromium')).includes('<pre>test license &amp; data</pre></body>'));
  assert.match(await documentFor(root, ORIGIN + '/'), /supplied separately/);
  await assert.rejects(readNotice(root, '../LICENSE'));
  await assert.rejects(documentFor(root, ORIGIN + '/unknown'));
  fs.unlinkSync(path.join(root, 'LICENSE'));
  await assert.rejects(documentFor(root, ORIGIN + '/electron'));
  fs.symlinkSync('LICENSES.chromium.html', path.join(root, 'LICENSE'));
  await assert.rejects(readNotice(root, 'LICENSE'));
  fs.unlinkSync(path.join(root, 'LICENSE'));
  fs.linkSync(path.join(root, 'LICENSES.chromium.html'), path.join(root, 'LICENSE'));
  await assert.rejects(readNotice(root, 'LICENSE'));
});
test('reject empty, oversized, directory and invalid UTF-8 without reading arbitrary paths', async t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-notices-bounds-'));
  t.after(() => fs.rmSync(root, { recursive: true }));
  const file = path.join(root, 'LICENSE');
  fs.writeFileSync(file, '');
  await assert.rejects(readNotice(root, 'LICENSE'));
  fs.truncateSync(file, LIMIT + 1);
  await assert.rejects(readNotice(root, 'LICENSE'));
  fs.writeFileSync(file, Buffer.from([255, 254]));
  await assert.rejects(readNotice(root, 'LICENSE'));
  fs.unlinkSync(file); fs.mkdirSync(file);
  await assert.rejects(readNotice(root, 'LICENSE'));
});

test('notice session denies permissions, downloads, other windows and non-mainframe requests', async () => {
  const ses = new EventEmitter();
  ses.setPermissionCheckHandler = f => { ses.check = f; };
  ses.setPermissionRequestHandler = f => { ses.permission = f; };
  ses.webRequest = { onBeforeRequest: f => { ses.request = f; } };
  ses.protocol = { handle: (scheme, f) => { assert.equal(scheme, 'https'); ses.handler = f; },
    unhandle: scheme => { assert.equal(scheme, 'https'); ses.removed = true; } };
  class Window extends EventEmitter {
    constructor(options) {
      super(); this.options = options; this.webContents = new EventEmitter();
      Object.assign(this.webContents, { id: 19, setWindowOpenHandler: f => { this.openHandler = f; },
        loadURL: async url => { this.loaded = url; } });
    }
    isDestroyed() { return !!this.destroyed; }
    destroy() { this.destroyed = true; this.emit('closed'); }
    close() { this.destroy(); }
    show() { this.shown = true; }
    focus() { this.focused = true; }
    setMenu() {}
  }
  const n = new Notices({ BrowserWindow: Window, Menu: { buildFromTemplate: t => t },
    session: { fromPartition: (partition, opts) => {
      assert.ok(partition.startsWith('floe-notices-')); assert.equal(opts.cache, false); return ses;
    } } }, '/unused');
  const done = n.open(); const w = n.window;
  assert.equal(w.options.webPreferences.javascript, false);
  assert.equal(w.options.webPreferences.preload, undefined);
  assert.equal(ses.check(), false);
  let grant; ses.permission(null, 'clipboard-sanitized-write', v => { grant = v; }); assert.equal(grant, false);
  let prevented = 0; const event = { preventDefault: () => prevented++ };
  ses.emit('will-download', event); w.webContents.emit('will-redirect', event);
  w.webContents.emit('will-attach-webview', event);
  w.webContents.emit('will-frame-navigate', { ...event, url: ORIGIN + '/', isMainFrame: false });
  w.webContents.emit('will-navigate', { ...event, url: 'file:///forbidden' });
  assert.equal(prevented, 5); assert.deepEqual(w.openHandler(), { action: 'deny' });
  const valid = { webContentsId: 19, method: 'GET', resourceType: 'mainFrame', url: ORIGIN + '/' };
  for (const extra of [{}, { webContentsId: 20 }, { method: 'POST' }, { resourceType: 'subFrame' }, { url: 'https://outside.invalid/' }]) {
    let cancel; ses.request({ ...valid, ...extra }, r => { cancel = r.cancel; });
    assert.equal(cancel, Object.keys(extra).length !== 0);
  }
  const response = await ses.handler({ method: 'GET', url: ORIGIN + '/' });
  assert.equal(response.status, 200); assert.equal(response.headers.get('Content-Security-Policy'), CSP);
  assert.match(await response.text(), /Open Source Licenses/);
  for (const request of [{ method: 'POST', url: ORIGIN + '/' }, { method: 'GET', url: ORIGIN + '/arbitrary' }]) {
    assert.equal((await ses.handler(request)).status, 404);
  }
  await n.open(); assert.equal(n.window, w); assert.ok(w.focused);
  n.close(); await done; assert.equal(n.window, null); assert.ok(ses.removed);
  let cancelled; ses.request(valid, r => { cancelled = r.cancel; }); assert.equal(cancelled, true);
});
