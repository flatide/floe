'use strict';
// Explicit synthetic source only. Native confirmation choices are injected;
// browser reload, cookie/sessionStorage retention, and renderer crash are real.
const { createHash } = require('node:crypto');
async function run({ window, evalOwned, wait, recover, recovery, selectConfirm, viewFailed, crashes, counts, service, stage,
  mode = 'normal', eraseCookie, requestClose, forcePrompts }) {
  const web = window.webContents, key = 'floe.electron.recovery.qa';
  const fixed = JSON.stringify(key);
  const snapshot = () => ({ ...counts() });
  async function until(predicate, ms = 30000) {
    const end = Date.now() + ms;
    while (Date.now() < end) { if (predicate()) return; await new Promise(resolve => setTimeout(resolve, 20)); }
    throw new Error('Synthetic native recovery state timed out');
  }
  const frame = "(()=>{const e=id=>document.getElementById(id);return !document.hidden&&!e('logout').disabled&&!e('fit').disabled&&/^Live.*margin crop/.test(e('status').textContent)&&!e('margin-canvas').hidden&&!!e('margin-canvas').dataset.frameId&&!/Prefetching/.test(e('margin-info').textContent);})()";
  async function geometry() {
    await wait(frame);
    await evalOwned('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>resolve(true))))');
    const rect = await evalOwned("(()=>{const r=document.getElementById('viewport').getBoundingClientRect();return {x:Math.ceil(r.x),y:Math.ceil(r.y),width:Math.floor(r.width)-1,height:Math.floor(r.height)-1};})()");
    const image = await web.capturePage(rect, { stayHidden: true, stayAwake: false });
    if (image.isEmpty()) throw new Error('Missing synthetic recovery pixels');
    return createHash('sha256').update(image.toBitmap()).digest('hex');
  }
  async function accepted(expected = 'ready') {
    const before = snapshot(); selectConfirm(true);
    if (!await recover()) throw new Error('Explicit synthetic recovery did not start');
    await until(() => !recovery.busy, 35000);
    if (recovery.lastResult !== expected || counts().root !== before.root + 1 || counts().exchange !== before.exchange || counts().mutations !== before.mutations) {
      console.log('ELECTRON RECOVERY: result flags ' + JSON.stringify({ result: recovery.lastResult, expected, before, after: snapshot() }));
      throw new Error('Synthetic recovery replayed or failed');
    }
  }
  window.show(); window.focus(); web.focus();
  stage('recovery initial frame'); const initialHash = await geometry();
  if (mode === 'storage' || mode === 'cookie') {
    stage('recovery erase synthetic ' + mode);
    if (mode === 'storage') await evalOwned("sessionStorage.removeItem('floe-session:'+location.origin);true");
    else await eraseCookie(); // exact derived name in this NEW session, never enumerate/read cookie values
    stage('recovery detect missing ' + mode); await accepted('restart-required');
    await wait("document.getElementById('connection').getAttribute('data-session-state')==='restart-required'&&document.getElementById('logout').disabled");
    const beforeClose = snapshot(); await requestClose(); // default injected native Cancel
    if (forcePrompts() !== 1 || service.child.exitCode !== null || JSON.stringify(beforeClose) !== JSON.stringify(snapshot())) throw new Error('Lost-auth close bypassed cancellation');
    console.log('ELECTRON RECOVERY: OK (real synthetic ' + mode + ' loss; restart-required; no bootstrap/mutation replay; injected native Cancel preserves Rust; explicit End follows)');
    return;
  }
  const arm = "(()=>{if(sessionStorage.getItem(" + fixed + ")!==null)return false;sessionStorage.setItem(" + fixed + ",'kept');window.__floeElectronRecoveryQa=true;const x=document.getElementById('goto-x');x.value='901.234';x.dispatchEvent(new Event('input'));return true;})()";
  if (await evalOwned(arm) !== true) throw new Error('Synthetic recovery marker conflict');
  const initialRequests = snapshot(); selectConfirm(false);
  if (await recover() !== false || JSON.stringify(initialRequests) !== JSON.stringify(snapshot()) ||
      await evalOwned("window.__floeElectronRecoveryQa===true&&document.getElementById('goto-x').value==='901.234'") !== true) throw new Error('Cancelled recovery lost draft');
  stage('recovery explicit GET'); await accepted();
  if (await evalOwned("window.__floeElectronRecoveryQa===undefined&&sessionStorage.getItem(" + fixed + ")==='kept'&&document.getElementById('goto-x').value!=='901.234'") !== true || await geometry() !== initialHash) throw new Error('Reload lost storage or displayed geometry');

  stage('recovery nonresponding probe');
  const probe = recovery.probe; let probes = 0;
  recovery.probe = () => { probes++; return evalOwned('new Promise(()=>{})'); };
  const start = performance.now();
  try { await accepted('timeout'); } finally { recovery.probe = probe; }
  if (performance.now() - start < 30000 || probes !== 1) throw new Error('Recovery deadline was extended or bypassed');
  stage('recovery retry after timeout'); await accepted();
  if (await geometry() !== initialHash) throw new Error('Retry changed synthetic geometry');

  stage('recovery actual Chromium process crash');
  const oldPid = web.getOSProcessId(), beforeCrash = snapshot(), crashCount = crashes();
  if (!oldPid || service.child.exitCode !== null) throw new Error('Synthetic process not live');
  web.forcefullyCrashRenderer();
  await until(() => crashes() === crashCount + 1 && viewFailed() && web.getURL().startsWith('data:text/html;'));
  if (counts().root !== beforeCrash.root || counts().exchange !== beforeCrash.exchange || counts().mutations !== beforeCrash.mutations || service.child.exitCode !== null) throw new Error('Crash automatically reloaded or ended Rust');
  selectConfirm(false);
  if (await recover() !== false || counts().root !== beforeCrash.root) throw new Error('Crash recovery cancellation navigated');
  stage('recovery explicit reload after crash'); await accepted();
  if (viewFailed() || web.getOSProcessId() === oldPid || await geometry() !== initialHash ||
      await evalOwned("sessionStorage.getItem(" + fixed + ")==='kept'") !== true) throw new Error('Crash recovery did not restore the owned session/frame');
  await evalOwned('sessionStorage.removeItem(' + fixed + ');true');
  console.log('ELECTRON RECOVERY: OK (injected confirm cancel/approve; real root GET; fixed 30s lost-probe deadline; explicit retry; actual Chromium crash; new PID; same landed pixels/storage; no bootstrap/mutation replay; palette POST reads counted separately)');
}
module.exports = { run };
