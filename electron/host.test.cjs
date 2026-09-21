'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const P = require('./policy.cjs');
const { CloseController } = require('./close-controller.cjs');
const origin = 'http://127.0.0.1:32123';
test('only owning root navigates; only owning HTTP/WS can request', () => {
  for (const suffix of ['/', '/#', '/#bootstrap=synthetic']) assert.ok(P.navigationAllowed(origin, origin + suffix));
  for (const url of ['file:///tmp/x', 'https://example.com/', origin + '/other', origin + '/?x',
    'http://127.0.0.1:32124/', origin + '.evil/', origin + '@evil/', 'javascript:1', 'about:blank']) {
    assert.equal(P.navigationAllowed(origin, url), false);
  }
  assert.equal(P.navigationAllowed(origin, origin + '/', false), false);
  assert.equal(P.navigationAllowed(null, origin + '/'), false);
  for (const url of [origin + '/api/v1/session', origin + '/assets/x.js', 'ws://127.0.0.1:32123/api/v1/ws']) assert.ok(P.requestAllowed(origin, url));
  for (const url of ['http://localhost:32123/', origin + '.evil/', 'file:///tmp/x', 'https://example.com/', 'ws://127.0.0.1:32124/']) assert.equal(P.requestAllowed(origin, url), false);
  const prefs = P.webPreferences('ephemeral-test');
  assert.equal(prefs.sandbox, true);
  assert.equal(prefs.contextIsolation, true);
  assert.equal(prefs.nodeIntegration, false);
  assert.equal(prefs.webSecurity, true);
  assert.equal(prefs.webviewTag, false);
  assert.equal(prefs.preload, undefined);
  assert.equal(prefs.spellcheck, false);
});
test('an explicit invalid service override never silently falls back', () => {
  assert.equal(P.serviceBinary({}, '/default'), '/default');
  assert.equal(P.serviceBinary({ FLOE_ELECTRON_SERVICE_BIN: '/tmp/with spaces/service' }, '/default'), '/tmp/with spaces/service');
  for (const value of ['', undefined, null, 'x\0y']) {
    assert.throws(() => P.serviceBinary({ FLOE_ELECTRON_SERVICE_BIN: value }, '/default'));
  }
});
test('dev launcher preserves cwd and quoted arguments, requires an explicit executable', () => {
  const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
  const { spawnSync } = require('node:child_process');
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-launch-test-'));
  const launcher = path.resolve(__dirname, '../tools/run_electron_dev.sh');
  try {
    const binary = path.join(root, 'fake runtime');
    // A test-owned fake runtime only; this test does not claim Chromium execution.
    fs.writeFileSync(binary, '#!' + process.execPath + '\nprocess.stdout.write(JSON.stringify({cwd:process.cwd(),args:process.argv.slice(2)}));\n', { mode: 0o700 });
    const result = spawnSync('sh', [launcher, 'view', 'relative 한글 layout.oas', '--detail', 'high'], {
      cwd: root, env: { ...process.env, FLOE_ELECTRON_BIN: binary }, encoding: 'utf8' });
    assert.equal(result.status, 0);
    const actual = JSON.parse(result.stdout);
    assert.equal(fs.realpathSync(actual.cwd), fs.realpathSync(root));
    assert.deepEqual(actual.args, [__dirname, 'view', 'relative 한글 layout.oas', '--detail', 'high']);
    for (const value of ['', path.join(root, 'missing')]) {
      const denied = spawnSync('sh', [launcher], { env: { ...process.env, FLOE_ELECTRON_BIN: value }, encoding: 'utf8' });
      assert.equal(denied.status, 2);
      assert.equal(denied.stdout, '');
    }
  } finally { fs.rmSync(root, { recursive: true }); }
});
function setup(openDialog, confirmForce = async () => false) {
  const counts = { revealed: 0, cancelled: 0, prompts: 0 };
  const close = new CloseController({ reveal: () => counts.revealed++, openDialog,
    confirmForce: async () => { counts.prompts++; return confirmForce(); },
    cancelService: () => counts.cancelled++, timeout: 10 });
  return { close, counts };
}
test('normal close only opens web confirmation; fallback defaults to no action', async () => {
  const { close, counts } = setup(async () => 'opened');
  await close.request();
  assert.deepEqual(counts, { revealed: 1, cancelled: 0, prompts: 0 });
  close.openDialog = async () => { throw new Error('synthetic'); };
  await close.request();
  assert.equal(counts.cancelled, 0);
  assert.equal(counts.prompts, 1);
});
test('timeout offers once, duplicate requests never authorize cancellation', async () => {
  const { close, counts } = setup(() => new Promise(() => {}));
  await Promise.all([close.request(), close.request(), close.request()]);
  assert.equal(counts.prompts, 1);
  assert.equal(counts.cancelled, 0);
});
test('only current explicit fallback acceptance cancels', async () => {
  const a = setup(async () => 'unavailable', async () => true);
  await a.close.request(); assert.equal(a.counts.cancelled, 1);
  let approve;
  const b = setup(async () => 'unavailable', () => new Promise(resolve => { approve = resolve; }));
  const request = b.close.request();
  await new Promise(resolve => setImmediate(resolve));
  b.close.invalidate(); approve(true); await request;
  assert.equal(b.counts.cancelled, 0);
  b.close.end(); await b.close.request();
  assert.equal(b.counts.revealed, 1);
});
