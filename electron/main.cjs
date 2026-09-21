'use strict';
const { app, BrowserWindow, Menu, dialog, session } = require('electron');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomUUID } = require('node:crypto');
const { ServiceClient } = require('./service-client.cjs');
const { CloseController } = require('./close-controller.cjs');
const P = require('./policy.cjs');
const runtime = require('./runtime.json');

app.setName('floe2 Electron comparison');
app.enableSandbox();
const args = process.argv.slice(app.isPackaged ? 1 : 2);
const smoke = args.length === 1 && args[0] === '--smoke-test';
let profile, root, window, service, close, origin = null, panel = false, stopping = false;
let ended = false, failure = false, shuttingDown = false, qaCompleted = false;
let statusURL = null, deniedNavigations = 0, deniedWindows = 0;
const validRuntime = process.versions.electron === runtime.version && ['darwin', 'linux'].includes(process.platform) &&
  !['no-sandbox', 'disable-web-security', 'remote-debugging-port', 'remote-debugging-pipe'].some(s => app.commandLine.hasSwitch(s));

function reveal() {
  if (!window || window.isDestroyed()) return;
  if (window.isMinimized()) window.restore();
  window.show(); window.focus();
}
function status(text) {
  if (!window || window.isDestroyed()) return;
  // Fixed native status only. Never interpolate a path, URL, or an exception.
  const html = '<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src &#39;none&#39;; style-src &#39;unsafe-inline&#39;">' +
    '<style>body{background:#171b23;color:#edf2fa;font:16px system-ui;padding:32px}</style>' +
    '<h1>floe2 · Electron comparison</h1><p>' + text + '</p>';
  statusURL = 'data:text/html;charset=utf-8,' + encodeURIComponent(html);
  window.loadURL(statusURL).catch(() => {});
}
async function message(text, buttons = ['OK']) {
  if (panel || !window || window.isDestroyed()) return 0;
  panel = true;
  try {
    return (await dialog.showMessageBox(window, { type: 'warning', message: text,
      buttons, defaultId: 0, cancelId: 0, noLink: true })).response;
  } finally { panel = false; }
}
function evalOwned(script) {
  if (ended || !window || window.isDestroyed() || !P.navigationAllowed(origin, window.webContents.getURL())) {
    return Promise.resolve('unavailable');
  }
  return window.webContents.executeJavaScript(script, false);
}
function requestClose() {
  reveal();
  if (panel || stopping || !close) return;
  close.request().catch(() => fail());
}
function cancelService() {
  stopping = true;
  status('Stopping this session; waiting for Rust worker cleanup…');
  if (service) service.close(); else finish(0);
}
function fail() {
  if (failure || ended) return;
  failure = true;
  status('The local service or view failed. No operation was replayed. Close this window to end the session. Check the matching Rust binaries and view options.');
  if (smoke) cancelService();
}
async function finish(code) {
  if (shuttingDown) return;
  shuttingDown = true; ended = true;
  if (close) close.end();
  if (code !== 0 && !(stopping && code === 143)) failure = true;
  if (smoke && !qaCompleted) failure = true;
  if (window && !window.isDestroyed()) {
    if (failure && !smoke) await message('The Rust session ended with an error. No save or index request was retried.');
    window.destroy();
  }
  if (root) { try { fs.rmdirSync(root); } catch (_) { failure = true; } }
  if (smoke) console.log(failure ? 'ELECTRON SMOKE: FAIL' : 'ELECTRON SMOKE: OK (sandboxed Chromium; Rust auth; navigation/window denial; close/cancel/quit; service joined)');
  app.exit(failure ? 1 : 0);
}
async function recover() {
  if (panel || stopping || ended || !origin) return;
  reveal();
  const epoch = close.epoch;
  if (await message('Reload the current view? Unsaved editor text is lost. Earlier approved saves may already have completed. No bootstrap or save is replayed.', ['Cancel', 'Reload View']) !== 1 || ended || stopping || epoch !== close.epoch) return;
  close.invalidate();
  window.webContents.loadURL(origin + '/').catch(() => fail());
}
async function menuAction(id) {
  if (panel || stopping || ended) return;
  const script = '(' + fs.readFileSync(path.join(__dirname, '../desktop/ui/menu-action.js'), 'utf8') + ')(' + JSON.stringify(id) + ')';
  const result = await evalOwned(script).catch(() => 'unavailable');
  if (result !== 'opened' && !ended) await message('This action is unavailable while the view is hidden, busy, or disconnected. No action was replayed.');
}

