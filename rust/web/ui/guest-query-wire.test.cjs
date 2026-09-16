'use strict';
const assert=require('node:assert/strict'),P=require('./protocol.js'),W=require('./guest-query-wire.js');
let seq='0',context={id:'view',state:{connection_epoch:'epoch'},connected:true},ruler=false;
const sent=[],seen={inspect:[],measure:[]};
const wire=W.bind({protocol:P,context:()=>context,send:m=>{m.seq=seq=P.next(seq);sent.push(m);return seq;},
    allowed:(tool,m)=>!(ruler&&tool==='inspect'&&(m.kind==='snap'||m.body&&m.body.operation.kind==='snap'))});
wire.receiver('inspect',m=>seen.inspect.push(m));wire.receiver('measure',m=>seen.measure.push(m));
const i=wire.sender('inspect'),r=wire.sender('measure');
const base={view_id:'view',connection_epoch:'epoch'},pick={...base,type:'view.query',body:{operation:{kind:'pick'}}},snap={...base,type:'view.query',body:{operation:{kind:'snap'}}};
const a=i(pick),b=r(snap);assert.equal(pick.type,'view.query');assert.equal(sent[0].type,'explore.query');
wire.receive({type:'query.accepted',seq:a});wire.receive({type:'query.accepted',seq:b});
wire.receive({type:'query.result',seq:a});wire.receive({type:'query.result',seq:b});
assert.deepEqual(seen.inspect.map(m=>m.seq),[a,a]);assert.deepEqual(seen.measure.map(m=>m.seq),[b,b]);
wire.receive({type:'query.result',seq:a});assert.equal(seen.inspect.length,2,'do not replay completed results');
const c=r({...base,type:'view.measure'});wire.receive({type:'measure.result',seq:c});assert.equal(sent.at(-1).type,'explore.measure');
const d=r({...base,type:'view.measure_selection'});wire.receive({type:'error',seq:d});assert.equal(seen.measure.at(-1).seq,d);
assert.equal(wire.receive({type:'accepted',seq:'99'}),false);assert.equal(wire.receive({type:'error',seq:'99'}),false);
assert.equal(wire.receive({type:'query.result',seq:'99'}),true,'expired query responses cannot change a new tool');
for(const type of ['view.set','view.clip.prepare','owner.shutdown','explore.set','toString','constructor','__proto__']){assert.throws(()=>i({...base,type}));}
assert.throws(()=>i({...base,type:'view.measure'}));assert.throws(()=>r(pick));
assert.throws(()=>i({...snap,connection_epoch:'old'}));
ruler=true;assert.throws(()=>i(snap));assert.throws(()=>i({...base,type:'view.query.cancel',kind:'snap'}));
const e=r({...base,type:'view.query.cancel',kind:'snap'});wire.receive({type:'query.cancelled',seq:e});
assert.equal(sent.at(-1).type,'explore.query.cancel');context=null;assert.throws(()=>r({...base,type:'view.measure'}));
wire.reset();assert.equal(wire.receive({type:'error',seq:d}),false,'old epoch ownership is forgotten');
console.log('GUEST QUERY WIRE: ALL OK (four mapped commands, per-tool replies, no owner routing/replay, snap ownership and epoch reset)');
