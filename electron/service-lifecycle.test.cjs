'use strict';
// Explicit integration test: a NEW empty root only. No Electron/GUI yet.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { once } = require('node:events');
const { ServiceClient } = require('./service-client.cjs');
const binary = process.env.FLOE_ELECTRON_SERVICE_BIN;
if (!binary || !path.isAbsolute(binary)) throw new Error('Set an absolute FLOE_ELECTRON_SERVICE_BIN');
for (const key of ['FLOE_INDEX_BIN', 'FLOE_RENDERD_BIN']) {
  if (!process.env[key] || !path.isAbsolute(process.env[key]) || !fs.existsSync(process.env[key])) throw new Error('Set an existing absolute ' + key);
}

function deadline(promise, ms = 30000) {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error('Private service test timed out')), ms);
  })]).finally(() => clearTimeout(timer));
}

async function login(ready) {
  const html = await (await fetch(ready.origin + '/')).text();
  const bundle = /name="floe-bundle" content="([^"]+)"/.exec(html)?.[1];
  assert.ok(bundle);
  const response = await fetch(ready.origin + '/api/v1/session/exchange', {
    method: 'POST', headers: { Origin: ready.origin, 'Content-Type': 'application/json' },
    body: JSON.stringify({ protocol: 1, bundle, bootstrap: ready.url.split('#bootstrap=')[1] }),
  });
  assert.equal(response.status, 200);
  const cookie = response.headers.get('set-cookie')?.split(';')[0];
  const { csrf } = await response.json();
  assert.ok(typeof cookie === 'string' && typeof csrf === 'string');
  return async (method, route, body) => {
    const reply = await fetch(ready.origin + route, { method, headers: {
      Origin: ready.origin, 'Content-Type': 'application/json', Cookie: cookie, 'X-Floe-CSRF': csrf,
    }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
    assert.equal(reply.status, 200);
    return reply.json();
  };
}

async function until(fn) {
  const end = Date.now() + 30000;
  while (Date.now() < end) {
    const result = await fn();
    if (result) return result;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  throw new Error('Private instance state timed out');
}

test('real Electron instance: quiet forwarding, present, busy, multi and DISPLAY isolation', async () => {
  // Short, fresh private directory: never join the user's real instance key.
  const root = fs.mkdtempSync('/tmp/fei.'); fs.chmodSync(root, 0o700);
  const env = { ...process.env, FLOE_ELECTRON_INSTANCE_DIR: root, DISPLAY: ':floe-electron-test.0' };
  const owner = new ServiceClient(binary, ['--root', root], env);
  let presented = 0;
  owner.on('present', () => { presented++; });
  const children = [owner];
  try {
    const [ready] = await deadline(once(owner, 'ready'));
    const call = await login(ready);
    assert.equal((await call('GET', '/api/v1/capabilities')).launcher, true);
    async function forward(success) {
      // No source/root: MUST forward before folder choice/native discovery.
      const sender = new ServiceClient(binary, [], { ...env, DISPLAY: ':floe-electron-test.1',
        FLOE_INDEX_BIN: '/missing/index', FLOE_RENDERD_BIN: '/missing/renderd' });
      children.push(sender);
      const events = [];
      for (const name of ['starting', 'directory', 'ready', 'forwarded', 'present']) sender.on(name, () => events.push(name));
      const exit = await deadline(sender.finished);
      assert.equal(exit.code, success ? 0 : 1);
      assert.deepEqual(events, success ? ['forwarded'] : []);
    }
    await forward(true);
    await until(() => presented === 1);
    const pending = await until(async () => {
      const p = (await call('GET', '/api/v1/launch')).pending;
      return p?.phase === 'ready' ? p : null;
    });
    assert.equal(pending.request, null);
    await forward(false); // an unresolved proposal cannot be silently replaced
    assert.equal(presented, 1);
    await call('POST', '/api/v1/launch/' + pending.id, { action: 'present' });
    assert.equal((await call('GET', '/api/v1/operations')).last_seq, '0');

    for (const [args, extra, launcher] of [
      [['--multi'], {}, false], [['--jobs', '1'], {}, false],
      [[], { DISPLAY: ':floe-electron-other.0' }, true],
    ]) {
      const other = new ServiceClient(binary, [...args, '--root', root], { ...env, ...extra });
      children.push(other);
      const [otherReady] = await deadline(once(other, 'ready'));
      assert.notEqual(otherReady.origin, ready.origin);
      const otherCall = await login(otherReady);
      assert.equal((await otherCall('GET', '/api/v1/capabilities')).launcher, launcher);
      other.close(); assert.equal((await deadline(other.finished)).code, 143);
    }
    await forward(true);
    await until(() => presented === 2);
  } finally {
    for (const child of children) child.close();
    await deadline(Promise.all(children.map(child => child.finished)));
    // Only this test's fresh IPC directory; never touch a normal owner socket.
    fs.rmSync(root, { recursive: true });
  }
});

test('folder-stage owner holds the lock; interrupted startup permits a new owner', async () => {
  const root = fs.mkdtempSync('/tmp/fes.'); fs.chmodSync(root, 0o700);
  const env = { ...process.env, FLOE_ELECTRON_INSTANCE_DIR: root, DISPLAY: ':floe-electron-startup' };
  const owner = new ServiceClient(binary, [], env);
  const children = [owner];
  try {
    await deadline(once(owner, 'directory')); // lock held; no service or worker yet
    const blocked = new ServiceClient(binary, ['--root', root], env); children.push(blocked);
    const events = [];
    for (const name of ['starting', 'directory', 'ready', 'forwarded']) blocked.on(name, () => events.push(name));
    assert.equal((await deadline(blocked.finished)).code, 1);
    assert.deepEqual(events, []); // no fallback owner/auth/folder choice
    owner.child.kill('SIGKILL');
    assert.equal((await deadline(owner.finished)).signal, 'SIGKILL');
    const replacement = new ServiceClient(binary, [], env); children.push(replacement);
    await deadline(once(replacement, 'directory'));
    const landing = once(replacement, 'ready'); replacement.chooseDirectory(root);
    const [ready] = await deadline(landing);
    const call = await login(ready);
    assert.equal((await call('GET', '/api/v1/capabilities')).launcher, true);
  } finally {
    for (const child of children) child.close();
    await deadline(Promise.all(children.map(child => child.finished)));
    fs.rmSync(root, { recursive: true });
  }
});

test('pipelined init and cancel remain visible to the real poll loop', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-pipeline-'));
  const child = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'] });
  child.stdout.on('data', () => {}); child.stderr.on('data', () => {});
  const closed = once(child, 'close');
  try {
    child.stdin.write(JSON.stringify({ v: 1, args: ['--multi', '--root', root] }) + '\ncancel\n');
    const [code] = await deadline(closed);
    assert.equal(code, 143);
    assert.deepEqual(fs.readdirSync(root), []);
  } finally { child.stdin.end(); await deadline(closed); fs.rmdirSync(root); }
});

test('real private pipe: explicit folder, one-use auth, confirmed shutdown, no files', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-ipc-'));
  fs.chmodSync(root, 0o700);
  const service = new ServiceClient(binary, ['--multi']);
  let failed = false;
  service.on('failure', () => { failed = true; });
  try {
    await deadline(once(service, 'directory'));
    const landing = once(service, 'ready');
    service.chooseDirectory(root);
    const [ready] = await deadline(landing);
    const page = await fetch(ready.origin + '/');
    assert.equal(page.status, 200);
    const html = await page.text();
    const bundle = /name="floe-bundle" content="([^"]+)"/.exec(html)?.[1];
    assert.ok(bundle, 'public bundle identifier');
    const exchange = JSON.stringify({ protocol: 1, bundle, bootstrap: ready.url.split('#bootstrap=')[1] });
    const headers = { Origin: ready.origin, 'Content-Type': 'application/json' };
    const response = await fetch(ready.origin + '/api/v1/session/exchange', { method: 'POST', headers, body: exchange });
    assert.equal(response.status, 200); // no credential-bearing body in assertions
    const cookie = response.headers.get('set-cookie')?.split(';')[0];
    const { csrf } = await response.json();
    assert.ok(typeof cookie === 'string' && typeof csrf === 'string');
    const duplicate = await fetch(ready.origin + '/api/v1/session/exchange', { method: 'POST', headers, body: exchange });
    assert.notEqual(duplicate.status, 200);
    const browse = await fetch(ready.origin + '/api/v1/browse', { headers: { ...headers, Cookie: cookie, 'X-Floe-CSRF': csrf } });
    assert.equal(browse.status, 200);
    assert.ok((await browse.text()).includes(path.basename(root)));
    const end = await fetch(ready.origin + '/api/v1/session', { method: 'DELETE', headers: { ...headers, Cookie: cookie, 'X-Floe-CSRF': csrf } });
    assert.equal(end.status, 204);
    const exit = await deadline(service.finished);
    assert.equal(exit.code, 0);
    assert.equal(failed, false);
    assert.deepEqual(fs.readdirSync(root), []);
    await assert.rejects(fetch(ready.origin + '/'));
  } finally {
    service.close();
    await deadline(service.finished);
    fs.rmdirSync(root); // unexpected files are not recursively erased
  }
});

