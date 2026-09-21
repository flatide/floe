'use strict';
// Development oracle generator may use Python/KLayout; the executed app does not.
// No input/output path arguments: all writes stay in a NEW synthetic directory.
// --frame-parity selects the geometry gate, not a user file or relaxed oracle.
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { spawnSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const parityOnly = process.argv.length===3 && process.argv[2]==='--frame-parity';
if (process.argv.length !== 2 && !parityOnly) throw new Error('Only --frame-parity is accepted; this test creates its own synthetic source');
const repo = path.resolve(__dirname, '..');
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-valmini-'));
fs.chmodSync(root, 0o700);
console.log('ELECTRON LAYOUT: generated fixture directory ' + root);
function run(binary, args, label, timeout, env = process.env) {
  const result = spawnSync(binary, args, { cwd: repo, env, encoding: 'utf8', timeout, maxBuffer: 8 * 1024 * 1024 });
  // Never echo raw renderer errors/URLs; only allow the fixed synthetic QA lines.
  const output = (result.stdout || '') + (result.stderr || '');
  for (const line of output.split('\n')) if (/^ELECTRON (SMOKE|LAYOUT|DOWNLOAD|RECOVERY):/.test(line)) console.log(line);
  if (result.status !== 0) throw new Error(label + ' failed; no existing design/cache modified');
}
function snapshot(directory, prefix = '') {
  const result = {};
  for (const entry of fs.readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    const name = path.join(prefix, entry.name), full = path.join(directory, entry.name);
    if (entry.isDirectory()) Object.assign(result, snapshot(full, name));
    else if (entry.isFile()) result[name] = createHash('sha256').update(fs.readFileSync(full)).digest('hex');
    else throw new Error('Unexpected link/special file in synthetic fixture');
  }
  return result;
}
const source = path.join(root, 'valmini.oas');
run('FLOE_QA_PYTHON_BIN' in process.env ? process.env.FLOE_QA_PYTHON_BIN : path.join(repo, '.venv/bin/python'),
  ['-B', path.join(repo, 'tools/gen_valmini.py'), source], 'Synthetic generator', 120000);
run('FLOE_INDEX_BIN' in process.env ? process.env.FLOE_INDEX_BIN : path.join(repo, 'rust/target/release/floe-index'),
  ['vfs', source, path.join(root, '.valmini.oas.ice'), '--jobs', '4'], 'Synthetic index', 120000);
const before = snapshot(root);
try {
  if(parityOnly) {
    for(const reuse of ['on','off']) {
      console.log('ELECTRON LAYOUT: native pan reuse '+reuse);
      run('sh', [path.join(repo, 'tools/run_electron_dev.sh'), '--smoke-frame-parity-test', source],
        'Actual foreground/margin pixel parity', 120000, {...process.env,FLOE_RUST_PAN_REUSE:reuse});
    }
  } else {
  run('sh', [path.join(repo, 'tools/run_electron_dev.sh'), '--smoke-layout-test', source], 'Actual Electron layout', 120000);
  run('sh', [path.join(repo, 'tools/run_electron_dev.sh'), '--smoke-clip-download-test', source], 'Actual Electron exact clip POST', 120000);
  run('sh', [path.join(repo, 'tools/run_electron_dev.sh'), '--smoke-recovery-test', source], 'Actual Electron recovery/crash', 120000);
  run('sh', [path.join(repo, 'tools/run_electron_dev.sh'), '--smoke-recovery-storage-test', source], 'Actual Electron session storage loss', 60000);
  run('sh', [path.join(repo, 'tools/run_electron_dev.sh'), '--smoke-recovery-cookie-test', source], 'Actual Electron session cookie loss', 60000);
  }
} finally {
  if (JSON.stringify(before) !== JSON.stringify(snapshot(root))) throw new Error('Synthetic source/cache changed during view QA');
}
console.log('ELECTRON LAYOUT GATE: OK (source/cache unchanged; synthetic artifacts retained)');
