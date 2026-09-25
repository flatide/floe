'use strict';
// Run with the pinned Electron executable, no arguments. Fresh synthetic inputs
// only. The shipped main/guards/service/recovery/UI are loaded unchanged.
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { spawnSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { app, BrowserWindow, Menu, dialog, session } = require('electron');
const { ReviewIntercept } = require('./electron-review-intercept.cjs');
if (process.argv.length !== 2) throw Error('Synthetic QA accepts no input paths or options');
const repo = path.resolve(__dirname, '..');
const pin = require(path.join(repo, 'electron/runtime.json'));
if (process.versions.electron !== pin.version) throw Error('Pinned Electron required');
app.enableSandbox();
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-review-'));
fs.chmodSync(root, 0o700);
console.log('ELECTRON REVIEW: synthetic artifacts ' + root);
require('./electron-review-fixture.cjs').writeInputs(root);
for (const [key, name] of [['FLOE_INDEX_BIN', 'floe-index'], ['FLOE_RENDERD_BIN', 'floe-renderd']]) {
  if (!Object.hasOwn(process.env, key)) process.env[key] = path.join(repo, 'rust/target/release', name);
}
function index(args) {
  const result = spawnSync(process.env.FLOE_INDEX_BIN, args, { stdio: 'ignore', timeout: 120000 });
  if (result.status !== 0) throw Error('Synthetic index failed');
}
index(['vfs', path.join(root, 'synthetic.oas'), path.join(root, '.synthetic.oas.ice'), '--jobs', '2']);
index(['drc', path.join(root, 'synthetic.db'), '--jobs', '2']);
function fingerprint() {
  const out = {};
  function walk(dir, prefix) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a,b) => a.name.localeCompare(b.name))) {
      const full = path.join(dir, entry.name), name = prefix + entry.name;
      if (entry.isDirectory()) walk(full, name + '/');
      else if (entry.isFile()) out[name] = createHash('sha256').update(fs.readFileSync(full)).digest('hex');
      else throw Error('Unexpected synthetic filesystem entry');
    }
  }
  walk(root, ''); return out;
}
const before = fingerprint();
let service, window, origin = null, stage = 'startup', complete = false, failed = false, crashes = 0;
let accepted = false, prompts = 0;
const wire = new ReviewIntercept(() => origin, () => window?.webContents.id);
const repairWire = new ReviewIntercept(() => origin, () => window?.webContents.id, '/recovery');
const actualExit = app.exit.bind(app);
app.exit = code => {
  wire.release();repairWire.release();
  const ok = code === 0 && complete && !failed && service?.phase === 'ended' && !wire.failed && !repairWire.failed && crashes === 4;
  console.log(ok ? 'ELECTRON REVIEW: OK (four actual Chromium crashes; save + file-repair approvals not replayed; explicit resolves keep receipt #1; exact link cleanup; picker reopen/reconnect; read-back; Rust joined)' : 'ELECTRON REVIEW: FAIL stage ' + stage);
  actualExit(ok ? 0 : 1);
};
// Observe the real inherited-pipe service. No credential is saved/logged.
const client = require(path.join(repo, 'electron/service-client.cjs'));
const ActualClient = client.ServiceClient;
client.ServiceClient = class extends ActualClient {
  constructor(...args) {
    super(...args); if (service) throw Error('Unexpected service'); service = this;
    this.on('ready', value => { origin = value.origin; });
  }
};
// Electron only keeps the last webRequest listener. Wrap registrations to retain
// the original guard's decision; do not install a competing permissive handler.
const partition = session.fromPartition.bind(session);
session.fromPartition = (...args) => {
  const ses = partition(...args);
  for (const [name, observer] of [['onBeforeRequest', 'before'], ['onHeadersReceived', 'headers']]) {
    const register = ses.webRequest[name].bind(ses.webRequest);
    ses.webRequest[name] = listener => register((d, cb) => listener(d, response => wire[observer](d, response, r => repairWire[observer](d, r, cb))));
  }
  return ses;
};
// Same native confirmation choice injection as other Electron QA. No physical
// dialog/keyboard acceptance is claimed. Only Recover View is automatically chosen.
dialog.showMessageBox = async (_window, options) => {
  if (options.message.startsWith('Reload the current view?')) {
    if (options.defaultId !== 0 || options.cancelId !== 0 || options.buttons.join('|') !== 'Cancel|Reload View') throw Error('Recovery confirmation changed');
    prompts++; return { response: accepted ? 1 : 0 };
  }
  failed = true; return { response: 0 };
};
process.argv = [process.argv[0], path.join(repo, 'electron'), 'view', '--multi', path.join(root, 'synthetic.oas'),
  '--drc', path.join(root, '.synthetic.db.tray'), '--drc-reviewer', 'native-recovery-test', '--drc-edit-waives',
  '--jobs', '2', '--raster-jobs', '2', '--budget-mb', '256', '--refinement', 'off'];
