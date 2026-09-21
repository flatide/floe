'use strict';
// Actual Rust service SIGKILL/restart, not a Chromium or storage-fault simulation.
// No arguments, user files, GUI, clipboard, saved auth, remote endpoint or Python.
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { createHash } = require('node:crypto');
const { ServiceClient } = require('../electron/service-client.cjs');
const { writeInputs } = require('./electron-review-fixture.cjs');
if (process.argv.length !== 2 || !['darwin', 'linux'].includes(process.platform)) throw Error('macOS/Linux synthetic QA; no arguments');
const repo = path.resolve(__dirname, '..');
const binary = (key, fallback) => {
  const value = Object.hasOwn(process.env, key) ? process.env[key] : path.join(repo, fallback);
  if (!value || !path.isAbsolute(value)) throw Error('Explicit executable required');
  fs.accessSync(value, fs.constants.X_OK); return value;
};
const index = binary('FLOE_INDEX_BIN', 'rust/target/release/floe-index');
const renderd = binary('FLOE_RENDERD_BIN', 'rust/target/release/floe-renderd');
const serviceBin = binary('FLOE_ELECTRON_SERVICE_BIN', 'electron/service/target/debug/floe-electron-service');
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-service-review-'));
fs.chmodSync(root, 0o700);
console.log('SERVICE REVIEW: new synthetic artifacts ' + root);
let stage = 'fixtures', problem = 'runtime or timeout', current, killed = 0, reopened = 0;
const refs = [{ check: '0', error: '0' }];
const reviewer = 'service-restart-test';
const message = 'Synthetic service restart — 한글';
function check(condition, reason) { if (!condition) { problem = reason; throw Error('Synthetic assertion'); } }
function deadline(promise, ms = 30000) {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => {
    timer = setTimeout(() => reject(Error('Deadline exceeded')), ms);
  })]).finally(() => clearTimeout(timer));
}
async function until(fn) {
  const end = Date.now() + 30000;
  while (Date.now() < end) {
    check(current?.service.phase !== 'ended', 'Service ended unexpectedly');
    const value = await fn(); if (value) return value;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  throw Error('State deadline exceeded');
}
function childPids(parent) {
  // Development harness only: numeric parent/child identity, no command lines,
  // environment, unrelated process details or caller-supplied PID selection.
  const result = spawnSync('/bin/ps', ['-axo', 'pid=,ppid='], { encoding: 'utf8', timeout: 5000 });
  check(result.status === 0, 'Child inventory unavailable');
  return result.stdout.trim().split('\n').map(line => line.trim().split(/\s+/).map(Number))
    .filter(([pid, ppid]) => Number.isSafeInteger(pid) && pid > 0 && ppid === parent).map(([pid]) => pid);
}
async function childrenEnded(pids) {
  const remaining = new Set(pids), end = Date.now() + 15000;
  while (remaining.size && Date.now() < end) {
    for (const pid of remaining) {
      try { process.kill(pid, 0); } catch (error) {
        if (error.code === 'ESRCH') remaining.delete(pid); else throw Error('Child liveness check failed');
      }
    }
    if (remaining.size) await new Promise(resolve => setTimeout(resolve, 20));
  }
  check(remaining.size === 0, 'Previously owned worker still alive after service exit');
}
function fingerprint(directory) {
  const result = {};
  function walk(dir) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else {
        check(entry.isFile(), 'Unexpected fixture entry');
        const stat = fs.statSync(full);
        result[path.relative(directory, full)] = [stat.size, stat.mode, stat.mtimeMs,
          createHash('sha256').update(fs.readFileSync(full)).digest('hex')];
      }
    }
  }
  walk(directory); return result;
}
function targetFor(work, kind) {
  return path.join(work, kind === 'notes' ? '.synthetic.db.notes.' + reviewer + '.fe' : '.synthetic.db.waive.' + reviewer);
}
function verifyDisk(work, kind, published, text = message, waived = true) {
  const target = targetFor(work, kind);
  if (!published) { check(!fs.existsSync(target), 'Unapproved draft was published'); return; }
  const st = fs.lstatSync(target);
  check(st.isFile() && st.nlink === 1 && (st.mode & 0o777) === 0o600, 'Sidecar identity/mode');
  const bytes = fs.readFileSync(target);
  if (kind === 'notes') {
    check(JSON.stringify(bytes.toString('utf8').split('\n').filter(x => x.startsWith('floe_note='))) ===
      JSON.stringify(['floe_note=0|' + text]), 'Note disk content');
  } else {
    check(bytes.length === 46 && bytes.subarray(0, 8).toString('ascii') === 'FLOEWAIV' &&
      bytes.readUInt32LE(8) === 1 && bytes.readBigUInt64LE(12) === BigInt(fs.statSync(path.join(work, 'synthetic.db')).size) &&
      bytes.readBigUInt64LE(28) === 2n && bytes.readUInt32LE(36) === 1 && bytes[40] === Number(waived) && bytes[41] === 0 &&
      bytes.readUInt32LE(42) === Number(waived), 'Waive disk content');
  }
}
class Owner {
  constructor(work, tmp) {
    this.service = new ServiceClient(serviceBin, [path.join(work, 'synthetic.oas'), '--drc', path.join(work, '.synthetic.db.tray'),
      '--drc-reviewer', reviewer, '--drc-edit-waives', '--jobs', '2', '--raster-jobs', '1', '--budget-mb', '256',
      '--refinement', 'off', '--frame-cache', 'off', '--no-labels'],
      { PATH: '', TMPDIR: tmp, FLOE_INDEX_BIN: index, FLOE_RENDERD_BIN: renderd });
    this.failed = false;
    this.service.on('failure', () => { this.failed = true; });
    this.landing = deadline(once(this.service, 'ready'));
  }
  async response(method, route, body, code = 200, auth = this.auth) {
    check(route.startsWith('/api/v1/') || route === '/', 'Unexpected route');
    const response = await fetch(this.origin + route, { method, redirect: 'error', signal: AbortSignal.timeout(8000),
      headers: { Origin: this.origin, 'Content-Type': 'application/json', ...(auth || {}) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
    check(response.status === code, 'HTTP status ' + response.status + ' expected ' + code);
    return response;
  }
  async call(method, route, body, code = 200, auth = this.auth) {
    const response = await this.response(method, route, body, code, auth);
    if (response.status === 204) return null;
    return response.headers.get('content-type')?.includes('application/json') ? response.json() : response.text();
  }
  async start() {
    const [ready] = await this.landing; this.origin = ready.origin;
    const html = await this.call('GET', '/');
    const bundle = /name="floe-bundle" content="([^"]+)"/.exec(html)?.[1];
    check(!!bundle, 'Bundle identifier missing');
    const response = await this.response('POST', '/api/v1/session/exchange', {
      protocol: 1, bundle, bootstrap: ready.url.split('#bootstrap=')[1]
    });
    const cookie = response.headers.get('set-cookie')?.split(';')[0], auth = await response.json();
    check(typeof cookie === 'string' && typeof auth.csrf === 'string', 'Auth response invalid');
    this.auth = { Cookie: cookie, 'X-Floe-CSRF': auth.csrf };
    const catalog = await until(async () => { const d = (await this.call('GET', '/api/v1/drc')).drc; return d.phase === 'ready' && d; });
    const startup = (await this.call('GET', '/api/v1/startup')).request;
    startup.body.pixels = [257, 191];
    await this.call('POST', '/api/v1/operations', startup, 202);
    const opened = await until(async () => { const r = await this.call('GET', '/api/v1/operations/1');
      check(!['failed', 'cancelled', 'incomplete'].includes(r.phase), 'Open failed'); return r.phase === 'succeeded' && r; });
    this.context = { drc_id: catalog.id, revision: catalog.revision, view_id: opened.view_id };
    await until(async () => (await this.call('GET', '/api/v1/view')).view.status === 'idle');
    this.workers = childPids(this.service.child.pid);
    check(this.workers.length > 0, 'No live child worker before service test');
  }
  async draft(kind, text = message, waived = true) {
    const api = '/api/v1/drc/review/' + kind;
    const snapshot = await this.call('POST', api + '/read', { context: this.context, errors: refs });
    const prepared = await this.call('POST', api + '/prepare', { context: this.context, token: snapshot.token,
      ...(kind === 'notes' ? { text } : { waived }) });
    return { context: this.context, token: prepared.token, seq: '1', approve: true, confirm_legacy: false };
  }
  async stop() {
    this.service.close();
    await deadline(this.service.finished, 15000);
    if (this.workers) await childrenEnded(this.workers);
  }
}
async function run() {
  for (const kind of ['notes', 'waives']) for (const phase of ['prepared', 'published']) {
    const work = path.join(root, kind + '-' + phase); fs.mkdirSync(work, { mode: 0o700 }); writeInputs(work);
    const source = path.join(work, 'synthetic.oas'), db = path.join(work, 'synthetic.db');
    for (const args of [['vfs', source, path.join(work, '.synthetic.oas.ice'), '--jobs', '2'], ['drc', db, '--jobs', '2']]) {
      check(spawnSync(index, args, { stdio: 'ignore', timeout: 120000 }).status === 0, 'Synthetic index failed');
    }
    const before = fingerprint(work), tmp = path.join(root, kind + '-' + phase + '-runtime'); fs.mkdirSync(tmp, { mode: 0o700 });
    stage = kind + '/' + phase + '/start'; current = new Owner(work, tmp); await current.start();
    const api = '/api/v1/drc/review/' + kind;
    stage = kind + '/' + phase + '/prepare'; const request = await current.draft(kind);
    verifyDisk(work, kind, false);
    if (phase === 'published') {
      stage = kind + '/' + phase + '/submit';
      check((await current.call('POST', api, request, 202)).phase === 'queued', 'Test consumed a terminal receipt before SIGKILL');
      // Deliberately do NOT GET a terminal receipt. This is a known on-disk
      // boundary, not a claim of crashing inside write/fsync/link/unlink.
      await until(() => { try { verifyDisk(work, kind, true); return true; } catch (_) { return false; } });
    }
    const old = current, snapshot = fingerprint(work);
    stage = kind + '/' + phase + '/SIGKILL';
    check(old.service.phase === 'ready' && old.service.child.kill('SIGKILL'), 'Owned service not killed');
    const exit = await deadline(old.service.finished, 15000);
    check(exit.signal === 'SIGKILL' && exit.code === null, 'Not an actual service SIGKILL'); killed++;
    await childrenEnded(old.workers);
    verifyDisk(work, kind, phase === 'published');
    check(JSON.stringify(fingerprint(work)) === JSON.stringify(snapshot), 'Unexpected post-kill disk change');
    stage = kind + '/' + phase + '/restart'; current = new Owner(work, tmp); await current.start(); reopened++;
    stage = kind + '/' + phase + '/fresh ledger';
    const status = await current.call('GET', api);
    check(status.operations.last_seq === '0' && status.autosave === false, 'Restart restored an approval or receipt');
    check((await current.call('GET', api + '/1', undefined, 410)).error === 'operation_expired', 'Lost receipt was not explicitly expired');
    stage = kind + '/' + phase + '/old auth';
    await current.call('GET', api, undefined, 401, old.auth);
    stage = kind + '/' + phase + '/old approval';
    check((await current.call('POST', api, { ...request, context: current.context }, 410)).error === 'review_expired', 'Old approval not explicitly expired');
    check((await current.call('GET', api)).operations.last_seq === '0', 'Rejected old token consumed a sequence');
    check(JSON.stringify(fingerprint(work)) === JSON.stringify(snapshot), 'Restart/replay probe changed disk');
    stage = kind + '/' + phase + '/fresh read';
    const read = await current.call('POST', api + '/read', { context: current.context, errors: refs });
    if (kind === 'notes') check(read.text === (phase === 'published' ? message : null), 'Fresh note read differs');
    else check(read.waived_count === (phase === 'published' ? '1' : '0'), 'Fresh waive read differs');
    await current.call('POST', api + '/revoke', { token: read.token }, 204);
    stage = kind + '/' + phase + '/new approval';
    const newText = message + ' · new approval', newWaived = phase !== 'published';
    const fresh = await current.draft(kind, newText, newWaived);
    await current.call('POST', api, fresh, 202);
    const done = await until(async () => { const r = await current.call('GET', api + '/1');
      check(!['failed', 'cancelled'].includes(r.phase), 'Fresh approval failed'); return r.phase === 'succeeded' && r; });
    check(done.published === true && done.outcome_unknown === false, 'Fresh publication unconfirmed');
    verifyDisk(work, kind, true, newText, newWaived);
    const after = fingerprint(work);
    for (const [name, state] of Object.entries(before)) check(JSON.stringify(after[name]) === JSON.stringify(state), 'Synthetic input changed');
    const target = path.basename(targetFor(work, kind));
    check(JSON.stringify(Object.keys(after).filter(n => !Object.hasOwn(before, n)).sort()) ===
      JSON.stringify([target, target + '.lock'].sort()), 'Unexpected output beside inputs');
    await current.stop(); check(current.service.child.exitCode === 143, 'Normal host EOF cleanup failed'); current = null;
    console.log('SERVICE REVIEW: ' + kind + '/' + phase + ' SIGKILL -> explicit restart/read -> old token rejected -> new approval saved');
  }
  check(killed === 4 && reopened === 4, 'Incomplete crash matrix');
  console.log('SERVICE REVIEW: OK (4 owned Rust service SIGKILLs; previously owned workers exited; fresh auth/readers; no receipt replay; inputs preserved; no GUI/storage-fault claim)');
}
run().catch(async () => {
  console.error('SERVICE REVIEW: FAIL at ' + stage + ' (' + problem + ')'); // fixed reasons only; no credential/body/exception echo
  if (current && current.service.phase !== 'ended') {
    try { await current.stop(); } catch (_) {
      current.service.child.kill('SIGKILL');
      try { await deadline(current.service.finished, 15000); } catch (_) { console.error('SERVICE REVIEW: owned child cleanup unconfirmed'); }
    }
  }
  process.exitCode = 1;
});
