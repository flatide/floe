'use strict';
// Fresh synthetic session, real Rust service SIGKILL, shipped Electron main.
// No clipboard, input paths, authentication files, save approval or auto-restart.
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { spawnSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { app, BrowserWindow, Menu, dialog } = require('electron');
const repo = path.resolve(__dirname, '..');
const pendingPrompt = process.argv.length === 3 && process.argv[2] === '--pending-prompt';
const signalExit = process.argv.length === 3 && process.argv[2] === '--signal';
if ((process.argv.length !== 2 && !pendingPrompt && !signalExit) || process.versions.electron !== require('../electron/runtime.json').version) {
  throw Error('Pinned Electron; only optional --pending-prompt or --signal accepted');
}
app.enableSandbox();
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-service-exit-ui-'));
fs.chmodSync(root, 0o700);
require('./electron-review-fixture.cjs').writeInputs(root);
console.log('SERVICE EXIT UI: synthetic artifacts ' + root);
for (const [key, fallback] of Object.entries({ FLOE_INDEX_BIN: 'rust/target/release/floe-index',
  FLOE_RENDERD_BIN: 'rust/target/release/floe-renderd', FLOE_ELECTRON_SERVICE_BIN: 'electron/service/target/debug/floe-electron-service' })) {
  if (!Object.hasOwn(process.env, key)) process.env[key] = path.join(repo, fallback);
  if (!path.isAbsolute(process.env[key])) throw Error('Absolute binary required');
  fs.accessSync(process.env[key], fs.constants.X_OK);
}
for (const args of [['vfs', path.join(root, 'synthetic.oas'), path.join(root, '.synthetic.oas.ice'), '--jobs', '2'],
  ['drc', path.join(root, 'synthetic.db'), '--jobs', '2']]) {
  if (spawnSync(process.env.FLOE_INDEX_BIN, args, { stdio: 'ignore', timeout: 120000 }).status !== 0) throw Error('Synthetic index failed');
}
function fingerprint() {
  const out = {};
  function walk(dir) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else {
        if (!entry.isFile()) throw Error('Unexpected fixture entry');
        const s = fs.statSync(full);
        out[path.relative(root, full)] = [s.mode, s.size, s.mtimeMs, createHash('sha256').update(fs.readFileSync(full)).digest('hex')];
      }
    }
  }
  walk(root); return JSON.stringify(out);
}
const before = fingerprint();
let service, window, origin, complete = false, failed = false, stage = 'startup';
let reloadPrompt = null, reloadCancelled = false, serviceCount = 0, workers = [];
const realExit = app.exit.bind(app);
app.exit = code => {
  const ok = code === 1 && complete && !failed && service?.phase === 'ended' && serviceCount === 1 && fingerprint() === before;
  console.log(ok ? 'SERVICE EXIT UI: OK (actual service SIGKILL; terminal display; no reload/save; explicit close; exit1)' : 'SERVICE EXIT UI: FAIL at ' + stage);
  realExit(ok ? 0 : 1);
};
const client = require('../electron/service-client.cjs'), ActualClient = client.ServiceClient;
client.ServiceClient = class extends ActualClient {
  constructor(...args) {
    super(...args); service = this; serviceCount++;
    this.on('ready', value => { origin = value.origin; });
  }
};
const actualMessage = dialog.showMessageBox.bind(dialog);
dialog.showMessageBox = async (parent, options) => {
  if (options.message.startsWith('Reload the current view?')) {
    if (!pendingPrompt || options.defaultId !== 0 || options.cancelId !== 0 || options.buttons.join('|') !== 'Cancel|Reload View') throw Error('Wrong confirmation');
    reloadPrompt = options.signal;
    const result = await actualMessage(parent, options); // real native sheet, cancelled by product AbortSignal
    reloadCancelled = options.signal.aborted && result.response === 0;
    return result;
  }
  return actualMessage(parent, options);
};
process.argv = [process.argv[0], path.join(repo, 'electron'), 'view', path.join(root, 'synthetic.oas'),
  '--drc', path.join(root, '.synthetic.db.tray'), '--drc-reviewer', 'service-exit-ui', '--drc-edit-waives',
  '--jobs', '2', '--raster-jobs', '1', '--budget-mb', '256', '--refinement', 'off'];
