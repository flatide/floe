'use strict';
// Real DRC + selection + notes modules: no stand-in for their lifecycle hooks.
const assert=require('node:assert/strict'),fs=require('node:fs');
const D=require('./drc.js'),N=require('./drc-notes.js'),P=require('./protocol.js'),F=require('./drc-notes.test.cjs');
const API='/api/v1/drc/review/notes',clone=v=>JSON.parse(JSON.stringify(v));
const ids=new Set([...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
const nodes=new Map(),requests=[],moves=[],timers=new Map(),raf=new Map();let serial=0,groupRev='1';
let groups=[{check:'0',errors:['0']},{check:'1',errors:['1']}];
class Element {
    constructor(){this.children=[];this.style={};this.value='';this.checked=false;this.hidden=false;this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}set innerHTML(_){throw Error('HTML injection');}
    appendChild(v){this.children.push(v);return v;}setAttribute(k,v){this[k]=v;}focus(){}scrollIntoView(){}
    getContext(){return new Proxy({}, {get:()=>()=>{}});}
}
const el=id=>{assert(ids.has(id),'missing HTML '+id);if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
el('drc-waived').value='all';el('drc-markers').checked=true;
const row=(ci='0',ei='0')=>({check:ci,local:ei,global:ei==='0'?'9007199254740993':'9007199254740994',kind:'p',status:0,bbox_um:['1','1','4','3'],points:'4'});
const view={id:F.context.view_id,source:'source',connected:true,pending:false,state:{connection_epoch:'8'.repeat(64),state_rev:'1',status:'idle',layers_isolated:false,bbox_dbu:['0','0','10','10'],pixels:[200,200],dbu_um:'1'}};
const cat={drc:{id:F.context.drc_id,revision:F.context.revision,title:'synthetic.db.ice',phase:'ready',source_id:'source',metadata:{checks:'2',errors:'4',format:'ice'}},notes:F.catalog()};
async function http(method,path,body,missing,token){
    requests.push({method,path,body,token});
    if(path==='/api/v1/drc')return clone(cat);
    if(path===API){assert.equal(method,'GET','integration must not approve a write');return clone(cat.notes);}
    if(path===API+'/read')return F.snapshot(body.context,body.errors.length);
    if(path===API+'/prepare')return F.prepared(body.context,groups.length||1,body.text);
    if(path===API+'/revoke')return null;
    if(path.endsWith('/selection')){assert.equal(method,'GET');return {revision:cat.drc.revision,view_id:view.id,state:{selection_rev:groupRev,total:String(groups.reduce((n,g)=>n+g.errors.length,0)),limit:5000,rules:clone(groups)}};}
    const q=body.body;
    if(q.kind==='rules')return {rows:[{check:'0',name:'WIDTH',errors:'2',waived:'0'},{check:'1',name:'SPACE',errors:'2',waived:'0'}],next:null};
    if(q.kind==='rule')return {name:'WIDTH',description:'Synthetic rule',errors:'2',waived:'0'};
    if(q.kind==='list')return {rows:[row(q.check),row(q.check,'1')],next:null,scanned:'2',bbox_um:null,selection_rev:null};
    if(q.kind==='records')return {rows:q.errors.map(e=>row(q.check,e))};
    if(q.kind==='comparison')return {...row(q.check,q.error),comparison:null};
    if(q.kind==='geometry')return {...row(q.check,q.error),precision:'1',points_dbu:[['1','1'],['4','1'],['4','3'],['1','3']].slice(0,q.limit),total:'4',start:'0',next:q.limit===1?'1':null};
    throw Error('Unexpected read '+q.kind);
}
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},
    window:{requestAnimationFrame:fn=>{raf.set(++serial,fn);return serial;},cancelAnimationFrame:id=>raf.delete(id)},
    protocol:P,rulers:require('./rulers.js'),groups:require('./drc-groups.js'),notes:N,http,context:()=>view,
    session:()=>'f'.repeat(64),loadNotePending:()=>null,saveNotePending:()=>{},now:()=>100,
    setTimeout:(fn,ms)=>{timers.set(++serial,{fn,ms});return serial;},clearTimeout:id=>timers.delete(id),
    navigate:(...args)=>moves.push(args),restoreLayers:(...args)=>moves.push(args),resize(){},
    stateStore:{bind:o=>({attach:()=>o.apply(null),change(){},close(){}})}});
async function tick(){for(let i=0;i<150;i++)await Promise.resolve();}
const noteReads=()=>requests.filter(r=>r.path===API+'/read');
(async()=>{
    await panel.init();await tick();assert(!el('notes-read').disabled);assert.match(el('notes-selection').textContent,/2 selected.*across rules/);
    assert(panel.key('n'));await tick();assert.deepEqual(noteReads().at(-1).body.errors,[{check:'0',error:'0'},{check:'1',error:'1'}]);
    const keyReads=noteReads().length;assert(panel.key('n'));await tick();assert.equal(noteReads().length,keyReads,'repeat note key reread snapshot');
    assert.deepEqual(noteReads().at(-1).body.context,F.context);el('notes-text').value='two rules';el('notes-text').oninput();await el('notes-prepare').onclick();
    assert(!el('notes-review').hidden);const reads=noteReads().length;
    view.pending=true;view.state.state_rev='2';panel.contextChanged();await tick();assert(!el('notes-review').hidden);assert.equal(noteReads().length,reads,'pan triggered a note read');view.pending=false;
    // Selection synchronization retires the old preview but never drops text.
    groups=[];groupRev='2';await el('drc-reload').onclick();await tick();assert(el('notes-review').hidden);assert.equal(el('notes-text').value,'two rules');
    el('notes-discard').onclick();el('drc-errors').children[0].onclick();await tick();assert.match(el('notes-selection').textContent,/Global 9007199254740993/);
    await el('notes-read').onclick();assert.deepEqual(noteReads().at(-1).body.errors,[{check:'0',error:'0'}],'used displayed global number as gid');
    el('notes-text').value='current error';el('notes-text').oninput();await el('notes-prepare').onclick();assert(!el('notes-review').hidden);
    view.state.connection_epoch='7'.repeat(64);panel.contextChanged();assert(el('notes-review').hidden);assert.equal(el('notes-text').value,'current error');
    el('notes-discard').onclick();await el('notes-read').onclick();el('notes-text').value='old database';el('notes-text').oninput();await el('notes-prepare').onclick();
    cat.drc.id='6'.repeat(64);cat.drc.revision='5'.repeat(64);await panel.refresh();await tick();assert(el('notes-review').hidden);assert.equal(el('notes-text').value,'old database');
    assert.equal(moves.length,0);assert.equal(requests.filter(r=>r.path===API&&r.method==='POST').length,0);
    panel.stop();assert.equal(timers.size,0);assert.equal(raf.size,0);assert.equal(el('notes-text').value,'');
    console.log('WEB DRC NOTES PANEL: ALL OK (real selection/notes integration, cross-rule refs, zero-based IDs, pan no-read, selection/epoch/DRC invalidation, no writes/navigation, cleanup)');
})().catch(e=>{panel.stop();console.error(e);process.exitCode=1;});
