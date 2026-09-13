'use strict';
const assert = require('node:assert/strict'), Q = require('./query.js'), P = require('./protocol.js');
const clone = v => JSON.parse(JSON.stringify(v));
function context() {
    const state = {view_id:'a'.repeat(64),connection_epoch:'b'.repeat(64),dataset_revision:'9007199254740993',worker_epoch:'2',
        state_rev:'5',render_rev:'5',render_key:'3',bbox_dbu:['-10','0','90','80'],pixels:[100,80],status:'idle',capabilities:{query:true}};
    const frame = {...state,frame_id:'9007199254740995',purpose:'foreground',width:100,height:80,final:true,query:true,
        query_scene:{generation:'3',round:'1',complete:true,summary_layers:'0'}};
    return {id:state.view_id,state,frame,origin:[0,0],acked:true,connected:true,pending:false,hidden:false,
        size:{pixels:[100,80],dpr:2,left:0.25,top:0.125},rect:{left:20,top:30}};
}
function harness() {
    let c=context(),time=100,sequence='9007199254740992',timer=0;
    const timers=new Map(), sent=[], received=[];
    const q=Q.bind({protocol:P,context:()=>c,send:v=>{v.seq=sequence=P.next(sequence);sent.push(clone(v));return sequence;},now:()=>time,
        setTimeout:(fn,ms)=>{timers.set(++timer,{fn,at:time+ms});return timer;},clearTimeout:id=>timers.delete(id)});
    const tick=ms=>{const end=time+ms;let n=0;while(true){const next=[...timers].sort((a,b)=>a[1].at-b[1].at)[0];if(!next||next[1].at>end)break;
        assert(++n<1000);time=next[1].at;timers.delete(next[0]);next[1].fn();}time=end;};
    return {q,sent,received,timers,tick,get c(){return c;},set c(v){c=v;},
        request:(kind='pick',p=[.5,.5],nth='0')=>q.request(kind,p,5,nth,v=>received.push(v)),
        answer:(t=sent.filter(v=>v.type==='view.query').at(-1),hit=null,status='ok')=>{
            q.receive({type:'query.accepted',seq:t.seq,view_id:t.view_id,connection_epoch:t.connection_epoch,query_id:'9007199254740997'});
            return {type:'query.result',seq:t.seq,view_id:t.view_id,connection_epoch:t.connection_epoch,query_id:'9007199254740997',anchor:t.body.anchor,
                status,hit,scene:{generation:'9',round:'2',complete:true,summary_layers:'0'},requested_summary_layers:'0'};
        }};
}
const s=Q.scope(context(),P);assert.equal(s.anchor.frame_id,'9007199254740995');assert.equal(s.anchor.state_rev,'5');
assert.deepEqual(Q.position(context(),45.25,50.125),[.5,.5]);
assert.equal(Q.position(context(),20,30),null,'fractional CSS origin is not the first device pixel');
for(const n of [NaN,Infinity,-Infinity])assert.equal(Q.position(context(),n,40),null);
for(const change of [c=>c.pending=true,c=>c.hidden=true,c=>c.connected=false,c=>c.acked=false,c=>c.frame.query=false,
    c=>c.state.capabilities.query=false,c=>c.size.pixels=[101,80],c=>c.state.worker_epoch='3',c=>c.state.render_rev='6',
    c=>c.origin=[-1,0],c=>c.origin=[1,0],c=>c.state.status='failed',c=>c.size.dpr=Infinity,c=>c.id='wrong',c=>c.rect.left=NaN]){const c=context();change(c);assert.equal(Q.scope(c,P),null);}
const margin=context();Object.assign(margin.frame,{purpose:'margin',width:196,height:176,bbox_dbu:['-58','-48','138','128'],state_rev:'1',render_rev:'1'});
margin.origin=[48,48];margin.state.margin={frame_id:margin.frame.frame_id};assert(Q.scope(margin,P));
margin.state.state_rev='6';margin.state.render_rev='6';margin.state.bbox_dbu=['38','0','138','80'];margin.origin=[96,48];
assert.equal(Q.scope(margin,P).anchor.state_rev,'6');assert.equal(Q.scope(margin,P).anchor.frame_id,'9007199254740995');
assert.deepEqual(Q.position(margin,45.25,50.125),[.5,.5],'input fractions must not include margin origin');
margin.frame.labels_truncated=true;assert(Q.scope(margin,P),'labels partial is not geometry incomplete');
for(const v of [0,'-0','+1','01','1.0','9223372036854775808','-9223372036854775809'])assert.throws(()=>Q.i64(v,P));
for(const v of ['0','-1','9007199254740993','9223372036854775807','-9223372036854775808'])assert.equal(Q.i64(v,P),v);
const shape={kind:'pick',count:'2',index:'0',pair:[7,0],layer_name:'<img src=x onerror=alert(1)>',cell_name:'칩 <script>',area_dbu2:'123.5',
    bbox_dbu:['9007199254740992','-5','9007199254740994','5'],points_dbu:[['9007199254740993','0']],points_truncated:true};
assert.equal(Q.hit(shape,'pick',P).points_dbu[0][0],'9007199254740993');
for(const edit of [v=>v.count='65',v=>v.index='2',v=>v.area_dbu2='NaN',v=>v.area_dbu2='-1',v=>v.pair=[-1,0],
    v=>v.bbox_dbu=['2','0','1','1'],v=>v.points_dbu=[['9007199254740995','0']],v=>v.points_truncated='true',v=>v.extra='path',
    v=>v.points_dbu=new Array(513).fill(['9007199254740993','0']),v=>v.cell_name='x'.repeat(65537)]){const v=clone(shape);edit(v);assert.throws(()=>Q.hit(v,'pick',P));}