require('../electron/main.cjs');
function check(value) { if (!value) throw Error('Synthetic assertion'); }
async function until(fn, timeout = 30000) {
  let timer, stopped = false;
  try {
    await Promise.race([(async () => {
      while (!stopped && !failed) {
        if (await fn()) return;
        await new Promise(resolve => setTimeout(resolve, 25));
      }
      throw Error('Synthetic check stopped');
    })(), new Promise((_, reject) => { timer = setTimeout(() => reject(Error('Stage timeout')), timeout); })]);
  } finally { stopped = true; clearTimeout(timer); }
}
const evaluate = script => window.webContents.executeJavaScript(script, false);
const wait = script => until(async () => await evaluate('(()=>{const e=id=>document.getElementById(id);return ' + script + ';})()') === true);
const action = script => evaluate('(()=>{const e=id=>document.getElementById(id);' + script + ';return true;})()');
function menu(label) { return Menu.getApplicationMenu().items.find(x => x.label === 'View').submenu.items.find(x => x.label === label); }
function ownedChildren() {
  const p = spawnSync('/bin/ps', ['-axo', 'pid=,ppid='], { encoding: 'utf8', timeout: 5000 });
  check(p.status === 0);
  return p.stdout.trim().split('\n').map(s => s.trim().split(/\s+/).map(Number))
    .filter(([pid, parent]) => Number.isSafeInteger(pid) && pid > 0 && parent === service.child.pid).map(([pid]) => pid);
}
async function run() {
  await until(() => { window = BrowserWindow.getAllWindows()[0]; return origin && window && window.webContents.getURL() === origin + '/'; });
  await require('../electron/readiness-qa.cjs').waitReady({ evaluate, alive: () => service.phase !== 'ended', observe: () => {} });
  stage = 'prepare unsaved note';
  await wait("e('drc-errors').children.length===2&&!e('drc-errors').children[0].disabled");
  await action("e('drc-errors').children[0].click()");
  await wait("!e('notes-read').disabled&&/^Saved notes · service-exit-ui · revision/.test(e('drc-notes-status').textContent)");
  await action("e('notes-read').click()");
  await wait("!e('notes-text').disabled&&!e('notes-editor').hidden");
  await action("e('notes-text').value='Unapproved synthetic service-exit draft';e('notes-text').dispatchEvent(new Event('input',{bubbles:true}))");
  await wait("!e('notes-prepare').disabled"); await action("e('notes-prepare').click()");
  await wait("!e('notes-review').hidden&&!e('notes-consent').disabled&&e('notes-approve').disabled");
  console.log('SERVICE EXIT UI: note prepared, consent not given');
  check(fingerprint() === before);
  if (pendingPrompt) {
    stage = 'pending real native reload confirmation'; menu('Recover View…').click();
    await until(() => !!reloadPrompt); check(!reloadPrompt.aborted);
  }
  workers = ownedChildren(); check(workers.length > 0);
  stage = 'actual Rust service SIGKILL';
  console.log('SERVICE EXIT UI: ' + stage);
  check(service.child.kill('SIGKILL'));
  await until(() => service.phase === 'ended', 15000);
  const result = await service.finished; check(result.code === null && result.signal === 'SIGKILL');
  stage = 'persistent terminal display';
  await until(() => !window.isDestroyed() && window.webContents.getURL().startsWith('data:text/html'));
  await wait("!e('viewport')&&!e('status')&&document.body.textContent.includes('The Rust session ended with an error.')&&document.body.textContent.includes('Close this window to exit.')");
  if (pendingPrompt) await until(() => reloadCancelled);
  stage = 'owned workers exited';
  await until(() => workers.every(pid => { try { process.kill(pid, 0); return false; } catch (e) { if (e.code === 'ESRCH') return true; throw e; } }), 15000);
  stage = 'no native recovery after service exit';
  // MenuItem wraps click and discards its return value. Check the owned page
  // and modal/service identities, not an invented menu return-value contract.
  menu('Recover View…').click();
  await new Promise(resolve => setImmediate(resolve));
  check(window.webContents.getURL().startsWith('data:text/html') && (!reloadPrompt || reloadPrompt.aborted));
  check(serviceCount === 1 && fingerprint() === before);
  stage = 'terminal screenshot';
  const png = await window.webContents.capturePage(); check(!png.isEmpty());
  // Outside the input directory so its unchanged inventory remains authoritative.
  const artifact = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-service-exit-image-'));
  fs.writeFileSync(path.join(artifact, 'terminal.png'), png.toPNG(), { mode: 0o600, flag: 'wx' });
  console.log('SERVICE EXIT UI: terminal screenshot ' + artifact + '/terminal.png');
  stage = 'explicit close and cleanup'; complete = true;
  if (signalExit) process.kill(process.pid, 'SIGTERM'); // actual OS signal to this QA-owned host only
  else if (pendingPrompt) app.quit(); else window.close();
}
app.whenReady().then(run).catch(async () => {
  failed = true;
  console.log('SERVICE EXIT UI: failed stage ' + stage);
  if (service && service.phase !== 'ended') {
    service.close();
    await Promise.race([service.finished, new Promise(resolve => setTimeout(resolve, 15000))]);
    if (service.phase !== 'ended') service.child.kill('SIGKILL');
  }
  realExit(1);
});
