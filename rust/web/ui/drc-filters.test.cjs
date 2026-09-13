'use strict';
// Deterministic clock/HTTP: exercise view/selection races without sleeps.
const assert = require('node:assert/strict'), D = require('./drc.js'), P = require('./protocol.js');
const nodes = new Map(), timers = new Map(), raf = new Map(), requests = [], moves = [];
let serial = 0, saved = null, revision = '1', holdList = false, holdStep = false, holdGroups = false, badList = false;
let heldList = null, heldStep = null, heldGroups = null, titleWrites = 0;
const chosen = new Map();
const nativeSet = global.setTimeout, nativeClear = global.clearTimeout;
global.setTimeout = (fn, delay) => { assert.equal(delay, 100); timers.set(++serial, fn); return serial; };
global.clearTimeout = id => timers.delete(id);
const ctx = new Proxy({}, {get: (t,k) => k in t ? t[k] : () => {}});
const rect = {left:10.25,top:20.5,width:64,height:20,right:74.25,bottom:40.5};
class Element {
    constructor(){this.children=[];this.style={};this.checked=false;this.value='';this.hidden=false;this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];} get textContent(){return this.text||'';}
    set innerHTML(_){throw new Error('No HTML insertion');}
    set title(v){this.tip=v;titleWrites++;} get title(){return this.tip||'';}
    appendChild(v){this.children.push(v);return v;} setAttribute(k,v){this[k]=v;}
    getContext(){return ctx;} getBoundingClientRect(){return rect;} focus(){} scrollIntoView(){}
}
const el = id => {if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
el('drc-waived').value='all';el('drc-markers').checked=true;
let context={id:'v1',source:'source',connected:true,pending:false,state:{state_rev:'1',status:'idle',dbu_um:'1',bbox_dbu:['0','0','64','20'],pixels:[128,40]}};
const row=(i,ci='0')=>({check:ci,local:String(i),global:String(i+1),kind:'p',status:i%3===0?1:0,bbox_um:[String(i),'0',String(i+.5),'10'],points:'4'});
const all=Array.from({length:130},(_,i)=>row(i));
function snapshot(){const rules=[...chosen].filter(([,s])=>s.size).sort((a,b)=>P.compare(a[0],b[0])).map(([check,s])=>({check,errors:[...s].sort(P.compare)}));
    return {revision:'r1',view_id:context.id,state:{selection_rev:revision,total:String(rules.reduce((n,r)=>n+r.errors.length,0)),limit:5000,rules}};}
function hold(token,value,set){return new Promise((resolve,reject)=>{set({token,value,resolve:()=>resolve(value),reject});token.abort=()=>{};});}
function http(method,path,envelope,missing,token){
    const q=envelope&&envelope.body;requests.push({method,path,q,envelope,token});
    if(path.endsWith('/selection')){
        if(method==='GET')return holdGroups?hold(token,snapshot(),v=>{heldGroups=v;}):Promise.resolve(snapshot());
        assert.equal(envelope.base_selection_rev,revision);
        if(q.kind==='clear_all')chosen.clear();else{
            const s=q.mode==='replace'?new Set():new Set(chosen.get(q.check)||[]);
            for(const id of new Set(q.errors)){if(q.mode==='toggle'&&s.has(id))s.delete(id);else s.add(id);}chosen.set(q.check,s);
        }
        revision=P.next(revision);return Promise.resolve(snapshot());
    }
    if(!q)return Promise.resolve({drc:{id:'drc',revision:'r1',source_id:'source',title:'test',phase:'ready',metadata:{checks:'2',errors:'132'}}});
    if(q.kind==='rules')return Promise.resolve({rows:[{check:'0',name:'MASK<&>',errors:'130',waived:'44'},{check:'1',name:'OTHER',errors:'2',waived:'0'}],next:null});
    if(q.kind==='rule')return Promise.resolve({name:q.check==='0'?'MASK<&>':'OTHER',errors:'130',waived:'44',description:'test'});
    if(q.kind==='records')return Promise.resolve({rows:q.errors.map(i=>row(Number(i),q.check))});
    if(q.kind==='geometry'){
        const r=row(Number(q.error),q.check),b=r.bbox_um,pts=[[b[0],b[1]],[b[2],b[1]],[b[2],b[3]],[b[0],b[3]]];
        return Promise.resolve({...r,precision:'1',points_dbu:pts.slice(0,q.limit),start:'0',total:'4',next:q.limit<4?String(q.limit):null});
    }
    assert(['list','filtered_step'].includes(q.kind),'filter used an unbounded or legacy request '+q.kind);
    if(q.in_view){assert.equal(envelope.state_rev,context.state.state_rev);assert(!('bbox_um' in q));}
    if(q.selection_rev!==null)assert.equal(q.selection_rev,revision);
    const bounds=q.in_view?context.state.bbox_dbu:null, b=bounds&&bounds.map(Number), ids=chosen.get(q.check)||new Set();
    const matching=(q.check==='0'?all:[row(0,'1'),row(1,'1')]).filter(r=>(q.waived===null||(r.status===1)===q.waived)&&
        (q.selection_rev===null||ids.has(r.local))&&(!b||(Number(r.bbox_um[0])<=b[2]&&Number(r.bbox_um[2])>=b[0]&&0<=b[3]&&10>=b[1])));
    let value={bbox_um:bounds,selection_rev:q.selection_rev,scanned:'130'};
    if(q.kind==='list'){
        const rest=matching.filter(r=>P.compare(r.local,q.start)>=0),rows=rest.slice(0,q.limit);
        value={...value,rows,next:rest.length>q.limit?rest[q.limit].local:null};
        if(badList)value.selection_rev='999';
        return holdList?hold(token,value,v=>{heldList=v;}):Promise.resolve(value);
    }
    const order=q.backwards?[...matching].reverse():matching;
    value={...value,hit:order.find(r=>q.after===null||(q.backwards?P.compare(r.local,q.after)<0:P.compare(r.local,q.after)>0))||order[0]||null,next:null};
    return holdStep?hold(token,value,v=>{heldStep=v;}):Promise.resolve(value);
}
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},protocol:P,rulers:require('./rulers.js'),groups:require('./drc-groups.js'),http,
    window:{requestAnimationFrame:fn=>{raf.set(++serial,fn);return serial;},cancelAnimationFrame:i=>raf.delete(i)},context:()=>context,navigate:n=>moves.push(n),resize(){},
    stateStore:{bind:o=>{let ready=false;return {attach:async()=>{ready=false;await o.apply(saved);ready=true;},change:d=>{if(ready)saved=d;},close(){ready=false;}};}}});