assert(Q.hit({...shape,points_dbu:new Array(512).fill(['9007199254740993','0'])},'pick',P));
for(const v of [{kind:'snap',snap:'face',point_dbu:['1','2']},{kind:'snap',snap:'edge',point_dbu:[1,2]}])assert.throws(()=>Q.hit(v,'snap',P));
{
    const h=harness();assert(h.request());const t=h.sent[0];assert.equal(t.seq,'9007199254740993');assert.deepEqual(t.body.layers,{mode:'all'});
    assert(h.q.receive(h.answer(t,shape)));assert.equal(h.received[0].hit,shape);assert.equal(h.timers.size,0);
    h.q.receive(h.answer(t,shape));assert.equal(h.received.length,1,'duplicate terminal was reapplied');
    h.request('snap');h.tick(80);h.q.receive(h.answer(undefined,{kind:'snap',point_dbu:['1','2'],snap:'vertex'}));
    assert.equal(h.received[1].hit.kind,'snap');h.q.stop();assert.equal(h.timers.size,0);
}
{
    const h=harness();h.request('snap');const old=h.sent[0];
    for(let i=0;i<500;i++)h.request('snap',[i/500,.5]);
    assert.equal(h.sent.length,1);assert(h.timers.size<=1,'hover created an unbounded timer queue');
    h.q.receive(h.answer(old,{kind:'snap',point_dbu:['1','2'],snap:'edge'}));assert.equal(h.received.length,0);
    h.tick(80);assert.equal(h.sent.length,2);assert.equal(h.sent[1].body.position[0],499/500);
    h.q.receive(h.answer(undefined,null));assert.equal(h.received.length,1);h.q.stop();assert.equal(h.timers.size,0);
}
for(const change of [c=>c.state.state_rev='6',c=>c.state.render_rev='6',c=>c.frame.frame_id='99',c=>c.state.worker_epoch='3',
    c=>c.size.dpr=1,c=>c.rect.top++,c=>c.size.left+=.1,c=>c.pending=true,c=>c.hidden=true,c=>c.connected=false,c=>c.acked=false]){
    const h=harness();h.request();const r=h.answer(undefined,shape);change(h.c);h.q.changed();h.q.receive(r);
    assert.equal(h.received.length,0);h.tick(10000);assert.equal(h.received.length,0);assert.equal(h.timers.size,0);
}
for(const edit of [r=>r.query_id='9007199254740998',r=>r.view_id='f'.repeat(64),r=>r.connection_epoch='e'.repeat(64),
    r=>r.anchor={...r.anchor,worker_epoch:'3'},r=>r.status='unknown',r=>r.scene.complete=false,r=>r.requested_summary_layers='1',
    r=>r.hit={...shape,points_truncated:undefined},r=>delete r.scene,r=>r.secret='/private/path']){
    const h=harness();h.request();const r=h.answer(undefined,clone(shape));edit(r);h.q.receive(r);
    assert.equal(h.received.length,1);assert.equal(h.received[0].status,'invalid');assert.equal(h.received[0].hit,null);h.q.stop();
}
{
    const h=harness();h.request();const r=h.answer(undefined,null,'scene_summary');r.scene.summary_layers='2';r.requested_summary_layers='1';h.q.receive(r);
    assert.equal(h.received[0].status,'scene_summary');assert.match(h.received[0].message,/summaries/);
    h.tick(80);h.request();h.q.receive(h.answer());assert.deepEqual(h.received[1],{status:'ok',hit:null,message:''});
    h.tick(80);h.request();h.q.receive({type:'error',seq:h.sent.at(-1).seq,code:'/secret/path'});assert(!h.received[2].message.includes('/secret'));
    h.tick(80);h.request();h.tick(8000);assert.equal(h.received.at(-1).status,'timeout');assert.equal(h.sent.at(-1).type,'view.query.cancel');assert.equal(h.timers.size,0);
}
{
    const h=harness();assert(!h.request('pick',[]));assert(!h.request('pick',[2,0]));assert(!h.request('pick',[NaN,0]));
    h.request('snap');h.tick(80);h.request('pick',[.4,.4],'-1');const p=h.sent.at(-1);h.q.cancel('snap');
    h.q.receive(h.answer(p,shape));assert.equal(h.received[0].hit.kind,'pick','snap cancel changed pick');h.q.stop();
    assert(!h.request());h.q.resume();assert(h.request());h.q.stop();assert.equal(h.timers.size,0);
}
{
    const h=harness();h.request('snap');h.request('snap',[.6,.5]);h.q.cancel('snap');
    assert.equal(h.sent.at(-1).type,'view.query.cancel','queued replacement forgot the already sent query');assert.equal(h.timers.size,0);
    const t=h.sent[0];h.q.receive(h.answer(t,{kind:'snap',point_dbu:['1','2'],snap:'edge'}));assert.equal(h.received.length,0);
}
for(const code of ['__proto__','toString','constructor']){
    const h=harness();h.request();h.q.receive({type:'error',seq:h.sent.at(-1).seq,code});
    assert.equal(h.received[0].message,'Query was refused.');h.q.stop();
}
console.log('WEB QUERIES: ALL OK (display receipt/DPR/crop, strict bounded DTO, u64/i64, latest/throttle, stale/cancel/timeout, summary/empty, cleanup)');
