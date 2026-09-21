'use strict';
const test = require('node:test'), assert = require('node:assert/strict');
const { RecoveryController } = require('./recovery-controller.cjs');
const flush = async () => { for (let n = 0; n < 8; n++) await Promise.resolve(); };
function setup(options = {}) {
  let time = 0, serial = 0;
  const timers = new Map(), events = { reveal: 0, confirm: 0, load: 0, probe: 0, result: [] };
  const r = new RecoveryController({ allowed: () => true, reveal: () => events.reveal++,
    confirm: async () => { events.confirm++; return true; },
    load: async () => { events.load++; r.navigation(); }, probe: async () => { events.probe++; return 'ready'; },
    result: marker => events.result.push(marker), now: () => time,
    setTimer: (fn, delay) => { const id = ++serial; timers.set(id, { at: time + delay, fn }); return id; },
    clearTimer: id => timers.delete(id), ...options });
  return { r, events, timers, setTime: n => { time = n; }, async advance(n) {
    time += n;
    for (const [id, t] of [...timers]) if (t.at <= time && timers.delete(id)) t.fn();
    await flush();
  } };
}
test('only current explicit acceptance starts one GET; cancellation and duplicate requests do nothing', async () => {
  for (const value of [false, undefined, 1]) {
    const h = setup({ confirm: async () => value });
    assert.equal(await h.r.request(), false); assert.equal(h.events.load, 0); assert.equal(h.r.busy, false);
  }
  let approve;
  const h = setup({ confirm: () => new Promise(resolve => { approve = resolve; }) });
  const first = h.r.request(); assert.equal(await h.r.request(), false);
  h.r.invalidate(); approve(true); assert.equal(await first, false); assert.equal(h.events.load, 0);
  h.r.confirm = async () => true;
  assert.equal(await h.r.request(), true); await flush();
  assert.equal(h.events.load, 1); assert.deepEqual(h.events.result, ['ready']); assert.equal(h.timers.size, 0);
});
test('navigation and JS response stalls expire the whole attempt without extra GETs/probes', async () => {
  for (const stage of ['load', 'probe']) {
    let late;
    const h = setup({ [stage]: () => new Promise(resolve => { late = resolve; }) });
    await h.r.request(); await flush();
    assert.equal(h.r.busy, true); assert.equal(await h.r.request(), false);
    await h.advance(30000);
    assert.equal(h.r.busy, false); assert.deepEqual(h.events.result, ['timeout']);
    late('ready'); await flush(); assert.deepEqual(h.events.result, ['timeout']); assert.equal(h.timers.size, 0);
  }
});
test('late success cannot win just before the deadline timer callback is observed', async () => {
  let reply;
  const h = setup({ probe: () => new Promise(resolve => { reply = resolve; }) });
  await h.r.request(); await flush(); h.setTime(30000); reply('ready'); await flush();
  assert.deepEqual(h.events.result, ['timeout']); assert.equal(h.timers.size, 0);
});
test('failed loads permit explicit retry; old callbacks cannot complete a new attempt', async () => {
  const h = setup({ load: async () => { throw new Error('network'); } });
  await h.r.request(); await flush(); assert.deepEqual(h.events.result, ['load-failed']);
  let oldReply, newReply;
  h.r.load = async () => h.r.navigation();
  h.r.probe = () => new Promise(resolve => { oldReply = resolve; });
  await h.r.request(); await flush(); h.r.invalidate();
  h.r.probe = () => new Promise(resolve => { newReply = resolve; });
  await h.r.request(); await flush(); oldReply('restart-required'); await flush();
  assert.equal(h.r.busy, true); newReply('ready'); await flush();
  assert.deepEqual(h.events.result, ['load-failed', 'ready']);
});
test('unready/hidden/error responses do not extend the deadline; hidden auth and lost auth are distinct', async () => {
  for (const marker of ['waiting', 'hidden', 'unknown', null]) {
    const h = setup({ probe: async () => marker });
    await h.r.request(); await flush(); await h.advance(500); await h.advance(29500);
    assert.deepEqual(h.events.result, ['timeout']);
  }
  for (const marker of ['ready-hidden', 'restart-required']) {
    const h = setup({ probe: async () => marker }); await h.r.request(); await flush();
    assert.deepEqual(h.events.result, [marker]); assert.equal(h.timers.size, 0);
  }
});
test('unrelated navigation, closure, and changing availability retire callbacks without replay', async () => {
  for (const action of ['navigation', 'end']) {
    let reply;
    const h = setup({ probe: () => new Promise(resolve => { reply = resolve; }) });
    await h.r.request(); await flush(); h.r[action](); reply('ready'); await flush(); await h.advance(30000);
    assert.deepEqual(h.events.result, []); assert.equal(h.r.busy, false);
  }
  const h = setup({ allowed: () => false });
  assert.equal(await h.r.request(), false); assert.equal(h.events.confirm, 0);
});
test('retired scheduled probe is not issued, and a probe rejection remains bounded', async () => {
  const h = setup();
  h.r.phase = 'checking'; h.r.until = 30000;
  h.r.poll(h.r.epoch); h.r.invalidate(); await flush();
  assert.equal(h.events.probe, 0); assert.deepEqual(h.events.result, []);
  h.r.probe = async () => { h.events.probe++; throw new Error('disposed context'); };
  await h.r.request(); await flush(); await h.advance(30000);
  assert.equal(h.events.probe, 1); assert.deepEqual(h.events.result, ['timeout']);
});