app.on('before-quit', event => { if (!ended) { event.preventDefault(); requestClose(); } });
app.on('window-all-closed', () => { if (!ended) cancelService(); });
app.on('activate', reveal);
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => { failure = smoke; cancelService(); });
// Best-effort exit cleanup, not a secure-erasure/crash-cleanup guarantee.
// Only our fresh generated profile, never a user's browser profile.
process.on('exit', () => { if (profile) { try { fs.rmSync(profile, { recursive: true }); } catch (_) {} } });
process.on('uncaughtException', () => fail());
process.on('unhandledRejection', () => fail());

try {
  if (!validRuntime) throw new Error('runtime');
  profile = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-profile-'));
  fs.chmodSync(profile, 0o700);
  app.setPath('userData', profile); app.setPath('sessionData', profile);
} catch (_) {
  console.error('floe2 Electron: requires the pinned runtime and sandbox; setup failed.');
  app.exit(1);
}

app.whenReady().then(async () => {
  const partition = 'floe-' + randomUUID();
  const ses = session.fromPartition(partition, { cache: false });
  ses.setPermissionCheckHandler(() => false);
  ses.setPermissionRequestHandler((_web, _permission, callback) => callback(false));
  ses.webRequest.onBeforeRequest((details, callback) => callback({ cancel:
    !P.requestAllowed(origin, details.url) && details.url !== statusURL }));
  ses.on('will-download', event => {
    event.preventDefault();
    message('Downloads are not yet supported by this comparison shell. Use the existing WebView app or browser for export.').catch(() => {});
  });
  window = new BrowserWindow({ width: 1100, height: 850, show: true,
    title: 'floe2 · Electron comparison', backgroundColor: '#171b23', webPreferences: P.webPreferences(partition) });
  window.on('close', event => { if (!ended) { event.preventDefault(); requestClose(); } });
  const web = window.webContents;
  web.setWindowOpenHandler(() => { deniedWindows++; return { action: 'deny' }; });
  web.on('will-attach-webview', event => event.preventDefault());
  for (const name of ['will-navigate', 'will-frame-navigate', 'will-redirect']) {
    web.on(name, event => {
      if (!P.navigationAllowed(origin, event.url, event.isMainFrame)) { deniedNavigations++; event.preventDefault(); }
    });
  }
  web.on('did-start-navigation', event => { if (event.isMainFrame && !event.isSameDocument && close) close.invalidate(); });
  web.on('render-process-gone', () => fail());
  close = new CloseController({ reveal, cancelService,
    openDialog: () => evalOwned("(()=>{const b=document.getElementById('logout');if(b&&!b.disabled){b.click();return 'opened';}return 'unavailable';})()"),
    confirmForce: async () => await message('End this session? Use when the normal confirmation is unavailable. Unsaved drafts are discarded; approved writes may already have completed. No save will be replayed.', ['Cancel', 'End Session']) === 1 });
  Menu.setApplicationMenu(Menu.buildFromTemplate([
    ...(process.platform === 'darwin' ? [{ label: 'floe2', submenu: [{ label: 'Quit floe2', accelerator: 'Command+Q', click: requestClose }] }] : []),
    { label: 'File', submenu: [
      { label: 'Open layout…', click: () => menuAction('browse-open') },
      { label: 'Open DRC…', click: () => menuAction('drc-open') },
      { label: 'End session…', accelerator: 'CmdOrCtrl+W', click: requestClose }] },
    { label: 'Edit', submenu: [{ role: 'undo' }, { role: 'redo' }, { type: 'separator' }, { role: 'cut' }, { role: 'copy' }, { role: 'paste' }, { role: 'selectAll' }] },
    { label: 'View', submenu: [{ label: 'Recover View…', click: recover }, { label: 'About…', click: () => menuAction('about-open') }] }
  ]));
  status('Starting the Rust service. Close this window to cancel. This is a comparison build; download and programmatic clipboard permissions are not enabled.');
  const viewArgs = smoke ? [] : (args[0] === 'view' ? args.slice(1) : args);
  if (smoke) {
    root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-smoke-'));
    fs.chmodSync(root, 0o700); viewArgs.push('--root', root);
  }
  const repo = path.resolve(__dirname, '..');
  const binary = P.serviceBinary(process.env, path.join(__dirname, 'service/target/release/floe-electron-service'));
  const env = { ...process.env };
  for (const [key, name] of [['FLOE_INDEX_BIN', 'floe-index'], ['FLOE_RENDERD_BIN', 'floe-renderd']]) {
    if (!(key in env)) env[key] = path.join(repo, 'rust/target/release', name);
  }
  service = new ServiceClient(binary, viewArgs, env);
  service.on('failure', fail);
  service.on('ended', result => finish(result.code));
  service.on('directory', async () => {
    panel = true;
    try {
      const choice = await dialog.showOpenDialog(window, { title: 'Choose an approved layout folder', properties: ['openDirectory'] });
      if (choice.canceled || choice.filePaths.length !== 1) cancelService();
      else if (!ended && !stopping) service.chooseDirectory(choice.filePaths[0]);
    } catch (_) { fail(); cancelService(); } finally { panel = false; }
  });
  service.on('ready', ready => {
    if (stopping || ended) return;
    origin = ready.origin;
    web.loadURL(ready.url).then(() => { if (smoke) runSmoke().catch(() => { failure = true; cancelService(); }); }).catch(() => fail());
  });
}).catch(() => { failure = true; cancelService(); });

