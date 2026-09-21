'use strict';
const { app, BrowserWindow, Menu, dialog, session } = require('electron');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { randomUUID } = require('node:crypto');
const { ServiceClient } = require('./service-client.cjs');
const { CloseController } = require('./close-controller.cjs');
const { Downloads, blobAllowed, postAllowed, mimeAllowed, outsideProfile } = require('./downloads.cjs');
const P = require('./policy.cjs');
const runtime = require('./runtime.json');

app.setName('floe2 Electron comparison');
app.enableSandbox();
const args = process.argv.slice(app.isPackaged ? 1 : 2);
const emptySmoke = args.length === 1 && args[0] === '--smoke-test';
const downloadSmoke = args.length === 1 && args[0] === '--smoke-download-test';
// Explicit read-only QA of a pre-indexed synthetic source. No DRC/default writes,
// indexing or caller-chosen output path. The validation driver creates the source.
const layoutSmoke = args.length === 2 && args[0] === '--smoke-layout-test' && path.isAbsolute(args[1]);
const clipSmoke = args.length === 2 && args[0] === '--smoke-clip-download-test' && path.isAbsolute(args[1]);
const smoke = emptySmoke || layoutSmoke || downloadSmoke || clipSmoke;
let profile, root, window, service, close, origin = null, panel = false, stopping = false;
let downloads, downloadQaRoot, downloadQaChoice = 0, cleanupConfirmed = false;
const downloadWindows = new Map();
let ended = false, failure = false, shuttingDown = false, qaCompleted = false;
let statusURL = null, deniedNavigations = 0, deniedWindows = 0;
let qaStep = 'startup';
let qaPostResponses = 0, qaRejectedPostResponses = 0;
let qaCancelReceive = false, qaCancelledActive = false, qaReceiveCleanup;
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
  const downloadCleanup = downloads ? downloads.shutdown() : Promise.resolve(true);
  if (window && !window.isDestroyed()) {
    if (failure && !smoke) await message('The Rust session ended with an error. No save or index request was retried.');
    window.destroy();
  }
  for (const entry of downloadWindows.values()) { if (!entry.window.isDestroyed()) entry.window.destroy(); }
  cleanupConfirmed = await downloadCleanup;
  if (!cleanupConfirmed) { failure = true; console.error('floe2 Electron: private download cleanup not confirmed; profile retained; no replay.'); }
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
process.on('exit', () => { if (profile && (!downloads || cleanupConfirmed)) { try { fs.rmSync(profile, { recursive: true }); } catch (_) {} } });
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
    !P.requestAllowed(origin, details.url) && !blobAllowed(origin, details.url) && details.url !== statusURL }));
  ses.webRequest.onHeadersReceived((details, callback) => {
    const entry = downloadWindows.get(details.webContentsId);
    if (!entry) { callback({}); return; }
    const headers = details.responseHeaders || {};
    const type = Object.entries(headers).find(([k]) => k.toLowerCase() === 'content-type')?.[1]?.[0]?.split(';')[0];
    const valid = !entry.responded && details.url === entry.url && details.method === 'POST' && details.statusCode === 200 && mimeAllowed(type);
    if (clipSmoke && valid) qaPostResponses++;
    if (clipSmoke && !valid) qaRejectedPostResponses++;
    entry.responded = true;
    callback({ cancel: !valid });
    if (!valid) setImmediate(() => { if (!entry.window.isDestroyed()) entry.window.destroy(); });
  });
  ses.on('will-download', (event, item, contents) => {
    if (downloads) downloads.receive(event, item, contents); else event.preventDefault();
    if (downloadSmoke && qaCancelReceive) {
      qaCancelledActive = !!downloads.active && !downloads.active.ended;
      qaReceiveCleanup = downloads.shutdown(); // same producer fence as normal session shutdown
    }
    const entry = contents && downloadWindows.get(contents.id);
    if (entry) setImmediate(() => { if (!entry.window.isDestroyed()) entry.window.destroy(); });
  });
  window = new BrowserWindow({ width: 1100, height: 850, show: true,
    title: 'floe2 · Electron comparison', backgroundColor: '#171b23', webPreferences: P.webPreferences(partition) });
  window.on('close', event => { if (!ended) { event.preventDefault(); requestClose(); } });
  const web = window.webContents;
  web.setWindowOpenHandler(details => {
    if (!stopping && !ended && !downloadWindows.size && downloads && !downloads.active &&
        downloads.slot.phase === 'ready' && postAllowed(origin, details.url) && details.postBody) {
      return { action: 'allow', overrideBrowserWindowOptions: { show: false, webPreferences: P.webPreferences(partition) } };
    }
    deniedWindows++; return { action: 'deny' };
  });
  web.on('did-create-window', (child, details) => {
    const entry = { window: child, url: details.url, responded: false };
    downloadWindows.set(child.webContents.id, entry);
    const id = child.webContents.id;
    const timeout = setTimeout(() => { if (!child.isDestroyed()) child.destroy(); }, 30000);
    child.once('closed', () => { clearTimeout(timeout); downloadWindows.delete(id); });
    child.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
    child.webContents.on('will-redirect', event => event.preventDefault());
    child.webContents.on('will-frame-navigate', event => { if (!event.isMainFrame || event.url !== entry.url || entry.responded) event.preventDefault(); });
    child.webContents.on('did-finish-load', () => { if (!child.isDestroyed()) child.destroy(); });
    child.webContents.on('did-fail-load', () => { if (!child.isDestroyed()) child.destroy(); });
  });
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
  status('Starting the Rust service. Close this window to cancel. Native exports save to new files only; programmatic clipboard permissions are not enabled.');
  const viewArgs = layoutSmoke || clipSmoke ? [args[1], '--goto', '200,200,300', '--depth', 'full', '--detail', 'high',
    '--jobs', '4', '--raster-jobs', '4', '--refinement', 'off'] : emptySmoke || downloadSmoke ? [] : (args[0] === 'view' ? args.slice(1) : args);
  if (clipSmoke) viewArgs.push('--budget-mb', '256'); // leave managed capacity for explicit exact export
  if (emptySmoke || downloadSmoke) {
    root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-smoke-'));
    fs.chmodSync(root, 0o700); viewArgs.push('--root', root);
  }
  const repo = path.resolve(__dirname, '..');
  const binary = P.serviceBinary(process.env, path.join(__dirname, 'service/target/release/floe-electron-service'));
  const downloadBinary = Object.hasOwn(process.env, 'FLOE_ELECTRON_DOWNLOAD_BIN') ? process.env.FLOE_ELECTRON_DOWNLOAD_BIN : path.join(path.dirname(binary), 'floe-electron-download');
  if (!downloadBinary) throw new Error('Invalid download binary');
  if (downloadSmoke || clipSmoke) { downloadQaRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-export-qa-')); fs.chmodSync(downloadQaRoot, 0o700); }
  downloads = new Downloads({ binary: downloadBinary, directory: profile, origin: () => origin,
    owns: (contents, url) => !!contents && (contents.id === web.id || downloadWindows.get(contents.id)?.url === url),
    choose: async name => {
      if (downloadSmoke) return downloadQaChoice++ === 0 ? null : path.join(downloadQaRoot, 'synthetic.json');
      if (clipSmoke) return path.join(downloadQaRoot, 'synthetic.oas');
      if (panel || stopping || ended) return null;
      panel = true;
      let choice;
      try { choice = await dialog.showSaveDialog(window, { title: 'Save export to a NEW file (never replace)', defaultPath: name }); }
      finally { panel = false; }
      if (choice.canceled || stopping || ended) return null;
      if (!outsideProfile(profile, choice.filePath)) {
        await message('Choose an accessible folder outside this app’s temporary profile. No file was published.');
        return null;
      }
      return choice.filePath;
    },
    notify: async code => {
      if (downloadSmoke || clipSmoke || ended || stopping) return;
      const text = { saved: 'Export saved to the selected new file.', not_requested: 'Download cancelled or interrupted. No file was published.',
        unconfirmed: 'Export was not confirmed. Existing files are never replaced. Check the selected destination; no automatic retry was made.',
        cleanup_failed: 'Private download cleanup was not confirmed. Check the selected folder. No save will be replayed.',
        rejected: 'Download rejected: another export is active, or the origin, type, redirect or size is not allowed.' }[code];
      if (code === 'saved' || code === 'not_requested') await evalOwned('(()=>{const e=document.getElementById("notice");e.hidden=false;e.textContent=' + JSON.stringify(text) + ';return true;})()');
      else await message(text);
    }
  });
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
  service.on('ready', async ready => {
    if (stopping || ended) return;
    origin = ready.origin;
    try { await downloads.ready; } catch (_) { fail(); return; }
    if (stopping || ended) return;
    web.loadURL(ready.url).then(() => { if (smoke) runSmoke().catch(() => {
      console.log('ELECTRON SMOKE: failed stage ' + qaStep); failure = true; cancelService();
    }); }).catch(() => fail());
  });
}).catch(() => { failure = true; cancelService(); });

