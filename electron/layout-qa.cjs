'use strict';
// Explicit synthetic UI QA only. No credentials/paths/notes are included in metrics.
const fs = require('node:fs'), path = require('node:path'), os = require('node:os');
const { createHash } = require('node:crypto');
const { execFileSync } = require('node:child_process');
const { roundEven } = require('../rust/web/ui/protocol.js');

function rustMemory(pid) {
  // Read only numeric process metadata; retain only the owned service descendants.
  return ownedMemory(execFileSync('/bin/ps', ['-e', '-o', 'pid=,ppid=,rss='], { encoding: 'utf8' }), pid);
}
function ownedMemory(text, pid) {
  const rows = text.trim().split('\n').map(s => s.trim().split(/\s+/).map(Number));
  if (!Number.isSafeInteger(pid) || pid < 1 || !rows.every(r => r.length === 3 &&
      r.every(n => Number.isSafeInteger(n) && n >= 0))) throw new Error('Invalid process metrics');
  const selected = new Set([pid]);
  for (let changed = true; changed;) {
    changed = false;
    for (const [child, parent] of rows) if (selected.has(parent) && !selected.has(child)) { selected.add(child); changed = true; }
  }
  const owned = rows.filter(r => selected.has(r[0]));
  if (!owned.some(r => r[0] === pid)) throw new Error('Rust service missing');
  return { process_count: owned.length, rss_sum_kib: owned.reduce((n, r) => n + r[2], 0) };
}

