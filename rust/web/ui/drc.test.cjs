'use strict';
// Simulated DOM/HTTP only. Actual browser QA is a separate gate.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const D = require('./drc.js'), P = require('./protocol.js');
const frame = {bbox_dbu:['-48','-48','148','128'],width:196,height:176};
assert.equal(D.projection(frame,[48,48]),null,'native frame has no DBU field');
const base = D.projection(frame,[48,48],'1');
assert.deepEqual(D.point(base,20,30),[20,50]);
assert.deepEqual(D.point(D.shifted(base,[48,0]),20,30),[-28,50]);
assert.deepEqual(D.point(D.shifted(base,[13,-11]),20,30),[7,61]);
assert.deepEqual(D.point(D.shifted(base,[-10,-8]),20,30),[30,58]);
const calls=[], drawing=[], raf=new Map(), nodes=new Map(); let serial=0, resize=0;
const ctx = new Proxy({}, {get(target,key){if(key in target)return target[key];return (...args)=>drawing.push([key,...args]);},set(target,key,v){target[key]=v;return true;}});
class Element {
    constructor(tag='div'){this.tag=tag;this.hidden=false;this.value='';this.checked=false;this.disabled=false;this.style={};this.children=[];this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];} get textContent(){return this.text||'';}
    set innerHTML(_){throw new Error('DRC must not insert HTML');}
    appendChild(v){this.children.push(v);return v;}
    setAttribute(k,v){this[k]=v;}
    getContext(){return ctx;}
}
for(const m of fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)){nodes.set(m[1],new Element());}
const el=id=>nodes.get(id);
el('drc-waived').value='all';el('drc-markers').checked=true;
const doc={getElementById:el,createElement:tag=>new Element(tag)};
const window={requestAnimationFrame:fn=>{raf.set(++serial,fn);return serial;},cancelAnimationFrame:id=>raf.delete(id)};
let state={state_rev:'1',status:'idle',bbox_dbu:['0','0','100','80'],dbu_um:'1',pixels:[100,80]};
let view={id:'view-a',source:'source',state,connected:true,pending:false};
const nav=[];
let savedPanel=null;const savedChanges=[];
// Browser timers reject a DTO object as their receiver (Node timers do not).
const nativeSetTimeout=global.setTimeout,nativeClearTimeout=global.clearTimeout;
global.setTimeout=function(...args){assert(!this || !this.http,'unbound browser timer receiver');return nativeSetTimeout(...args);};
global.clearTimeout=function(...args){assert(!this || !this.http,'unbound browser timer receiver');return nativeClearTimeout(...args);};
function http(method,path,body,missing,token){return new Promise((resolve,reject)=>{
    const req={method,path,body,resolve,reject,token,done:false};calls.push(req);
    if(token)token.abort=()=>{req.done=true;reject(new Error('aborted'));};
});}
function pending(kind){const call=calls.find(c=>!c.done&&(kind==='catalog'?c.path==='/api/v1/drc':c.body&&c.body.body.kind===kind));assert(call,'no pending '+kind);return call;}
function reply(kind,value){const c=pending(kind);c.done=true;if(c.token)c.token.abort=null;c.resolve(value);return c;}
async function tick(){for(let i=0;i<12;i++)await Promise.resolve();}
function paint(){for(const [id,fn] of [...raf]){raf.delete(id);fn();}}
const panel=D.bind({document:doc,window,protocol:P,http,context:()=>view,navigate:n=>nav.push(n),resize:()=>resize++,
    stateStore:{bind:o=>{o.clearTimeout(o.setTimeout(()=>{},0));let ready=false;return {attach:async()=>{ready=false;await o.apply(savedPanel);ready=true;},
        change:d=>{if(ready)savedChanges.push(d);},close(){ready=false;}};}}});
const a={check:'0',local:'9007199254740993',global:'9007199254740994',kind:'p',status:0,bbox_um:['10','10','30','30'],points:'5000'};
const b={check:'0',local:'9007199254740994',global:'9007199254740995',kind:'e',status:1,bbox_um:['40','10','60','30'],points:'2'};
const geom=(r,pts,start,total,next)=>({check:r.check,local:r.local,global:r.global,kind:r.kind,status:r.status,bbox_um:r.bbox_um,
    precision:'1000',points_dbu:pts,start:String(start),total:String(total),next});
