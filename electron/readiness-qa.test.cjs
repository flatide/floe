'use strict';
const test = require('node:test'), assert = require('node:assert/strict');
const vm = require('node:vm');
const { probe, waitReady } = require('./readiness-qa.cjs');
test('readiness returns only bits, separating visibility from exchange/UI readiness', () => {
  const state = { document: { hidden: false, getElementById: () => ({ disabled: false }) }, location: { hash: '' } };
  assert.equal(vm.runInNewContext(probe, state), 15);
  state.document.hidden = true;
  assert.equal(vm.runInNewContext(probe, state), 14);
  state.location.hash = '#bootstrap=synthetic-do-not-echo';
  assert.equal(vm.runInNewContext(probe, state), 12);
  state.document.getElementById = () => ({ disabled: true });
  assert.equal(vm.runInNewContext(probe, state), 4);
  state.document.getElementById = () => null;
  assert.equal(vm.runInNewContext(probe, state), 0);
});
test('ready requires all bits, never echoes unexpected renderer data', async () => {
  const observations = [], replies = ['#bootstrap=synthetic-do-not-echo', 14, 15];
  await waitReady({ evaluate: async () => replies.shift(), alive: () => true, observe: value => observations.push(value) });
  assert.deepEqual(observations, [null, 14, 15]);
});
test('stalled renderer is bounded; late completion cannot become ready', async () => {
  let reply, probes = 0;
  const seen = [];
  await assert.rejects(waitReady({ evaluate: () => { probes++; return new Promise(resolve => { reply = resolve; }); },
    alive: () => true, observe: value => seen.push(value), timeout: 20 }));
  reply(15); await new Promise(resolve => setImmediate(resolve));
  assert.equal(probes, 1); assert.deepEqual(seen, []);
});
test('ready after a blocked event-loop deadline cannot beat the timer callback', async () => {
  const seen = [];
  await assert.rejects(waitReady({ evaluate: async () => {
    const end = performance.now() + 25;
    while (performance.now() < end) { /* deliberately block the timer in this unit test */ }
    return 15;
  }, alive: () => true, observe: value => seen.push(value), timeout: 10 }));
  assert.deepEqual(seen, []);
});
test('session termination and probe rejection fail without retry', async () => {
  let calls = 0;
  await assert.rejects(waitReady({ evaluate: async () => { calls++; return 15; }, alive: () => false, observe: () => {} }));
  assert.equal(calls, 0);
  await assert.rejects(waitReady({ evaluate: async () => { calls++; throw Error('probe failed'); }, alive: () => true, observe: () => {} }));
  assert.equal(calls, 1);
});
