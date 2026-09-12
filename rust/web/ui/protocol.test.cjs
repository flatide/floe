'use strict';
const assert = require('node:assert/strict');
const p = require('./protocol.js');
for (const invalid of [0, 1, '', '00', '+1', ' 1', '1.0', '1e3', '18446744073709551616']) {
    assert.throws(() => p.counter(invalid, true));
}
assert.equal(p.next('9007199254740992'), '9007199254740993');
assert.equal(p.next('9999999999999999999'), '10000000000000000000');
assert.equal(p.next('0'), '1');
assert.throws(() => p.next('18446744073709551615'));
assert.equal(p.compare('9007199254740992', '9007199254740993'), -1);
for (const invalid of ['NaN', 'Infinity', '', '0x10', ' 0', '1e500']) {
    assert.throws(() => p.decimal(invalid));
}
p.decimal('-10.9375');
assert.throws(() => p.pixels(8192, 8192));
p.pixels(4096, 4096);
const h = {
    type: 'frame', protocol: 1, row0: 'top', purpose: 'foreground', format: 'raw',
    view_id: 'a'.repeat(64), connection_epoch: 'b'.repeat(64), frame_id: '1',
    dataset_revision: '2', state_rev: '3', render_rev: '3', render_key: '1',
    worker_epoch: '1', generation: '4', round: '1', payload_length: '24',
    deferred: '0', deck_skipped: '0', final: true, partial: false, labels_truncated: false,
    complete: true, approximate: false, query: false, width: 2, height: 1,
    bbox_dbu: ['-10.9375', '-1', '1', '2'], perf: {}
};
function frame(header, data) {
    const json = new TextEncoder().encode(JSON.stringify(header));
    const a = new Uint8Array(4 + json.length + data.length);
    new DataView(a.buffer).setUint32(0, json.length, true);
    a.set(json, 4); a.set(data, 4 + json.length);
    return a.buffer;
}
const raw = new Uint8Array(24);
raw.set(new TextEncoder().encode('FLOERAW1'));
const v = new DataView(raw.buffer); v.setUint32(8, 2, true); v.setUint32(12, 1, true);
raw.set([255, 0, 0, 255, 0, 255, 0, 255], 16);
assert.deepEqual(p.packet(frame(h, raw)).data, raw);
const state = Object.assign({}, h, {pixels: [2, 1]});
assert(p.matches(h, state));
for (const k of ['view_id', 'connection_epoch', 'dataset_revision', 'worker_epoch', 'render_rev', 'render_key']) {
    assert(!p.matches(h, Object.assign({}, state, {[k]: 'different'})));
}
assert(!p.matches(h, Object.assign({}, state, {pixels: [1, 2]})));
assert(!p.matches(h, Object.assign({}, state, {bbox_dbu: ['0', '0', '1', '2']})));
// state-only edits may retain identical pixels; render revision is the oracle.
assert(p.matches(h, Object.assign({}, state, {state_rev: '4'})));
for (const change of [{payload_length: '25'}, {width: 3}, {partial: true},
    {row0: 'bottom'}, {worker_epoch: '0'}, {bbox_dbu: ['0', '0', '0', '1']}]) {
    assert.throws(() => p.packet(frame(Object.assign({}, h, change), raw)));
}
const malformed = frame(h, raw);
new DataView(malformed).setUint32(0, 65537, true);
assert.throws(() => p.packet(malformed));
const png = new Uint8Array(33);
png.set([137, 80, 78, 71, 13, 10, 26, 10]);
const pv = new DataView(png.buffer);
pv.setUint32(8, 13); pv.setUint32(12, 0x49484452); pv.setUint32(16, 2); pv.setUint32(20, 1);
p.packet(frame(Object.assign({}, h, {format: 'png', payload_length: '33'}), png));
pv.setUint32(20, 2);
assert.throws(() => p.packet(frame(Object.assign({}, h, {format: 'png', payload_length: '33'}), png)));
assert.equal(p.roundEven(2.5),2);assert.equal(p.roundEven(3.5),4);
assert.equal(p.roundEven(-2.5),-2);assert.equal(p.roundEven(-3.5),-4);
const mh={...h,purpose:'margin',width:196,height:176,bbox_dbu:['-58.9375','-48','137.0625','128']};
const ms={...state,pixels:[100,80],bbox_dbu:['-10.9375','0','89.0625','80'],margin:{frame_id:'1'}};
assert.deepEqual(p.placement(mh,ms),[48,48]);
assert(p.matches(mh,{...ms,render_rev:'99'}));
for(const change of [{render_key:'2'},{connection_epoch:'x'},{margin:null},
    {bbox_dbu:['-9.9375','0','90.0625','80']},{pixels:[101,80]}]) {
    assert(!p.matches(mh,{...ms,...change}));
}
console.log('WEB PROTOCOL: ALL OK (u64, frames, stale identity, margin phase/scale)');
