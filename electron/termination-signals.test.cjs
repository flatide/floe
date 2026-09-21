'use strict';
const test = require('node:test'), assert = require('node:assert/strict');
const { EventEmitter } = require('node:events');
const { terminationSignals } = require('./termination-signals.cjs');
test('rearm replaces only its own watchers and never duplicates callbacks', () => {
  const process = new EventEmitter(), calls = [], other = [];
  const rearm = terminationSignals(process, signal => calls.push(signal));
  process.on('SIGINT', () => other.push('other'));
  for (let i = 0; i < 3; i++) rearm();
  assert.equal(process.listenerCount('SIGINT'), 2);
  assert.equal(process.listenerCount('SIGTERM'), 1);
  assert.deepEqual(calls, []); // installation must never authorize a shutdown
  process.emit('SIGINT'); process.emit('SIGTERM');
  assert.deepEqual(calls, ['SIGINT', 'SIGTERM']);
  assert.deepEqual(other, ['other']);
});
