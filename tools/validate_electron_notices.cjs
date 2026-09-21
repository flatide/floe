'use strict';
// Explicit actual window QA: runtime notices and a new synthetic notice only.
// No Rust service, designs, authentication, clipboard or existing user profile.
const electron = require('electron');
const { app } = electron;
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const assert = require('node:assert/strict');
const { Notices, runtimeRoot, ORIGIN } = require('../electron/notices.cjs');
const runtime = require('../electron/runtime.json');
if (process.argv.length !== 2 || process.versions.electron !== runtime.version) throw new Error('Pinned Electron, no arguments required');
app.enableSandbox();
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-notices-qa-'));
fs.chmodSync(root, 0o700);
app.setPath('userData', path.join(root, 'profile'));
app.setPath('sessionData', path.join(root, 'profile'));
let view;
let phase = 'ready';
const timeout = setTimeout(() => finish(1), 30000);
function finish(code) {
  clearTimeout(timeout);
  view?.close();
  console.log(code ? 'ELECTRON NOTICES: FAIL at ' + phase : 'ELECTRON NOTICES: OK (runtime text; scripts/external loads denied; private session; normal close; no clipboard)');
  app.exit(code);
}
function find(web, text) {
  return new Promise(resolve => {
    const listener = (_event, result) => {
      if (result.requestId === id && result.finalUpdate) {
        web.removeListener('found-in-page', listener);
        web.stopFindInPage('clearSelection'); resolve(result.matches);
      }
    };
    web.on('found-in-page', listener);
    const id = web.findInPage(text);
  });
}
async function readyText(web, text, minimum = 1) {
  // did-finish-load/title precedes compositor/search activation. Require the
  // actual native text index, within the unchanged overall 30-second deadline.
  while (await find(web, text) < minimum) await new Promise(resolve => setTimeout(resolve, 20));
}
app.on('window-all-closed', () => {});
app.whenReady().then(async () => {
  phase = 'runtime contents';
  view = new Notices(electron, runtimeRoot(fs.realpathSync(process.execPath), process.platform));
  const pending = view.open();
  pending.catch(error => { console.error(error.stack); finish(1); });
  const web = view.window.webContents;
  await Promise.all([
    new Promise(resolve => web.once('did-finish-load', resolve)),
    new Promise(resolve => view.window.once('ready-to-show', resolve))
  ]);
  assert.equal(web.getTitle(), 'Open Source Licenses');
  await assert.rejects(web.executeJavaScript('document.title'));
  const prefs = web.getLastWebPreferences();
  assert.equal(prefs.javascript, false); assert.equal(prefs.sandbox, true); assert.equal(prefs.nodeIntegration, false);
  assert.equal(web.session.isPersistent(), false);
  phase = 'Electron text';
  await web.loadURL(ORIGIN + '/electron');
  assert.equal(web.getTitle(), 'Electron license');
  await readyText(web, 'Copyright');
  fs.writeFileSync(path.join(root, 'electron.png'), (await web.capturePage()).toPNG());
  console.log('ELECTRON NOTICES: Electron page screenshot ' + path.join(root, 'electron.png'));
  assert.ok(await find(web, 'Copyright'));
  phase = 'Chromium text';
  await web.loadURL(ORIGIN + '/chromium');
  assert.equal(web.getTitle(), 'Credits');
  await readyText(web, '2-dim General Purpose FFT');
  await readyText(web, 'Apache License', 101);
  assert.ok(await find(web, 'Apache License') > 100);
  console.log('ELECTRON NOTICES: upstream Chromium content is searchable');
  phase = 'external request denial';
  await assert.rejects(web.loadURL('https://notices-network-denied.invalid/'));
  await assert.rejects(web.loadURL('file:///floe-notices-no-such-file'));
  await web.loadURL(ORIGIN + '/');
  await readyText(web, 'Notices from the running');
  const image = await web.capturePage();
  assert.equal(image.isEmpty(), false);
  fs.writeFileSync(path.join(root, 'notices.png'), image.toPNG());
  console.log('ELECTRON NOTICES: synthetic screenshot ' + path.join(root, 'notices.png'));
  view.close(); await pending;
  phase = 'synthetic scripts and missing notice';
  const synthetic = path.join(root, 'synthetic'); fs.mkdirSync(synthetic, { mode: 0o700 });
  fs.writeFileSync(path.join(synthetic, 'LICENSES.chromium.html'), '<!doctype html><html><head><title>Unchanged</title><script>document.title="unsafe"</script></head><body><h1>synthetic</h1><script>document.body.textContent="unsafe"</script></body></html>');
  view = new Notices(electron, synthetic);
  const next = view.open();
  next.catch(error => { console.error(error.stack); finish(1); });
  const second = view.window.webContents;
  await Promise.all([
    new Promise(resolve => second.once('did-finish-load', resolve)),
    new Promise(resolve => view.window.once('ready-to-show', resolve))
  ]);
  await second.loadURL(ORIGIN + '/chromium');
  assert.equal(second.getTitle(), 'Unchanged');
  await readyText(second, 'synthetic');
  assert.equal(await find(second, 'synthetic'), 1);
  assert.equal(await find(second, 'unsafe'), 0);
  await second.loadURL(ORIGIN + '/electron');
  assert.equal(second.getTitle(), 'Notice unavailable');
  view.close(); await next;
  finish(0);
}).catch(error => { console.error(error.stack); finish(1); });