test('parent pipe EOF cancels a live owned service', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-eof-'));
  const service = new ServiceClient(binary, ['--multi', '--root', root]);
  try {
    const [ready] = await deadline(once(service, 'ready'));
    service.close();
    const exit = await deadline(service.finished);
    assert.equal(exit.code, 143);
    await assert.rejects(fetch(ready.origin + '/'));
    assert.deepEqual(fs.readdirSync(root), []);
  } finally {
    service.close();
    await deadline(service.finished);
    fs.rmdirSync(root);
  }
});

test('regular stdout file rejected before even requesting a folder', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-redirection-'));
  const output = path.join(root, 'empty');
  const fd = fs.openSync(output, 'wx', 0o600);
  try {
    const child = spawn(binary, [], { stdio: ['pipe', fd, 'ignore'] });
    child.stdin.on('error', () => {});
    child.stdin.end(JSON.stringify({ v: 1, args: [] }) + '\n');
    const [code] = await deadline(once(child, 'close'));
    assert.equal(code, 1);
    assert.equal(fs.statSync(output).size, 0);
  } finally {
    fs.closeSync(fd);
    fs.unlinkSync(output);
    fs.rmdirSync(root);
  }
});

test('malformed live control revokes the owned service with an error', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-control-'));
  const service = new ServiceClient(binary, ['--multi', '--root', root]);
  try {
    const [ready] = await deadline(once(service, 'ready'));
    service.child.stdin.write('not-a-command\n');
    const exit = await deadline(service.finished);
    assert.equal(exit.code, 1);
    await assert.rejects(fetch(ready.origin + '/'));
    assert.deepEqual(fs.readdirSync(root), []);
  } finally {
    service.close();
    await deadline(service.finished);
    fs.rmdirSync(root);
  }
});

test('malformed initial input is never echoed and cannot emit a credential', async () => {
  const child = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'] });
  let out = '', err = '';
  child.stdout.on('data', b => { out += b; });
  child.stderr.on('data', b => { err += b; });
  child.stdin.on('error', () => {});
  child.stdin.end('{"v":1,"args":[],"extra":"#bootstrap=synthetic-do-not-echo"}\n');
  const [code] = await deadline(once(child, 'close'));
  assert.equal(code, 1);
  assert.equal(out, '');
  assert.equal(err.includes('synthetic-do-not-echo'), false);
});