require(path.join(repo, 'electron/main.cjs'));

async function until(fn) {
  let timer;
  const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(Error('QA stage timeout')), 30000); });
  let expired = false;
  async function poll() {
    while (!expired && !failed && !wire.failed && !repairWire.failed && service?.phase !== 'ended') {
      if (await fn()) return;
      await new Promise(resolve => setTimeout(resolve, 30));
    }
    throw Error('Synthetic state failed');
  }
  try { await Promise.race([poll(), timeout]); } finally { expired = true; clearTimeout(timer); }
}
async function evaluate(script) {
  if (!origin || window.isDestroyed() || window.webContents.getURL() !== origin + '/') return null;
  return window.webContents.executeJavaScript(script, false);
}
const e = 'const e=id=>document.getElementById(id);';
const wait = script => until(async () => await evaluate('(()=>{' + e + 'return ' + script + ';})()') === true);
const action = script => evaluate('(()=>{' + e + script + ';return true;})()');
async function ready(withList = false) {
  await require(path.join(repo, 'electron/readiness-qa.cjs')).waitReady({ evaluate,
    alive: () => !failed && service.phase !== 'ended', observe: () => {} });
  await wait("e('notes-owner').textContent==='Reviewer: native-recovery-test'&&e('waives-owner').textContent==='Reviewer: native-recovery-test'");
  // An unresolved waive intentionally pauses error metadata. Resolve its saved
  // approval before asking for the refreshed list; do not bypass that barrier.
  if (withList) await wait("e('drc-errors').children.length===2&&!e('drc-errors').children[0].disabled");
}
function recoverMenu() {
  const item = Menu.getApplicationMenu().items.find(x => x.label === 'View')?.submenu.items.find(x => x.label === 'Recover View…');
  if (!item) throw Error('Recovery menu missing');
  item.click();
}
async function fileRecovery(kind) {
  const gap = require('./electron-review-gap.cjs').createGap(root,kind);
  stage = kind + ' file-repair preview';
  await wait("!e('recovery-panel').hidden&&!e('recovery-kind').disabled&&!e('recovery-prepare').disabled");
  await action("e('recovery-panel').open=true;e('recovery-kind').value='"+kind+"';e('recovery-kind').dispatchEvent(new Event('change',{bubbles:true}))");
  await wait("!e('recovery-prepare').disabled");
  await action("e('recovery-prepare').click()");
  await wait("!e('recovery-preview').hidden&&e('recovery-approve').disabled");
  if (await evaluate('document.getElementById("recovery-target").textContent') !== gap.leaf+' · '+gap.bytes+' bytes · reviewer native-recovery-test') throw Error('Wrong recovery target preview');
  gap.verify(2);if(repairWire.counts[kind]!==0)throw Error('Preview approved recovery');
  await action("e('recovery-discard').click()");
  await wait("e('recovery-preview').hidden&&!e('recovery-prepare').disabled");gap.verify(2);
  await action("e('recovery-prepare').click()");
  await wait("!e('recovery-preview').hidden&&e('recovery-approve').disabled");
  repairWire.arm(kind);
  await action("e('recovery-consent').click();e('recovery-approve').click()");
  stage = kind + ' file-repair accepted response held';
  await until(()=>repairWire.held?.kind===kind);
  // A known completed on-disk boundary, NOT an unlink syscall fault or NFS kill.
  await until(()=>{try{gap.verify(1);return true;}catch(_){return false;}});
  const counts=repairWire.snapshot(),pid=window.webContents.getOSProcessId(),oldCrashes=crashes;
  stage = kind + ' file-repair Chromium crash';
  window.webContents.forcefullyCrashRenderer();
  await until(()=>crashes===oldCrashes+1&&window.webContents.getURL().startsWith('data:text/html;'));
  repairWire.release();gap.verify(1);
  const p=prompts;accepted=false;recoverMenu();await until(()=>prompts===p+1);
  await new Promise(resolve=>setImmediate(resolve));
  if(repairWire.counts[kind]!==1)throw Error('Cancelled reload replayed file repair');
  accepted=true;recoverMenu();await until(()=>prompts===p+2);await ready();
  if(window.webContents.getOSProcessId()===pid||repairWire.counts[kind]!==1||repairWire.counts.exchanges!==counts.exchanges)throw Error('Native recovery replayed file repair');
  stage = kind + ' file-repair same approval resolve';
  await wait("!e('recovery-resolve').hidden&&!e('recovery-resolve').disabled&&e('recovery-kind').value==='"+kind+"'&&!e('notes-autosave').checked&&!e('waives-autosave').checked");
  gap.verify(1);
  await action("e('recovery-resolve').click()");
  await wait("e('recovery-resolve').hidden&&/^Recovery #1: succeeded/.test(e('recovery-status').textContent)");
  if(repairWire.counts[kind]!==2)throw Error('Wrong file repair resolve count');gap.verify(1);
  // The failed/retired reader must be replaced through the visible picker,
  // not a hidden API request or an implicit permission transfer.
  stage = kind + ' file-repair explicit DRC reopen';
  await wait("!e('drc-open').disabled");await action("e('drc-open').click()");
  await wait("!e('browse-dialog').hidden&&Array.from(e('browse-entries').querySelectorAll('button')).some(b=>!b.disabled&&b.textContent.startsWith('synthetic.db  ·'))");
  await action("Array.from(e('browse-entries').querySelectorAll('button')).find(b=>b.textContent.startsWith('synthetic.db  ·')).click();e('browse-select').click()");
  await wait("e('browse-dialog').hidden&&!e('drc-reconnect').hidden&&!e('drc-reconnect').disabled");
  await action("e('drc-reconnect').click()");
  await wait("!e('browse-dialog').hidden&&!e('browse-review-label').hidden&&!e('browse-review-consent').checked&&e('browse-select').disabled");
  await action("e('browse-review-consent').click();e('browse-select').click()");
  await wait("e('browse-dialog').hidden&&!e('notes-autosave').checked&&!e('waives-autosave').checked");
  await ready(true);gap.verify(1);
  await wait("/^Earlier recovery #1: succeeded/.test(e('recovery-status').textContent)&&!e('recovery-status').textContent.includes('Open DRC')");
  stage = kind + ' file-repair fresh read-back';
  await action("e('drc-errors').children[0].click()");
  await wait("!e('"+kind+"-read').disabled&&/^Saved notes · native-recovery-test · revision/.test(e('drc-notes-status').textContent)");
  await action("e('"+kind+"-read').click()");
  await wait(kind==='notes'?"!e('notes-editor').hidden&&!e('notes-text').disabled&&e('notes-text').value==='Synthetic native recovery — 한글'":
    "!e('waives-editor').hidden&&!e('waives-action').disabled&&e('waives-target').textContent.includes('1 already waived · 0 reserved statuses')");
  await action("e('"+kind+"-discard').click()");await wait("e('"+kind+"-editor').hidden");gap.verify(1);
  if(wire.counts.notes!==2||wire.counts.waives!==2)throw Error('Repair rewrote review payload');
  console.log('ELECTRON FILE RECOVERY: '+kind+' preview/discard -> approve -> Chromium crash -> explicit resolve #1 -> picker reopen/reconnect -> read-back; POST 1 -> 1 -> 2');
}
async function run() {
  await until(() => { window = BrowserWindow.getAllWindows()[0]; return window && origin; });
  window.webContents.on('render-process-gone', () => { crashes++; });
  window.show(); window.focus(); window.webContents.focus();
  await ready(true);
  const prefs = window.webContents.getLastWebPreferences();
  if (!prefs.sandbox || !prefs.contextIsolation || prefs.nodeIntegration || prefs.preload || window.webContents.session.isPersistent()) throw Error('Sandbox changed');
  for (const kind of ['notes', 'waives']) {
    stage = kind + ' preview'; console.log('ELECTRON REVIEW: ' + stage);
    await wait("e('drc-errors').children.length===2&&!e('drc-errors').children[0].disabled&&e('drc-errors').children[0].getAttribute('aria-label')==='Error 1, global 1'");
    await action("e('drc-errors').children[0].click()");
    stage = kind + ' read available';
    await wait("!e('" + kind + "-read').disabled");
    // A selected-row badge read uses the same bounded review resources. Wait
    // for its visible completion, not a sleep or a retry after a rejected read.
    await wait("/^Saved notes · native-recovery-test · revision/.test(e('drc-notes-status').textContent)");
    await action("e('" + kind + "-read').click()");
    stage = kind + ' editor ready';
    await wait("!e('" + kind + "-editor').hidden&&!e('" + (kind === 'notes' ? 'notes-text' : 'waives-action') + "').disabled");
    await action(kind === 'notes' ? "e('notes-text').value='Synthetic native recovery — 한글';e('notes-text').dispatchEvent(new Event('input',{bubbles:true}))" :
      "e('waives-action').value='waive';e('waives-action').dispatchEvent(new Event('change',{bubbles:true}))");
    stage = kind + ' prepare available';
    await wait("!e('" + kind + "-prepare').disabled");
    await action("e('" + kind + "-prepare').click()");
    stage = kind + ' consent available';
    await wait("!e('" + kind + "-review').hidden&&!e('" + kind + "-consent').disabled");
    if (kind === 'notes') await wait("e('notes-preview').textContent==='Synthetic native recovery — 한글'");
    else await wait("/^Waive 1 selected errors/.test(e('waives-preview').textContent)");
    wire.arm(kind);
    await action("e('" + kind + "-consent').click();e('" + kind + "-approve').click()");
    stage = kind + ' accepted response held';
    await until(() => wire.held?.kind === kind);
    await wait("e('" + kind + "-approve').disabled");
    const counts = wire.snapshot(), pid = window.webContents.getOSProcessId(), oldCrashes = crashes;
    if (counts[kind] !== 1 || !pid || service.phase !== 'ready') throw Error('Save not pending');
    stage = kind + ' actual Chromium crash';
    window.webContents.forcefullyCrashRenderer();
    await until(() => crashes === oldCrashes + 1 && window.webContents.getURL().startsWith('data:text/html;'));
    wire.release();
    if (JSON.stringify(wire.snapshot()) !== JSON.stringify(counts) || service.phase !== 'ready') throw Error('Crash replayed or ended Rust');
    accepted = false; const p = prompts; recoverMenu();
    await until(() => prompts === p + 1);
    await new Promise(resolve => setImmediate(resolve)); // settle cancelled confirmation
    if (JSON.stringify(wire.snapshot()) !== JSON.stringify(counts)) throw Error('Cancel navigated');
    stage = kind + ' explicit recovery'; accepted = true; recoverMenu();
    await until(() => prompts === p + 2);
    stage = kind + ' recovered UI ready';
    await ready();
    stage = kind + ' recovered PID and request counts';
    if (window.webContents.getOSProcessId() === pid || wire.counts.roots !== counts.roots + 1 ||
        wire.counts.exchanges !== counts.exchanges || wire.counts.notes !== counts.notes || wire.counts.waives !== counts.waives) throw Error('Recovery replayed approved save');
    stage = kind + ' recovered pending journal';
    await wait("!e('" + kind + "-uncertain').hidden&&!e('" + kind + "-resolve').disabled&&!e('notes-autosave').checked&&!e('waives-autosave').checked");
    stage = kind + ' resolve original approval';
    await action("e('" + kind + "-resolve').click()");
    await wait("e('" + kind + "-uncertain').hidden&&/" + (kind === 'notes' ? 'Saved' : 'Save completed') + " · #1\\b/.test(e('" + kind + "-status').textContent)");
    if (wire.counts[kind] !== 2) throw Error('Wrong explicit resolve count');
    console.log('ELECTRON REVIEW: ' + kind + ' crash/recover/resolve #1; POST 1 -> 1 -> 2');
  }
  stage = 'UI read-back';
  await wait("e('drc-errors').children.length===2&&!e('drc-errors').children[0].disabled");
  await action("e('drc-errors').children[0].click()");
  for (const kind of ['notes', 'waives']) {
    await wait("!e('" + kind + "-read').disabled&&/^Saved notes · native-recovery-test · revision/.test(e('drc-notes-status').textContent)");
    await action("e('" + kind + "-read').click()");
    await wait("!e('" + kind + "-editor').hidden");
    await wait(kind === 'notes' ? "!e('notes-text').disabled&&e('notes-text').value==='Synthetic native recovery — 한글'" :
      "!e('waives-action').disabled&&e('waives-target').textContent.includes('1 already waived · 0 reserved statuses')");
    await action("e('" + kind + "-discard').click()");
    await wait("e('" + kind + "-editor').hidden");
  }
  if (wire.counts.notes !== 2 || wire.counts.waives !== 2) throw Error('Unexpected extra save');
  stage = 'disk read-back';
  const after = fingerprint();
  for (const [name, digest] of Object.entries(before)) if (after[name] !== digest) throw Error('Synthetic input changed');
  const note = path.join(root, '.synthetic.db.notes.native-recovery-test.fe');
  const waive = path.join(root, '.synthetic.db.waive.native-recovery-test');
  for (const file of [note, waive]) if (!fs.lstatSync(file).isFile() || (fs.statSync(file).mode & 0o777) !== 0o600) throw Error('Unsafe sidecar');
  const records = fs.readFileSync(note, 'utf8').split('\n').filter(line => line.startsWith('floe_note='));
  if (JSON.stringify(records) !== JSON.stringify(['floe_note=0|Synthetic native recovery — 한글'])) throw Error('Note differs');
  // Independent on-disk check of the fixed 2-error/1-rule fixture, not cached
  // reader metadata. See floe/drc.py _WAIVE_HEADER (40 bytes), then status/count.
  const bytes = fs.readFileSync(waive);
  if (bytes.length !== 46 || bytes.subarray(0,8).toString('ascii') !== 'FLOEWAIV' || bytes.readUInt32LE(8) !== 1 ||
      bytes.readBigUInt64LE(12) !== BigInt(fs.statSync(path.join(root,'synthetic.db')).size) ||
      bytes.readBigUInt64LE(28) !== 2n || bytes.readUInt32LE(36) !== 1 || bytes[40] !== 1 || bytes[41] !== 0 ||
      bytes.readUInt32LE(42) !== 1) throw Error('Waive disk read-back differs');
  const additions = [note, waive, note+'.lock', waive+'.lock'].map(file => path.basename(file)).sort();
  if (JSON.stringify(Object.keys(after).filter(name => !Object.hasOwn(before,name)).sort()) !== JSON.stringify(additions)) throw Error('Unexpected synthetic output');
  stage = 'marked publication gap fixture';
  const decoy = path.join(root,'.floe-review-000-000.tmp');fs.writeFileSync(decoy,'unrelated staging name; preserve',{flag:'wx',mode:0o600});
  for(const kind of ['notes','waives'])await fileRecovery(kind);
  if(fs.readFileSync(decoy,'utf8')!=='unrelated staging name; preserve')throw Error('Recovery scanned unrelated file');
  const repaired=fingerprint();for(const [name,digest] of Object.entries(after))if(repaired[name]!==digest)throw Error('Recovery modified existing data');
  if(JSON.stringify(Object.keys(repaired).filter(name=>!Object.hasOwn(after,name)))!==JSON.stringify([path.basename(decoy)]))throw Error('Unexpected repair output');
  stage = 'file recovery panel mounted';
  await wait("!e('recovery-panel').hidden&&!e('recovery-prepare').disabled&&e('recovery-preview').hidden&&e('recovery-check').hidden");
  await action("e('recovery-panel').open=true;e('recovery-panel').querySelector('summary').focus({preventScroll:true});e('recovery-panel').scrollIntoView({block:'start',behavior:'instant'})");
  await wait("e('recovery-panel').open&&e('recovery-panel').getBoundingClientRect().top>=0&&e('recovery-prepare').getBoundingClientRect().bottom<innerHeight");
  await evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>resolve(true))))');
  stage = 'file recovery compositor pixels';
  // Neither a nonempty screenshot nor a rescaled preview proves that geometry
  // landed. This fixed synthetic view has a large green rectangle; UI icons
  // alone cannot satisfy the threshold. Poll capture only, never force redraw
  // or mutate the viewport to make a missing frame pass.
  let screenshot, captures = 0;
  await until(async () => {
    const candidate = await window.webContents.capturePage();captures++;
    if(candidate.isEmpty())return false;
    const rgba = candidate.toBitmap();let green = 0;
    for(let i=0;i<rgba.length;i+=4)if(rgba[i+1]>40&&rgba[i+1]>rgba[i]+20&&rgba[i+1]>rgba[i+2]+20)green++;
    if(green<rgba.length/4/20)return false;
    screenshot=candidate;return true;
  });
  console.log('ELECTRON REVIEW: compositor geometry confirmed in '+captures+' capture(s)');
  fs.writeFileSync(path.join(root,'recovery-panel.png'), screenshot.toPNG(), {flag:'wx',mode:0o600});
  console.log('ELECTRON REVIEW: panel screenshot ' + path.join(root,'recovery-panel.png'));
  stage = 'close cancel confirm join';
  app.quit();
  await wait("!e('session-exit-dialog').hidden&&document.activeElement.id==='session-exit-cancel'");
  await action("e('session-exit-cancel').click()");
  await wait("e('session-exit-dialog').hidden&&!e('logout').disabled");
  if (service.phase !== 'ready') throw Error('Cancel ended service');
  app.quit(); await wait("!e('session-exit-dialog').hidden");
  complete = true; await action("e('session-exit-confirm').click()");
}
app.whenReady().then(run).catch(async () => {
  failed = true; wire.release();repairWire.release();
  console.log('ELECTRON REVIEW: failed stage ' + stage);
  console.log('ELECTRON REVIEW: host flags ' + JSON.stringify({counts:wire.snapshot(),repairs:repairWire.snapshot(),readStatus:wire.readStatus,prompts,crashes,service:service?.phase,wireFailed:wire.failed,repairFailed:repairWire.failed}));
  let timer;
  const flags = await Promise.race([evaluate("(()=>{const e=id=>document.getElementById(id),out={visible:!document.hidden};for(const kind of ['notes','waives']){for(const suffix of ['read','text','action','prepare','approve','resolve']){const n=e(kind+'-'+suffix);out[kind+'_'+suffix]=!!n&&!n.disabled;}for(const suffix of ['editor','review','uncertain']){const n=e(kind+'-'+suffix);out[kind+'_'+suffix]=!!n&&!n.hidden;}out[kind+'_invalid']=/changed|expired/i.test(e(kind+'-message').textContent);}out.live=/^Live/.test(e('status').textContent);return out;})()").catch(()=>null),new Promise(resolve=>{timer=setTimeout(()=>resolve(null),1000);})]);
  clearTimeout(timer); console.log('ELECTRON REVIEW: UI flags ' + JSON.stringify(flags));
  if (service && service.phase !== 'ended') {
    service.once('ended', () => app.exit(1));
    service.close();
  } else app.exit(1);
});
