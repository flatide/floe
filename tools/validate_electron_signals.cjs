'use strict';
// Fresh empty-root Electron sessions only. No designs, clipboard or writable reviews.
const { spawn, execFileSync } = require('node:child_process');
const path = require('node:path');
if (process.argv.length !== 2) throw Error('This test accepts no input/output paths');
const repo = path.resolve(__dirname, '..');
function tree(pid) {
  const rows = execFileSync('/bin/ps', ['-e', '-o', 'pid=,ppid='], { encoding: 'utf8' }).trim()
    .split('\n').map(s => s.trim().split(/\s+/).map(Number));
  const ids = new Set([pid]);
  for (let changed = true; changed;) {
    changed = false;
    for (const [child, parent] of rows) if (ids.has(parent) && !ids.has(child)) { ids.add(child); changed = true; }
  }
  return ids;
}
const alive = pid => { try { process.kill(pid, 0); return true; } catch (e) { if (e.code === 'ESRCH') return false; throw e; } };
const delay = ms => new Promise(r => setTimeout(r, ms));
async function run(signal, mode = 'idle') {
  const flag = { idle: '--smoke-signal-test', dialog: '--smoke-signal-dialog-test', error: '--smoke-signal-error-test' }[mode];
  const expectedExit = mode === 'error' ? 1 : 0;
  const child = spawn('sh', [path.join(repo, 'tools/run_electron_dev.sh'), flag], {
    cwd: repo, env: process.env, stdio: ['ignore', 'pipe', 'pipe'] });
  let ready = false, handled = false, ok = false, failed = false, closed = false, result, buffer = '';
  let dialogPending = false, dialogCancelled = false;
  const ended = new Promise(resolve => {
    child.once('error', () => { failed = true; });
    child.once('close', (code, sig) => { closed = true; result = { code, signal: sig }; resolve(); });
  });
  child.stderr.on('data', () => {}); // no raw native/credential-bearing diagnostics
  child.stdout.on('data', bytes => {
    buffer += bytes.toString('utf8');
    if (buffer.length > 65536) { failed = true; buffer = ''; return; }
    let at;
    while ((at = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, at); buffer = buffer.slice(at + 1);
      if (/^ELECTRON (SIGNAL|SMOKE):/.test(line)) console.log(line);
      if (line === 'ELECTRON SIGNAL: ready') ready = true;
      if (line === 'ELECTRON SIGNAL: handled ' + signal) handled = true;
      if (line === 'ELECTRON SIGNAL: dialog pending') dialogPending = true;
      if (line === 'ELECTRON SIGNAL: dialog cancelled') dialogCancelled = true;
      if (line.startsWith('ELECTRON SMOKE: OK (')) ok = true;
      if (line === 'ELECTRON SMOKE: FAIL') failed = true;
    }
  });
  let owned = new Set();
  try {
    const startup = performance.now() + 30000;
    while (!ready && !closed && !failed && performance.now() < startup) await delay(20);
    if (!ready || closed || failed) throw Error('Synthetic signal session did not become ready');
    owned = tree(child.pid);
    if (owned.size < 3) throw Error('Synthetic Rust/Chromium descendants not present');
    console.log('ELECTRON SIGNAL: send ' + signal);
    child.kill(signal);
    const deadline = performance.now() + 10000;
    while (!closed && performance.now() < deadline) await delay(20);
    if (!closed || !handled || (expectedExit === 0 ? failed || !ok : !failed || ok) ||
        result.code !== expectedExit || result.signal || (mode !== 'idle' && (!dialogPending || !dialogCancelled))) {
      throw Error('Real signal did not finish the normal cleanup path');
    }
    for (const pid of owned) if (alive(pid)) throw Error('Owned child remained after signal shutdown');
    console.log('ELECTRON SIGNAL: ' + signal + ' ' + mode + ' OK (handler, Rust join, descendants gone; exit ' + expectedExit + ')');
  } finally {
    // Failure cleanup is NOT success: kill only our directly spawned host; Rust
    // owns EOF cancellation and must still release all previously owned children.
    if (!closed) {
      for (const pid of tree(child.pid)) owned.add(pid);
      child.kill('SIGKILL');
    }
    await ended;
    const cleanup = performance.now() + 10000;
    while ([...owned].some(alive) && performance.now() < cleanup) await delay(50);
    if ([...owned].some(alive)) throw Error('Synthetic child cleanup not confirmed');
  }
}
(async () => {
  for (const signal of ['SIGTERM', 'SIGINT']) await run(signal);
  await run('SIGTERM', 'dialog');
  await run('SIGTERM', 'error');
  console.log('ELECTRON SIGNAL GATE: ALL OK');
})().catch(e => { console.error(e.message); process.exitCode = 1; });
