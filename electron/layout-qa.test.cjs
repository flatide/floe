'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { ownedMemory } = require('./layout-qa.cjs');
test('numeric memory sample includes only the live owned service tree, in any row order', () => {
  const text = ' 22 21 300\n99 1 9000\n21 20 200\n20 10 100\n10 1 50\n';
  assert.deepEqual(ownedMemory(text, 20), { process_count: 3, rss_sum_kib: 600 });
  assert.deepEqual(ownedMemory(text, 22), { process_count: 1, rss_sum_kib: 300 });
});
test('missing service and malformed metrics fail, never report zero-cost success', () => {
  for (const value of ['', '20 1 NaN', '20 1 -1', '20 1 0.5', '20 1 1 extra', '21 1 1']) {
    assert.throws(() => ownedMemory(value, 20));
  }
  assert.throws(() => ownedMemory('20 1 10', 0));
});
