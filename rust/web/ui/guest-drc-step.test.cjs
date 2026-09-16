'use strict';
const assert=require('node:assert/strict'),S=require('./guest-drc-step.js'),P=require('./protocol.js');
const request={kind:'filtered_step',check:'0',backwards:false,after:'9007199254740993',cursor:null,waived:null,in_view:false,selection_rev:null};
const hit={check:'0',local:'9007199254740994',global:'9007199254740995',status:0,bbox_um:['1','2','3','4']};
const page={hit,next:null,scanned:'1',bbox_um:null,selection_rev:null};
function decode(p=page,r=request,member=true){return S.decode(p,r,P,v=>{P.counter(v.local,true);return v;},()=>member);}
assert.equal(decode().hit.local,hit.local);assert.deepEqual(decode({...page,hit:null}),{hit:null,continuation:null});
const yielded={...page,hit:null,scanned:'262144',next:{next:'9007199254740999',remaining:'9007199254740989'}};
const next=decode(yielded).continuation;assert.equal(next.after,null);assert.deepEqual(next.cursor,yielded.next);assert.equal(request.cursor,null);
assert.equal(decode({...yielded,next:{next:'0',remaining:'1'}},next).continuation.cursor.next,'0');
for(const [p,r] of [
    [{...page,scanned:'262145'},request],[{...page,next:yielded.next},request],[{...yielded,scanned:'0'},request],
    [{...yielded,next:{next:'1',remaining:'0'}},request],[yielded,next],[{...page,selection_rev:'2'},request],
    [{...page,bbox_um:['0','0','5','5']},request],[page,{...request,in_view:true}],
    [{...page,hit:{...hit,check:'1'}},request],[page,{...request,waived:true}]
]){assert.throws(()=>decode(p,r));}
const selected={...request,selection_rev:'3'},sp={...page,selection_rev:'3'};
assert.equal(decode(sp,selected).hit,hit);assert.throws(()=>decode(sp,selected,false));assert.throws(()=>decode({...yielded,selection_rev:'3'},selected));
const local={...request,in_view:true},vp={...page,bbox_um:['0','0','10','10']};assert.equal(decode(vp,local).hit,hit);
assert.throws(()=>decode({...vp,bbox_um:['4','5','6','7']},local));assert.throws(()=>decode({...vp,bbox_um:['0','0','-1','2']},local));
assert.equal(decode({...vp,bbox_um:['3','4','6','7']},local).hit,hit,'touching boundary remains inclusive');
console.log('GUEST DRC STEP: ALL OK (bounded scan, u64 wrap/progress, explicit continuation, selected/status/view filters)');