(async()=>{
    const initialized=panel.init();
    reply('catalog',{drc:{id:'drc-id',revision:'r1',source_id:'source',title:'synthetic',phase:'ready',metadata:{checks:'1',errors:'9007199254740996'}}});
    await initialized;
    assert.equal(el('drc-panel').hidden,false);
    reply('rules',{rows:[{check:'0',name:'MASK <img src=x>',name_truncated:false,errors:'9007199254740996',waived:'1'}],next:null});
    await tick();
    assert.equal(el('drc-rules').children[0].textContent,'MASK <img src=x>  ·  9007199254740996');
    reply('rule',{description:'<script>not HTML</script>'});
    reply('errors',{rows:[a,b],next:'9007199254740995'});await tick();
    assert.equal(el('drc-description').textContent,'<script>not HTML</script>');
    assert(el('drc-errors').children[0].textContent.includes('#9007199254740994'));
    panel.paint(base,{pixels:[100,80],dpr:2,left:.5,top:0});
    assert.equal(el('drc-canvas').style.width,'50px');
    el('drc-errors').children[0].onclick();
    const first=pending('focus');assert.equal(first.body.state_rev,'1');assert.equal(first.body.body.fit,true);
    reply('geometry',geom(a,new Array(2048).fill(['10000','10000']),0,5000,'2048'));await tick();
    paint();assert(drawing.some(c=>c[0]==='strokeRect'));assert(!drawing.some(c=>c[0]==='closePath'),'incomplete polygon was closed');
    // Replacing a selection aborts its in-flight geometry/focus and no late
    // page can populate the new selection's typed array.
    const old=pending('geometry');el('drc-errors').children[1].onclick();
    assert(old.token.cancelled);assert(first.token.cancelled);
    old.resolve(geom(a,[['0','0']],2048,5000,null));await tick();
    reply('geometry',geom(b,[['40000','10000'],['60000','30000']],0,2,null));
    reply('focus',{navigation:{kind:'goto',center_um:['50','20'],width_um:'66.66666666666667'}});await tick();
    assert.equal(nav.length,1);assert.equal(nav[0].center_um[0],'50');
    paint();assert(drawing.some(c=>c[0]==='lineTo'));assert(!drawing.some(c=>c[0]==='closePath'));
    // A user zoom sticks on the next selection; Frame error resets it.
    state={...state,state_rev:'2',bbox_dbu:['0','0','20','16']};view={...view,state};panel.contextChanged();
    el('drc-errors').children[0].onclick();assert.equal(pending('focus').body.body.fit,false);
    state={...state,state_rev:'3'};view={...view,state};
    reply('focus',{navigation:{kind:'goto',center_um:['20','20'],width_um:'20'}});await tick();
    assert.equal(nav.length,1,'late focus moved a changed view');
    el('drc-frame').onclick();assert.equal(pending('focus').body.body.fit,true);
    reply('focus',{navigation:{kind:'goto',center_um:['20','20'],width_um:'66.66666666666667'}});await tick();
    assert.equal(nav.length,2);
    // Full 5k polygon assembled only after its final page; selected array is
    // bounded by the existing core record limit, not an unbounded fetch loop.
    reply('geometry',geom(a,new Array(2048).fill(['10000','10000']),0,5000,'2048'));await tick();
    reply('geometry',geom(a,new Array(2048).fill(['30000','10000']),2048,5000,'4096'));await tick();
    reply('geometry',geom(a,new Array(904).fill(['30000','30000']),4096,5000,null));await tick();paint();
    assert(drawing.some(c=>c[0]==='closePath'));assert(el('drc-selected').textContent.includes('5000/5000'));
    el('drc-waived').value='waived';el('drc-waived').onchange();assert.equal(pending('errors').body.body.waived,true);
    reply('errors',{rows:[b],next:null});await tick();assert.equal(el('drc-error-next').disabled,true);
    // In-view bounds are computed by the server, not rounded in the browser.
    el('drc-in-view').onclick();const q=pending('in_view');assert.equal(q.body.state_rev,'3');assert(!('bbox_um' in q.body.body));
    reply('in_view',{rows:[],next:{check:'0',error:'100'},bbox_um:['-0.125','0','19.875','16']});await tick();
    assert.equal(el('drc-error-next').disabled,false);assert(el('drc-result-info').textContent.includes('more available'));
    el('drc-error-next').onclick();assert.deepEqual(pending('query').body.body.bbox_um,['-0.125','0','19.875','16']);
    reply('query',{rows:[b],next:null});await tick();
    el('drc-error-prev').onclick();reply('query',{rows:[],next:{check:'0',error:'100'}});await tick();
    state={...state,state_rev:'4'};view={...view,state};panel.contextChanged();assert(el('drc-result-info').textContent.includes('earlier viewport'));
    el('drc-toggle').onclick();assert.equal(resize,1);assert.equal(el('drc-panel').hidden,true);
    // A different source drops rows and all pending requests immediately.
    el('drc-error-next').onclick();const stale=pending('query');view={...view,source:'other',id:'view-b'};panel.contextChanged();
    assert(stale.token.cancelled);stale.resolve({rows:[a],next:null});await tick();paint();
    assert.equal(el('drc-canvas').hidden,true);assert.equal(el('drc-errors').children.length,0);
    // Restoring server state reloads bounded pages/selected geometry without
    // running a focus/goto or writing the restored state back as a new edit.
    savedPanel={search:'MASK',rule_start:'0',check:'0',error_start:'0',query:null,waived:true,
        selected:{check:b.check,error:b.local},markers:false,shown:true,jump_scale:'0.2',zoom_lock:true};
    const beforeRestoreNav=nav.length,beforeRestoreSaves=savedChanges.length;
    view={...view,source:'source',id:'view-c'};panel.contextChanged();
    reply('rules',{rows:[{check:'0',name:'MASK',name_truncated:false,errors:'9007199254740996',waived:'1'}],next:null});await tick();
    reply('rule',{name:'MASK',description:'Restored description',errors:'9007199254740996',waived:'1'});await tick();
    assert.equal(pending('errors').body.body.waived,true);reply('errors',{rows:[b],next:null});await tick();
    assert.equal(pending('geometry').body.body.limit,1);
    reply('geometry',geom(b,[['40000','10000']],0,2,'1'));await tick();
    reply('geometry',geom(b,[['40000','10000'],['60000','30000']],0,2,null));await tick();paint();
    assert.equal(el('drc-search').value,'MASK');assert.equal(el('drc-waived').value,'waived');
    assert.equal(el('drc-selected').textContent,'Global '+b.global+' · 2/2 vertices');
    assert.equal(el('drc-canvas').hidden,true);assert.equal(el('drc-markers').checked,false);
    assert.equal(nav.length,beforeRestoreNav,'restore moved the existing viewport');
    assert.equal(savedChanges.length,beforeRestoreSaves,'restore wrote panel state');
    el('drc-markers').checked=true;el('drc-markers').onchange();
    assert.deepEqual(savedChanges.at(-1).selected,{check:b.check,error:b.local});
    assert.equal(savedChanges.at(-1).zoom_lock,true);assert.equal(savedChanges.at(-1).jump_scale,'0.2');
    savedPanel={...savedPanel,selected:null,query:{bbox_um:['1.25','2','60','70'],state_rev:'1',cursor:{check:'0',error:'100'}}};
    const queryReload=el('drc-reload').onclick();
    reply('rules',{rows:[],next:null});await tick();
    reply('rule',{name:'MASK',description:'query restoration',errors:'9007199254740996',waived:'1'});await tick();
    assert.deepEqual(pending('query').body.body.bbox_um,savedPanel.query.bbox_um);
    assert.deepEqual(pending('query').body.body.cursor,savedPanel.query.cursor);
    reply('query',{rows:[],next:{check:'0',error:'200'}});await queryReload;await tick();
    assert.equal(el('drc-error-next').disabled,false);assert(el('drc-result-info').textContent.includes('earlier viewport'));
    assert.equal(nav.length,beforeRestoreNav);
    panel.stop();assert.equal(raf.size,0);
    global.setTimeout=nativeSetTimeout;global.clearTimeout=nativeClearTimeout;
    console.log('WEB DRC UI: ALL OK (projection, u64, text safety, stale/cancel, complete geometry, focus/zoom, paging, in-view, cleanup)');
})().catch(e=>{console.error(e);process.exitCode=1;});