async function runSmoke() {
  const wait = async (script) => {
    const end = Date.now() + 30000;
    while (Date.now() < end && !ended && !stopping) {
      if (await evalOwned(script) === true) return;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    throw new Error('synthetic QA did not reach expected state');
  };
  await wait("(()=>{const b=document.getElementById('logout');return !document.hidden&&!location.hash&&!!b&&!b.disabled;})()");
  await wait("/No matching entries/.test(document.getElementById('browse-status').textContent)");
  const prefs = window.webContents.getLastWebPreferences();
  if (!prefs.sandbox || !prefs.contextIsolation || prefs.nodeIntegration || prefs.preload || window.webContents.session.isPersistent()) throw new Error('invalid synthetic preferences');
  if (await evalOwned("typeof require==='undefined'&&typeof process==='undefined'") !== true) throw new Error('Node exposed');
  // Attempt navigation only to a fresh owned loopback sentinel, never an external
  // host. Require the actual host guard to fire, not just a coincidental CSP block.
  const sentinel = require('node:http').createServer((_req, res) => { requests++; res.end('synthetic'); });
  let requests = 0;
  try {
    await new Promise((resolve, reject) => { sentinel.once('error', reject); sentinel.listen(0, '127.0.0.1', resolve); });
    const target = 'http://127.0.0.1:' + sentinel.address().port + '/';
    const beforeNav = deniedNavigations, beforeWindow = deniedWindows;
    await evalOwned('location.href=' + JSON.stringify(target) + '; true');
    const deadline = Date.now() + 5000;
    while (deniedNavigations === beforeNav && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
    await evalOwned('window.open(' + JSON.stringify(target) + '); true');
    if (deniedNavigations <= beforeNav || deniedWindows <= beforeWindow || requests !== 0 ||
        !P.navigationAllowed(origin, window.webContents.getURL()) || BrowserWindow.getAllWindows().length !== 1) {
      throw new Error('synthetic navigation guard failed');
    }
  } finally { await new Promise(resolve => sentinel.close(resolve)); }
  // Capture only the authenticated synthetic empty-root page, after checking
  // visibility. A capture must not be used to make a hidden-page test pass.
  if (await evalOwned('!document.hidden&&!location.hash') !== true) throw new Error('hidden synthetic page');
  const capture = await window.webContents.capturePage(undefined, { stayHidden: true, stayAwake: false });
  if (capture.isEmpty()) throw new Error('empty synthetic capture');
  const artifacts = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-e1-artifacts-'));
  fs.chmodSync(artifacts, 0o700);
  fs.writeFileSync(path.join(artifacts, 'window.png'), capture.toPNG(), { flag: 'wx', mode: 0o600 });
  console.log('ELECTRON SMOKE: synthetic screenshot ' + path.join(artifacts, 'window.png'));
  await close.request();
  await wait("!document.getElementById('session-exit-dialog').hidden&&document.activeElement.id==='session-exit-cancel'");
  await evalOwned("document.getElementById('session-exit-cancel').click()");
  await wait("document.getElementById('session-exit-dialog').hidden&&!document.getElementById('logout').disabled");
  app.quit();
  await wait("!document.getElementById('session-exit-dialog').hidden");
  qaCompleted = true;
  await evalOwned("document.getElementById('session-exit-confirm').click()");
}
