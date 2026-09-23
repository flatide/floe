'use strict';
// Actual host + actual second Electron processes, fresh synthetic data only.
// The wrapper observes product events; it never substitutes a service/window.
const { app, BrowserWindow } = require('electron');
const fs = require('node:fs'), path = require('node:path');
const { spawn, spawnSync, execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const assert = require('node:assert/strict');
const { ServiceClient } = require('../electron/service-client.cjs');
const repo = path.resolve(__dirname, '..');
const secondary = process.argv[2] === '--secondary';
if (!secondary && process.argv.length !== 2) throw Error('No user source or output paths accepted');
const counts = { windows: 0, shows: 0, starting: 0, ready: 0, forwarded: 0, present: 0 };
let servicePid;
app.on('browser-window-created', (_event, win) => { counts.windows++; win.on('show', () => counts.shows++); });
const originalEmit = ServiceClient.prototype.emit;
ServiceClient.prototype.emit = function(name, ...args) {
  if (Object.hasOwn(counts, name)) counts[name]++;
  if (name === 'ready') servicePid = this.child.pid;
  return originalEmit.call(this, name, ...args);
};
if (secondary) {
  process.argv.splice(2, 1);
  process.on('exit', () => console.log('ELECTRON INSTANCE SECONDARY: ' + JSON.stringify(counts)));
  require('../electron/main.cjs');
} else {
  const root = fs.mkdtempSync('/tmp/feq.'); fs.chmodSync(root, 0o700);
  const designs = path.join(root, 'designs'); fs.mkdirSync(designs, { mode: 0o700 });
  process.env.FLOE_ELECTRON_INSTANCE_DIR = root;
  process.env.DISPLAY = ':floe-native-instance-qa.0';
  const source = path.join(designs, '한국 with spaces.oas');
  const other = path.join(designs, 'other.oas');
  const index = process.env.FLOE_INDEX_BIN || path.join(repo, 'rust/target/release/floe-index');
  const python = process.env.FLOE_QA_PYTHON_BIN || path.join(repo, '.venv/bin/python');
  function setup(binary, args) {
    const result = spawnSync(binary, args, { cwd: repo, env: process.env, stdio: 'pipe', timeout: 120000 });
    if (result.status !== 0) throw Error('Synthetic instance fixture preparation failed');
  }
  setup(python, ['-B', path.join(repo, 'tools/gen_valmini.py'), source]);
  setup(index, ['vfs', source, path.join(designs, '.' + path.basename(source) + '.ice'), '--jobs', '4']);
  fs.copyFileSync(source, other);
  setup(index, ['vfs', other, path.join(designs, '.other.oas.ice'), '--jobs', '4']);
  function snapshot(dir) {
    return fs.readdirSync(dir).sort().map(name => {
      const file = path.join(dir, name), stat = fs.lstatSync(file);
      if (stat.isDirectory()) return [name, snapshot(file)];
      if (!stat.isFile()) throw Error('Unexpected synthetic fixture entry');
      return [name, createHash('sha256').update(fs.readFileSync(file)).digest('hex')];
    });
  }
  const before = snapshot(designs);
  process.argv = [process.argv[0], path.join(repo, 'electron'), 'view', '--root', designs];
  require('../electron/main.cjs');
  let phase = 'initial', ok = false;
  const exit = app.exit.bind(app);
  app.exit = code => exit(ok ? code : 1); // an incomplete QA never becomes exit0
  const children = new Set();
  const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
  async function until(fn, ms = 30000) {
    const end = Date.now() + ms;
    while (Date.now() < end) { const value = await fn(); if (value) return value; await delay(20); }
    throw Error('Native instance QA deadline');
  }
  function rustTree() {
    const rows = execFileSync('/bin/ps', ['-e', '-o', 'pid=,ppid='], { encoding: 'utf8' }).trim().split('\n')
      .map(s => s.trim().split(/\s+/).map(Number));
    const ids = new Set([servicePid]);
    for (let changed = true; changed;) {
      changed = false;
      for (const [pid, ppid] of rows) if (ids.has(ppid) && !ids.has(pid)) { ids.add(pid); changed = true; }
    }
    return [...ids].sort((a, b) => a - b);
  }
  async function forward(...args) {
    const child = spawn(process.execPath, [__filename, '--secondary', 'view', ...args], {
      cwd: designs, env: { ...process.env, DISPLAY: ':floe-native-instance-qa.1',
        FLOE_INDEX_BIN: '/missing/index', FLOE_RENDERD_BIN: '/missing/renderd', FLOE_ELECTRON_DOWNLOAD_BIN: '/missing/download' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    children.add(child); let output = '', done = false, code;
    child.stdout.on('data', data => { if (output.length < 65536) output += data; });
    child.stderr.on('data', () => {});
    child.once('error', () => { done = true; code = -1; });
    child.once('close', result => { done = true; code = result; children.delete(child); });
    await until(() => done);
    assert.equal(code, 0);
    const line = output.split('\n').find(s => s.startsWith('ELECTRON INSTANCE SECONDARY: '));
    assert.ok(line);
    assert.deepEqual(JSON.parse(line.slice('ELECTRON INSTANCE SECONDARY: '.length)),
      { windows: 1, shows: 0, starting: 0, ready: 0, forwarded: 1, present: 0 });
  }
  app.whenReady().then(async () => {
    const win = await until(() => BrowserWindow.getAllWindows()[0]);
    const evaluate = script => win.webContents.executeJavaScript(script, false);
    await until(async () => counts.ready === 1 && await evaluate("!!document.getElementById('logout')&&!document.getElementById('logout').disabled&&!document.hidden&&!location.hash"));
    // An empty owner intentionally opens the server-file picker. Explicitly
    // dismiss that synthetic dialog before testing automatic CLI source open.
    await until(async () => await evaluate("!document.getElementById('browse-dialog').hidden&&!document.getElementById('browse-close').disabled"));
    await evaluate("document.getElementById('browse-close').click();true");
    assert.equal(await evaluate("document.getElementById('browse-dialog').hidden"), true);
    const initialPid = servicePid;
    phase = 'hidden owner / new relative source';
    win.hide(); assert.equal(win.isVisible(), false);
    await forward(path.basename(source), '--goto', '200,200,300', '--depth', 'full', '--detail', 'high');
    console.log('ELECTRON INSTANCE: secondary forwarded without show/auth/download');
    const settled = width => until(async () => await evaluate(`(()=>{const e=id=>document.getElementById(id);return !document.hidden&&
      !e('fit').disabled&&e('empty').hidden&&e('rendering').hidden&&/^Live/.test(e('status').textContent)&&
      e('launch-panel').hidden&&Number(e('goto-width').value)===${width};})()`));
    await settled(300);
    console.log('ELECTRON INSTANCE: hidden owner restored; first synthetic layout live');
    assert.equal(win.isVisible(), true); assert.equal(servicePid, initialPid);
    const workerPids = rustTree(); assert.ok(workerPids.length >= 2);
    const state = () => evaluate("(()=>{const e=id=>document.getElementById(id);return {source:e('source').value,rev:e('canvas').dataset.renderRev,frame:e('canvas').dataset.frameId};})()");
    const first = await state();
    phase = 'minimized owner / empty present';
    win.minimize(); await until(() => win.isMinimized());
    await forward();
    await until(async () => !win.isMinimized() && win.isVisible() && await evaluate("!document.hidden&&document.getElementById('launch-panel').hidden"));
    assert.deepEqual(await state(), first); assert.deepEqual(rustTree(), workerPids);
    console.log('ELECTRON INSTANCE: minimized owner restored; empty present kept frame');
    phase = 'same source / retained worker';
    await forward(path.basename(source), '--goto', '200,200,250', '--depth', 'full', '--detail', 'high');
    await settled(250);
    assert.equal((await state()).source, first.source); assert.deepEqual(rustTree(), workerPids);
    console.log('ELECTRON INSTANCE: same-source goto retained Rust worker');
    phase = 'pending confirmation / new source';
    await evaluate("document.getElementById('logout').click();true");
    const oldSource = (await state()).source;
    await forward(path.basename(other), '--goto', '200,200,280');
    await until(async () => await evaluate("!document.getElementById('launch-panel').hidden"));
    // Observe several auto-open retry turns; no modal approval or source change.
    await delay(350);
    assert.equal(await evaluate("!document.getElementById('session-exit-dialog').hidden"), true);
    assert.equal((await state()).source, oldSource);
    await evaluate("document.getElementById('session-exit-cancel').click();true");
    await settled(280);
    assert.notEqual((await state()).source, oldSource);
    assert.equal(BrowserWindow.getAllWindows().length, 1);
    assert.equal(counts.starting, 1); assert.equal(counts.ready, 1); assert.equal(counts.present, 4);
    assert.deepEqual(snapshot(designs), before);
    phase = 'explicit synthetic session end';
    fs.writeFileSync(path.join(root, 'owner.png'), (await win.webContents.capturePage()).toPNG(), { flag: 'wx', mode: 0o600 });
    console.log('ELECTRON INSTANCE: synthetic screenshot ' + path.join(root, 'owner.png'));
    ok = true;
    await evaluate("document.getElementById('logout').click();document.getElementById('session-exit-confirm').click();true");
  }).catch(async () => {
    console.error('ELECTRON INSTANCE: FAIL at ' + phase);
    const win = BrowserWindow.getAllWindows()[0];
    if (win && !win.isDestroyed()) {
      try {
        const flags = await win.webContents.executeJavaScript(`(()=>{const e=id=>document.getElementById(id);return {
          visible:!document.hidden, live:/^Live/.test(e('status').textContent), fit:!e('fit').disabled,
          width:Number(e('goto-width').value), empty:e('empty').hidden, rendering:e('rendering').hidden,
          renderer_failed:/renderer failed/i.test(e('notice').textContent), launch:!e('launch-panel').hidden,
          launch_disabled:e('launch-open').disabled,
          blocking:['browse-dialog','index-open-dialog','about-dialog','session-exit-dialog','share-dialog',
            'notes-editor','notes-review','notes-uncertain','notes-cancel','waives-editor','waives-review','waives-uncertain','waives-cancel',
            'default-review','default-uncertain','default-cancel'].filter(id=>!e(id).hidden)};})()`, false);
        console.error('ELECTRON INSTANCE: flags ' + JSON.stringify({ ...flags, ...counts, nativeVisible: win.isVisible(), minimized: win.isMinimized() }));
        fs.writeFileSync(path.join(root, 'failed.png'), (await win.webContents.capturePage()).toPNG(), { flag: 'wx', mode: 0o600 });
        console.error('ELECTRON INSTANCE: synthetic failure screenshot ' + path.join(root, 'failed.png'));
      } catch (_) { console.error('ELECTRON INSTANCE: diagnostic view unavailable'); }
    }
    for (const child of children) child.kill('SIGTERM');
    process.exitCode = 1;
    process.kill(process.pid, 'SIGTERM'); // product EOF/cancel/join, not a bypass
  });
  process.on('exit', code => {
    if (!ok || code !== 0) console.error('ELECTRON INSTANCE: incomplete');
    else console.log('ELECTRON INSTANCE: OK (two actual hosts; no secondary show/auth/download; present restores; worker reuse; modal waits; source/cache unchanged)');
  });
}