async function runSmoke() {
  qaStep = 'authentication';
  const wait = async (script) => {
    const end = Date.now() + 30000;
    while (Date.now() < end && !ended && !stopping) {
      if (await evalOwned(script) === true) return;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    throw new Error('synthetic QA did not reach expected state');
  };
  await wait("(()=>{const b=document.getElementById('logout');return !document.hidden&&!location.hash&&!!b&&!b.disabled;})()");
  if (emptySmoke || downloadSmoke) await wait("/No matching entries/.test(document.getElementById('browse-status').textContent)");
  const prefs = window.webContents.getLastWebPreferences();
  if (!prefs.sandbox || !prefs.contextIsolation || prefs.nodeIntegration || prefs.preload || window.webContents.session.isPersistent()) throw new Error('invalid synthetic preferences');
  if (await evalOwned("typeof require==='undefined'&&typeof process==='undefined'") !== true) throw new Error('Node exposed');
  qaStep = 'navigation guards';
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
  if (layoutSmoke) {
    qaStep = 'layout actions';
    await require('./layout-qa.cjs').run({ app, window, evalOwned, service, extraRustPids: downloads.pids() });
  }
  if (downloadSmoke) {
    qaStep = 'download cancel publish conflict';
    await runDownloadSmoke();
  }
  if (clipSmoke) await runClipSmoke(wait);
  qaStep = 'capture';
  // Capture only the authenticated synthetic empty-root page, after checking
  // visibility. A capture must not be used to make a hidden-page test pass.
  if (await evalOwned('!document.hidden&&!location.hash') !== true) throw new Error('hidden synthetic page');
  const capture = await window.webContents.capturePage(undefined, { stayHidden: true, stayAwake: false });
  if (capture.isEmpty()) throw new Error('empty synthetic capture');
  const artifacts = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-e1-artifacts-'));
  fs.chmodSync(artifacts, 0o700);
  fs.writeFileSync(path.join(artifacts, 'window.png'), capture.toPNG(), { flag: 'wx', mode: 0o600 });
  console.log('ELECTRON SMOKE: synthetic screenshot ' + path.join(artifacts, 'window.png'));
  qaStep = 'close cancel quit';
  await close.request();
  await wait("!document.getElementById('session-exit-dialog').hidden&&document.activeElement.id==='session-exit-cancel'");
  await evalOwned("document.getElementById('session-exit-cancel').click()");
  await wait("document.getElementById('session-exit-dialog').hidden&&!document.getElementById('logout').disabled");
  app.quit();
  await wait("!document.getElementById('session-exit-dialog').hidden");
  qaCompleted = true;
  await evalOwned("document.getElementById('session-exit-confirm').click()");
}

async function runClipSmoke(wait) {
  // Only the explicit synthetic fixture driver invokes this mode. Approval
  // stays in the existing web controller; the host never reads/replays CSRF.
  qaStep = 'clip open';
  await wait("!document.getElementById('clip-open').disabled&&/^Live/.test(document.getElementById('status').textContent)");
  await evalOwned("document.getElementById('clip-open').click();true");
  await wait("!document.getElementById('clip-prepare').disabled");
  qaStep = 'clip prepare';
  await evalOwned("document.getElementById('clip-prepare').click();true");
  await wait("!document.getElementById('clip-approve').disabled");
  qaStep = 'clip approve';
  await evalOwned("document.getElementById('clip-approve').click();true");
  await wait("!!document.querySelector('#clip-files button[data-kind=save]:not(:disabled)')");
  qaStep = 'clip original POST download';
  await downloads.ready;
  await evalOwned("document.querySelector('#clip-files button[data-kind=save]').click();true");
  const end = Date.now() + 30000;
  while ((!downloads.results.length || downloads.active || downloadWindows.size) && Date.now() < end) await new Promise(resolve => setTimeout(resolve, 20));
  const result = downloads.results[0], saved = path.join(downloadQaRoot, 'synthetic.oas');
  if (!result || result.publication !== 'saved' || !result.cleanup || qaPostResponses !== 1 || downloadWindows.size ||
      !P.navigationAllowed(origin, window.webContents.getURL()) || BrowserWindow.getAllWindows().length !== 1) {
    console.log('ELECTRON DOWNLOAD: POST flags ' + JSON.stringify({ responses: qaPostResponses,
      results: downloads.results, child_windows: downloadWindows.size, active: !!downloads.active }));
    throw new Error('Synthetic POST download failed');
  }
  if (fs.readFileSync(saved).subarray(0, 13).toString('ascii') !== '%SEMI-OASIS\r\n' ||
      (fs.statSync(saved).mode & 0o777) !== 0o600 || fs.readdirSync(downloadQaRoot).join(',') !== 'synthetic.oas') throw new Error('Invalid synthetic clip');
  qaStep = 'clip rejected GET and unauthenticated POST';
  await downloads.ready;
  const deniedBefore = deniedWindows, responseBefore = qaRejectedPostResponses;
  await evalOwned("(()=>{const id=document.querySelector('#clip-files button[data-kind=save]').dataset.artifact;window.open('/api/v1/artifacts/'+id+'/download');return true;})()");
  if (deniedWindows !== deniedBefore + 1) throw new Error('Non-POST artifact popup allowed');
  // Deliberately invalid CSRF, not a copy of the secret or a replay of the
  // successful download. The real gateway must reject before a file is saved.
  await evalOwned("(()=>{const id=document.querySelector('#clip-files button[data-kind=save]').dataset.artifact,f=document.createElement('form'),i=document.createElement('input');f.method='POST';f.target='_blank';f.rel='noopener';f.action='/api/v1/artifacts/'+id+'/download';i.name='csrf';i.value='synthetic-invalid';f.appendChild(i);document.body.appendChild(f);f.submit();f.remove();return true;})()");
  const rejectedEnd = Date.now() + 10000;
  while ((qaRejectedPostResponses === responseBefore || downloadWindows.size) && Date.now() < rejectedEnd) await new Promise(resolve => setTimeout(resolve, 20));
  if (qaRejectedPostResponses !== responseBefore + 1 || downloadWindows.size || downloads.active || downloads.results.length !== 1 ||
      !P.navigationAllowed(origin, window.webContents.getURL()) || BrowserWindow.getAllWindows().length !== 1) throw new Error('Rejected POST navigated or downloaded');
  console.log('ELECTRON DOWNLOAD: synthetic artifacts ' + downloadQaRoot);
  console.log('ELECTRON DOWNLOAD: OK (real approved clip; original authenticated POST once; GET/invalid-CSRF denied; injected native destination; OASIS 0600; cleanup)');
}

async function runDownloadSmoke() {
  // Native destination decisions are injected ONLY in this no-argument QA.
  // No existing directory/design/reviewer is accepted by the test.
  const payload = '{"synthetic":"Electron export — 한글"}\n';
  const saved = path.join(downloadQaRoot, 'synthetic.json');
  for (let i = 0; i < 3; i++) {
    await downloads.ready;
    const before = downloads.results.length;
    await evalOwned('(()=>{const a=document.createElement("a"),u=URL.createObjectURL(new Blob([' + JSON.stringify(payload) +
      '],{type:"application/json"}));a.href=u;a.download="synthetic.json";document.body.appendChild(a);a.click();a.remove();setTimeout(()=>URL.revokeObjectURL(u),1000);return true;})()');
    const end = Date.now() + 30000;
    while (downloads.results.length === before && Date.now() < end) await new Promise(resolve => setTimeout(resolve, 20));
    const result = downloads.results[before];
    const expected = ['not_requested', 'saved', 'unconfirmed'][i];
    if (!result || result.publication !== expected || !result.cleanup) throw new Error('Synthetic native export result incorrect');
    if (i === 0 && fs.existsSync(saved)) throw new Error('Cancelled export published');
    if (i > 0 && (fs.readFileSync(saved, 'utf8') !== payload || (fs.statSync(saved).mode & 0o777) !== 0o600)) throw new Error('Synthetic export changed');
    while (downloads.active && Date.now() < end) await new Promise(resolve => setTimeout(resolve, 20));
  }
  if (fs.readdirSync(downloadQaRoot).join(',') !== 'synthetic.json') throw new Error('Unexpected synthetic output');
  await downloads.ready;
  qaCancelReceive = true;
  await evalOwned("(()=>{const a=document.createElement('a'),u=URL.createObjectURL(new Blob([new Uint8Array(16*1024*1024)],{type:'application/octet-stream'}));a.href=u;a.download='cancelled.bin';document.body.appendChild(a);a.click();a.remove();setTimeout(()=>URL.revokeObjectURL(u),1000);return true;})()");
  const cancelEnd = Date.now() + 30000;
  while (!qaReceiveCleanup && Date.now() < cancelEnd) await new Promise(resolve => setTimeout(resolve, 20));
  if (!qaCancelledActive || !qaReceiveCleanup || !await qaReceiveCleanup || downloads.results[3]?.publication !== 'not_requested' ||
      fs.readdirSync(downloadQaRoot).join(',') !== 'synthetic.json') throw new Error('Active Chromium download cancellation failed');
  console.log('ELECTRON DOWNLOAD: synthetic artifacts ' + downloadQaRoot);
  console.log('ELECTRON DOWNLOAD: OK (real Chromium blob; injected cancel/save/conflict; active receive cancellation; Rust 0600 no-clobber; cleanup)');
}