async function run({ app, window, evalOwned, service }) {
  const web = window.webContents;
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-layout-'));
  fs.chmodSync(root, 0o700);
  console.log('ELECTRON LAYOUT: synthetic artifacts ' + root);
  const stateScript = `(()=>{
    const e=id=>document.getElementById(id),v=e('viewport'),r=v.getBoundingClientRect();
    const c=e('canvas'),m=e('margin-canvas');
    return {visible:!document.hidden,ready:!e('logout').disabled&&!e('fit').disabled&&e('empty').hidden&&
      e('rendering').hidden&&/^Live.*margin crop/.test(e('status').textContent)&&!m.hidden&&!!m.dataset.frameId&&
      !/Prefetching/.test(e('margin-info').textContent),
      x:Number(e('goto-x').value),y:Number(e('goto-y').value),width:Number(e('goto-width').value),
      detail:e('detail').value,depth:e('depth').value,dpr:devicePixelRatio,
      pixels:[Math.floor(r.right*devicePixelRatio)-Math.ceil(r.left*devicePixelRatio),
        Math.floor(r.bottom*devicePixelRatio)-Math.ceil(r.top*devicePixelRatio)],
      rect:{x:Math.ceil(r.x),y:Math.ceil(r.y),width:Math.floor(r.width)-1,height:Math.floor(r.height)-1}};
  })()`;
  async function settled(predicate = () => true) {
    const end = Date.now() + 30000;
    while (Date.now() < end) {
      const state = await evalOwned(stateScript);
      if (state && state.visible && state.ready && predicate(state)) {
        await evalOwned('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>resolve(true))))');
        const again = await evalOwned(stateScript);
        if (again.visible && again.ready && predicate(again)) return again;
      }
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    console.log('ELECTRON LAYOUT: timeout flags ' + JSON.stringify(await evalOwned(`(()=>{
      const e=id=>document.getElementById(id);return {visible:!document.hidden,
      live:/^Live/.test(e('status').textContent),fit:!e('fit').disabled,empty:e('empty').hidden,
      rendering:e('rendering').hidden,frame:!!(e('canvas').dataset.frameId||e('margin-canvas').dataset.frameId),
      renderer_failed:/renderer failed/i.test(e('notice').textContent),margin:!e('margin-canvas').hidden,
      margin_crop:/^Live.*margin crop/.test(e('status').textContent)};})()`)));
    throw new Error('Synthetic layout did not settle');
  }
  async function capture(name, state) {
    if (await evalOwned('!document.hidden&&!location.hash') !== true) throw new Error('Hidden synthetic layout');
    const image = await web.capturePage(state.rect, { stayHidden: true, stayAwake: false });
    if (image.isEmpty()) throw new Error('Empty synthetic layout');
    const bytes = image.toPNG();
    fs.writeFileSync(path.join(root, name + '.png'), bytes, { flag: 'wx', mode: 0o600 });
    const bitmap = image.toBitmap();
    return { hash: createHash('sha256').update(bitmap).digest('hex'), bitmap };
  }
  window.show(); window.focus(); web.focus();
  console.log('ELECTRON LAYOUT: waiting for initial frame');
  await settled(s => s.detail === 'high' && s.width === 300);
  await evalOwned("document.getElementById('viewport').focus();true");
  const initial = await settled();
  const initialHash = await capture('initial', initial);
  console.log('ELECTRON LAYOUT: initial geometry captured');
  // Prime CPU interval; these Chromium-only averages are not Rust CPU metrics.
  app.getAppMetrics();
  const measurements = [];
  // Keyboard pan intentionally snaps to a 16-device-pixel phase. Do not demand
  // an unsnapped 30/150 um shift from a correctly snapped 10%/50% command.
  const step = fraction => roundEven(fraction * initial.pixels[0] / 16) * 16 / initial.pixels[0] * initial.width;
  for (const [name, key, shift, expected] of [
    ['right10', 'Right', true, initial.x + step(0.1)],
    ['return10', 'Left', true, initial.x],
    ['right50', 'Right', false, initial.x + step(0.5)],
    ['return50', 'Left', false, initial.x]
  ]) {
    console.log('ELECTRON LAYOUT: checking ' + name);
    const start = performance.now();
    const modifiers = shift ? ['shift'] : [];
    web.sendInputEvent({ type: 'keyDown', keyCode: key, modifiers });
    web.sendInputEvent({ type: 'keyUp', keyCode: key, modifiers });
    const state = await settled(s => Math.abs(s.x - expected) < 0.001 && s.width === initial.width);
    const domMs = performance.now() - start;
    const hash = await capture(name, state);
    if (hash.bitmap.length !== initialHash.bitmap.length) throw new Error('Synthetic capture size changed');
    if ((name.startsWith('right') && hash.hash === initialHash.hash) || (name.startsWith('return') && hash.hash !== initialHash.hash)) {
      let changed = 0;
      for (let i = 0; i < hash.bitmap.length; i += 4) {
        if (hash.bitmap.readUInt32LE(i) !== initialHash.bitmap.readUInt32LE(i)) changed++;
      }
      console.log('ELECTRON LAYOUT: return comparison ' + JSON.stringify({ action: name, changed_pixels: changed,
        initial_center: [initial.x, initial.y], current_center: [state.x, state.y], pixels: state.pixels }));
      throw new Error('Synthetic pan pixels did not change/restore');
    }
    measurements.push({ action: name, input_to_settled_dom_ms: Math.round(domMs * 10) / 10,
      restored_initial_pixels: hash.hash === initialHash.hash });
  }
  const metrics = app.getAppMetrics();
  const report = { schema: 1, host: 'electron', runtime: process.versions.electron, platform: process.platform,
    arch: process.arch, fixture: 'caller-selected synthetic; use the valmini driver', decode_jobs: 4, raster_jobs: 4,
    refinement: 'off', detail: initial.detail, depth: initial.depth, dpr: initial.dpr,
    viewport_css: initial.rect, viewport_pixels: initial.pixels, measurements,
    chromium: metrics.map(m => ({ type: m.type, working_set_kib: m.memory.workingSetSize,
      cpu_percent_since_prime: m.cpu.percentCPUUsage, sandboxed: m.sandboxed ?? null })),
    rust: rustMemory(service.child.pid),
    limitations: ['Settled landed margin plus two RAFs/polling, not input-to-photon or first response',
      'Native capture/readback adds workload between actions', 'Rust build profile depends on selected binaries', 'No WKWebView baseline yet',
      'RSS/working-set sums include shared pages; not unique memory or peak RSS', 'No RHEL/ETX or physical input acceptance'] };
  fs.writeFileSync(path.join(root, 'metrics.json'), JSON.stringify(report, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  console.log('ELECTRON LAYOUT: OK (landed geometry; injected 10%/50% pan; exact screenshot return)');
}
module.exports = { run, ownedMemory };
