'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { EventEmitter } = require('node:events');
const { decodeFrame, encodeFrame, ServiceClient } = require('./service-client.cjs');
const origin = 'http://127.0.0.1:32123';
const token = 'a'.repeat(64); // synthetic, never a live credential
const ready = { v: 1, event: 'ready', origin, url: origin + '/#bootstrap=' + token };

test('strict private ready/directory messages', () => {
  assert.deepEqual(decodeFrame(encodeFrame(ready)), ready);
  assert.deepEqual(decodeFrame(encodeFrame({ v: 1, event: 'directory' })), { v: 1, event: 'directory' });
  for (const value of [null, [], {}, { ...ready, v: 2 }, { ...ready, extra: true },
    { v: 1, event: 'directory', extra: true }, { ...ready, url: 'file:///tmp/x' },
    { ...ready, origin: 'http://127.0.0.1:65536' },
    { ...ready, origin: 'http://127.0.0.1:0' },
    { ...ready, origin: 'http://localhost:32123' },
    { ...ready, url: ready.url + 'x' }, { ...ready, url: ready.url.replace('/#', '/other#') }]) {
    assert.throws(() => decodeFrame(encodeFrame(value)), /^Error: Invalid service frame$/);
  }
  assert.throws(() => decodeFrame(Buffer.from([255, 10])), /^Error: Invalid service frame$/);
});

test('bounded requests preserve spaces and UTF-8', () => {
  const input = { v: 1, args: ['한국 칩.oas', '--detail', 'high'] };
  assert.deepEqual(JSON.parse(encodeFrame(input)), input);
  assert.throws(() => encodeFrame({ text: 'x'.repeat(65536) }), /too large/);
});

function client() {
  const value = Object.create(ServiceClient.prototype);
  EventEmitter.call(value);
  Object.assign(value, { phase: 'starting', buffer: Buffer.alloc(0), closing: false, failed: false });
  const seen = { ready: 0, directory: 0, failure: 0, closed: 0, writes: [] };
  value.child = { stdin: { end() { seen.closed++; }, write(bytes) { seen.writes.push(bytes); } } };
  for (const name of ['ready', 'directory', 'failure']) value.on(name, () => seen[name]++);
  return { value, seen };
}

test('every ready split boundary, duplicate rejection and late-frame discard', () => {
  const wire = encodeFrame(ready);
  for (let at = 0; at <= wire.length; at++) {
    const { value, seen } = client();
    value.receive(wire.subarray(0, at));
    value.receive(wire.subarray(at));
    assert.equal(seen.ready, 1);
    assert.equal(seen.failure, 0);
    value.receive(wire);
    assert.equal(seen.failure, 1);
    assert.equal(seen.closed, 1);
    value.receive(wire);
    assert.equal(seen.ready, 1);
  }
});

test('folder choice is explicit and one-use; malformed/oversized streams close', () => {
  const { value, seen } = client();
  value.receive(encodeFrame({ v: 1, event: 'directory' }));
  assert.equal(seen.directory, 1);
  assert.throws(() => value.chooseDirectory('relative'), /Invalid folder/);
  value.chooseDirectory('/tmp/한국 폴더');
  assert.equal(seen.writes.length, 1);
  assert.throws(() => value.chooseDirectory('/tmp/other'), /Invalid folder/);
  value.receive(encodeFrame(ready));
  assert.equal(seen.ready, 1);
  for (const input of [Buffer.alloc(65538, 32), Buffer.from('{}\n'), Buffer.from([255, 10])]) {
    const { value: bad, seen: result } = client();
    bad.receive(input);
    assert.equal(result.failure, 1);
    assert.equal(result.closed, 1);
    assert.equal(result.ready, 0);
  }
});
