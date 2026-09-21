'use strict';
const assert = require('assert/strict');
const T = require('./display-timing.js');
let time = 100, calls = 0;
const r = T.create(() => { ++calls; return time; });
assert.equal(r.now(), null); assert.equal(r.beginEdit('pan'), null); assert.equal(calls, 0);
assert(r.enable(true));
const edit = r.beginEdit('pan'); time += 65; r.sent(edit); time += 10; r.ack(edit); time += 5; r.endEdit(edit, true);
assert.deepEqual(r.snapshot().events[0], {kind:'edit',outcome:'accepted',at_ms:80,elapsed_ms:80,action:'pan',queue_ms:65,
    send_to_ack_ms:10,ack_to_snapshot_ms:5,queue_to_snapshot_ms:80});
r.endEdit(edit, true); assert.equal(r.snapshot().events.length, 1);
const header = {purpose:'margin',format:'raw',final:true,view_id:'secret-id',bbox_dbu:['private'],perf:{path:'private'}};
const received = r.now(); time += 3;
const frame = r.beginFrame(header, received); time += 12; r.decoded(frame); time += 4; r.endFrame(frame, true);
assert.deepEqual(r.snapshot().events[1], {kind:'frame',outcome:'submitted',at_ms:99,elapsed_ms:19,purpose:'margin',format:'raw',final:true,
    packet_ms:3,decode_ms:12,submit_ms:4,receive_to_submit_ms:19});
let start=r.now(); time+=16; r.preview(start,'pan','raf'); start=r.now();time+=10;r.preview(start,'pan','raf');
assert.equal(r.snapshot().events[3].submit_interval_ms,10);
r.endGesture();start=r.now();time+=1000;r.preview(start,'pan','release');
assert.equal(r.snapshot().events[4].submit_interval_ms,null);
const late=r.beginFrame(header);r.enable(false);r.decoded(late);r.endFrame(late,true);
assert.equal(r.snapshot().events.at(-1).outcome,'interrupted');
assert.equal(r.snapshot().events.at(-1).receive_to_submit_ms,null);
const n=calls;r.now();r.beginEdit('private');r.beginFrame(header);assert.equal(calls,n);
r.enable(true);r.endFrame(late,true);assert.equal(r.snapshot().events.length,0);
for(let i=0;i<300;i++){const t=r.beginEdit('private');time++;r.endEdit(t,false);}
assert.equal(r.snapshot().events.length,256);assert.equal(r.snapshot().overwritten,44);
assert(r.snapshot().events.every((e,i,a)=>!i||e.at_ms>a[i-1].at_ms));
assert(!JSON.stringify(r.snapshot()).includes('private'));assert(!JSON.stringify(r.snapshot()).includes('secret'));
const copy=r.snapshot();copy.events[0].action='mutated';assert.notEqual(r.snapshot().events[0].action,'mutated');
r.clear();for(let i=0;i<130;i++){r.beginEdit('pan');}
assert.equal(r.snapshot().pending,128);assert.equal(r.snapshot().untracked,2);
r.interrupt();assert.equal(r.snapshot().pending,0);assert.equal(r.snapshot().events.length,128);
const stale=r.beginEdit('fit');r.clear();r.sent(stale);r.ack(stale);r.endEdit(stale,true);assert.equal(r.snapshot().events.length,0);
for(const bad of [undefined,null,NaN,Infinity,-1,time+1]){
    const t=r.beginFrame(header,bad);r.decoded(t);r.endFrame(t,true);
    const row=r.snapshot().events.at(-1);assert.equal(row.packet_ms,null);assert.equal(row.receive_to_submit_ms,null);
    assert.equal(row.decode_ms,0);assert.equal(row.submit_ms,0);
}
assert.equal(T.create(null).enable(true),false);assert.equal(T.create(()=>NaN).enable(true),false);
const nodes=new Map();const el=id=>{if(!nodes.has(id)){nodes.set(id,{addEventListener(k,f){this[k]=f;}});}return nodes.get(id);};
const bound=T.bind({el,window:{performance:{now:()=>time}}});
assert.equal(el('timing-enabled').checked,false);el('timing-enabled').checked=true;el('timing-enabled').change();
const t=bound.beginEdit('pan');time++;bound.endEdit(t,true);assert.equal(el('timing-report').textContent.includes('pan'),false);
el('timing-refresh').onclick();assert.equal(JSON.parse(el('timing-report').textContent).events.length,1);
bound.stop();assert.equal(el('timing-enabled').checked,false);el('timing-clear').onclick();assert.equal(bound.snapshot().events.length,0);
console.log('BROWSER TIMING: OK (opt-in, monotonic boundaries, privacy, bounded storage, interrupted/stale callbacks, explicit reports; not photon)');
