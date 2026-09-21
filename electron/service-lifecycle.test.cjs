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

test('pipelined init and cancel remain visible to the real poll loop', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-pipeline-'));
  const child = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'] });
  child.stdout.on('data', () => {}); child.stderr.on('data', () => {});
  const closed = once(child, 'close');
  try {
    child.stdin.write(JSON.stringify({ v: 1, args: ['--root', root] }) + '\ncancel\n');
    const [code] = await deadline(closed);
    assert.equal(code, 143);
    assert.deepEqual(fs.readdirSync(root), []);
  } finally { child.stdin.end(); await deadline(closed); fs.rmdirSync(root); }
});

test('real private pipe: explicit folder, one-use auth, confirmed shutdown, no files', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-electron-ipc-'));
  fs.chmodSync(root, 0o700);
  const service = new ServiceClient(binary, []);
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
  const service = new ServiceClient(binary, ['--root', root]);
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
  const service = new ServiceClient(binary, ['--root', root]);
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
