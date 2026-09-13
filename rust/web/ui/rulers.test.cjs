'use strict';
const assert=require('node:assert/strict'), R=require('./rulers.js'), P=require('./protocol.js'), D=require('./drc.js');
const target={check:'0',error:'9007199254740993'};
const segment={endpoints_um:[['10','20'],['30','20']],distance_um:'20',offset:true};
const reply={check:'0',local:target.error,global:'9007199254740994',segments:[segment]};
const s=R.decode(reply,target,P).segments[0];assert.equal(s.label,'Length 20.0000 µm');assert.equal(s.distance,'20');
for(const bad of [{...reply,local:'1'},{...reply,global:9007199254740994},{...reply,segments:[segment,segment,segment,segment]},
    {...reply,segments:[{...segment,distance_um:'NaN'}]},{...reply,segments:[{...segment,distance_um:'0'}]},
    {...reply,segments:[{...segment,offset:1}]},{...reply,segments:[{...segment,endpoints_um:[['0','Infinity'],['1','2']]}]}]) {
    assert.throws(()=>R.decode(bad,target,P));
}
assert.match(R.decode({...reply,segments:[{...segment,distance_um:'1e-10'}]},target,P).segments[0].label,/1.0000e-10/);
assert.deepEqual(R.decode({...reply,segments:[segment,segment]},target,P).segments.map(s=>s.role),['Width','Height']);
assert.deepEqual(R.decode({...reply,segments:[segment,segment,segment]},target,P).segments.map(s=>s.role),['Gap','X','Y']);
assert.deepEqual(R.offset([1,30],[101,30]),[[1,16],[101,16]]);
assert.deepEqual(R.offset([101,30],[1,30]),[[101,16],[1,16]]);
assert.deepEqual(R.offset([1,30],[1,130]),[[15,30],[15,130]]);
assert.deepEqual(R.offset([1,130],[1,30]),[[15,130],[15,30]]);
assert.equal(R.project(s,()=>[Infinity,0],1),null);
assert.equal(R.project({...s,ends:[[-1e308,0],[1e308,0]]},(x,y)=>[x,y],1),null);
for(const dpr of [1,1.25,2,3]) {
    const p=D.projection({bbox_dbu:['0','0','100','80'],width:100*dpr,height:80*dpr},[0,0],'1');
    const actual=R.project(s,(x,y)=>D.point(p,x,y),dpr);
    assert.deepEqual(actual.original,[[10,60],[30,60]]);assert.deepEqual(actual.ends,[[10,46],[30,46]]);
    const moved=R.project(s,(x,y)=>D.point(D.shifted(p,[7*dpr,-9*dpr]),x,y),dpr);
    assert.deepEqual(moved.ends,[[3,55],[23,55]]);
}
assert.deepEqual(R.clipped([-100,20],[200,20],[0,0,100,80]),[[0,20],[100,20]]);
assert.equal(R.clipped([-100,90],[200,90],[0,0,100,80]),null);
const lines=[[[20,100],[180,100]],[[100,20],[100,180]]], placed=[];
for(let i=0;i<2;i++) {
    const r=R.labelSpot(lines[i],110,20,placed,lines.filter((_,j)=>i!==j),[],200,200);
    assert(r);assert(r[0]>=0&&r[1]>=0&&r[2]<=200&&r[3]<=200);
    if(i)assert(r[0]>=placed[0][2]||r[2]<=placed[0][0]||r[1]>=placed[0][3]||r[3]<=placed[0][1]);placed.push(r);
}
assert.equal(R.labelSpot(lines[0],110,20,[],[],[],40,20),null);
const calls=[],ctx=new Proxy({measureText:t=>({width:t.length*6})},{get:(t,k)=>k in t?t[k]:(...v)=>calls.push([k,...v])});
R.paint(ctx,[s],(x,y)=>[x*2,(80-y)*2],{dpr:2,pixels:[400,200]});
assert(calls.some(c=>c[0]==='setTransform'&&c[1]===2));assert(calls.some(c=>c[0]==='fillText'&&c[1]===s.label));
assert.equal(calls.at(-1)[0],'restore');
console.log('WEB CD RULERS: ALL OK (bounded DTO, u64, stable 14 CSS px, DPR/crop, clipping, readable labels)');