async function tick(){for(let i=0;i<100;i++)await Promise.resolve();}
async function flush(){assert(timers.size<=1,'more than one filter timer');for(const [id,fn] of [...timers]){timers.delete(id);fn();}await tick();}
async function toggle(id,on){el(id).checked=on;el(id).onchange();await tick();}
const list=()=>el('drc-errors').children.map(e=>e['aria-label']);
const listCount=()=>requests.filter(r=>r.q&&r.q.kind==='list').length;
const listIds=()=>list().map(s=>Number(s.match(/Error (\d+),/)[1])-1);
function paint(){for(const [id,fn] of [...raf]){raf.delete(id);fn();}panel.paint(D.projection({bbox_dbu:['0','0','64','20'],width:128,height:40},[0,0],'1'),{pixels:[128,40],dpr:2,left:.25,top:.5});}
(async()=>{
    await panel.init();await tick();assert.equal(list().length,64);paint();
    const reads=requests.length;panel.move(rect.left+10.25,rect.top+15);
    assert.equal(el('viewport').title,'MASK<&> #11 (11)');const writes=titleWrites;
    for(let i=0;i<100;i++)panel.move(rect.left+10.25,rect.top+15);
    assert.equal(titleWrites,writes,'unchanged hover rewrote the DOM');assert.equal(requests.length,reads);
    panel.move(NaN,NaN);assert.equal(el('viewport').title,'');
    await toggle('drc-selected-only',true);assert.deepEqual(listIds(),[]);
    panel.key('n');await tick();assert.match(el('drc-message').textContent,/No matching/);assert.equal(moves.length,0);
    await toggle('drc-selected-only',false);assert.equal(list().length,64);
    const before=listCount();el('drc-errors').children[0].onclick({shiftKey:true});await tick();el('drc-errors').children[2].onclick({shiftKey:true});await tick();
    assert.equal(listCount(),before,'unfiltered group edit reloaded errors');
    el('drc-error-next').onclick();await tick();el('drc-errors').children[1].onclick({shiftKey:true});await tick();
    await toggle('drc-selected-only',true);assert.deepEqual(listIds(),[0,2,65]);
    panel.key('n');await tick();assert.equal(saved.selected.error,'0');panel.key('p');await tick();assert.equal(saved.selected.error,'65');assert.equal(moves.length,0);
    await toggle('drc-in-view',true);assert.deepEqual(listIds(),[0,2]);
    holdStep=true;panel.key('n');await tick();const oldStep=heldStep;
    context={...context,pending:true};panel.contextChanged();assert(oldStep.token.cancelled);assert.deepEqual(listIds(),[]);
    holdStep=false;oldStep.resolve();await tick();
    context={...context,pending:false,state:{...context.state,state_rev:'2',bbox_dbu:['64','0','128','20']}};
    const panReads=listCount();for(let i=0;i<100;i++)panel.contextChanged();assert.equal(listCount(),panReads);assert.equal(timers.size,1);
    await flush();assert.equal(listCount(),panReads+1);assert.deepEqual(listIds(),[65]);assert.equal(moves.length,0);
    holdList=true;el('drc-first').onclick();await tick();const oldSelected=heldList;
    el('drc-group-clear').onclick();await tick();assert(oldSelected.token.cancelled);holdList=false;oldSelected.resolve();await tick();
    await flush();assert.deepEqual(listIds(),[],'old selected response resurrected a removed member');
    await toggle('drc-selected-only',false);assert.equal(list().length,64);assert.equal(timers.size,0,'auto-paginated a dense view');
    el('drc-error-next').onclick();await tick();assert.deepEqual(listIds(),[128]);
    holdList=true;context={...context,state:{...context.state,state_rev:'3',bbox_dbu:['64','0','96','20']}};panel.contextChanged();await flush();const oldView=heldList;
    context={...context,state:{...context.state,state_rev:'4',bbox_dbu:['0','0','10','20']}};panel.contextChanged();assert(oldView.token.cancelled);holdList=false;await flush();
    oldView.resolve();await tick();assert.deepEqual(listIds(),Array.from({length:11},(_,i)=>i));
    const disconnectedReads=listCount();context={...context,connected:false};panel.contextChanged();await flush();assert.equal(listCount(),disconnectedReads);
    context={...context,connected:true};panel.contextChanged();await flush();assert.equal(listCount(),disconnectedReads+1);
    badList=true;el('drc-first').onclick();await tick();assert.deepEqual(listIds(),[]);assert.match(el('drc-message').textContent,/revision mismatch/);assert(el('drc-box').disabled);
    badList=false;el('drc-first').onclick();await tick();el('drc-errors').children[0].onclick({shiftKey:true});await tick();el('drc-errors').children[2].onclick({shiftKey:true});await tick();
    await toggle('drc-selected-only',true);assert.deepEqual(listIds(),[0,2]);assert(saved.in_view&&saved.selected_only);
    holdGroups=true;const countBeforeRestore=listCount(), restoring=el('drc-reload').onclick();await tick();
    assert.equal(listCount(),countBeforeRestore,'Selected restored before authoritative group GET');assert(el('drc-selected-only').disabled);
    // Reload may finish its group/panel GET before WebSocket/resize is ready.
    context={...context,connected:false};holdGroups=false;heldGroups.resolve();await restoring;await tick();
    assert.deepEqual(listIds(),[]);assert(!el('drc-sync-status').textContent.includes('unavailable'));
    context={...context,connected:true};panel.contextChanged();await flush();
    assert.deepEqual(listIds(),[0,2]);assert(el('drc-in-view').checked&&el('drc-selected-only').checked);assert.equal(moves.length,0);
    const beforePendingRestore=listCount();context={...context,pending:true};await el('drc-reload').onclick();await tick();
    assert.equal(listCount(),beforePendingRestore);context={...context,pending:false};panel.contextChanged();await flush();assert.deepEqual(listIds(),[0,2]);
    holdList=true;const conflictingRestore=el('drc-reload').onclick();await tick();const oldRestore=heldList;
    context={...context,state:{...context.state,state_rev:'5'}};panel.contextChanged();holdList=false;
    oldRestore.reject(new Error('drc_context_changed'));await conflictingRestore;await flush();
    assert.deepEqual(listIds(),[0,2]);assert(!el('drc-sync-status').textContent.includes('unavailable'),'normal view conflict broke panel persistence');
    // Waive changes clear groups and intersect before the page limit.
    el('drc-waived').value='waived';el('drc-waived').onchange();await tick();await flush();assert.deepEqual(listIds(),[]);assert.equal(chosen.size,0);
    await toggle('drc-selected-only',false);assert.deepEqual(listIds(),[0,3,6,9]);
    await toggle('drc-in-view',false);assert.equal(list().length,44);const fixed=listCount();
    context={...context,state:{...context.state,state_rev:'6'}};panel.contextChanged();await flush();assert.equal(listCount(),fixed,'disabled live filter still followed pan');
    holdGroups=true;const staleRestore=el('drc-reload').onclick();await tick();const stale=heldGroups;
    context={...context,id:'v2',source:'other'};panel.contextChanged();const boundary=requests.length;stale.resolve();await staleRestore;await tick();assert.equal(requests.length,boundary,'old group GET restarted panel restore');
    assert.deepEqual(listIds(),[]);assert.equal(timers.size,0);panel.stop();assert.equal(raf.size,0);
    console.log('WEB DRC FILTER UI: ALL OK (intersection/empty, circular step, live debounce/no drain, revisions/stale/cancel, restore ordering, hover/no reads)');
})().catch(e=>{console.error(e);process.exitCode=1;}).finally(()=>{global.setTimeout=nativeSet;global.clearTimeout=nativeClear;});
