'use strict';
// Real DRC/notes/waives/group modules: paused reads, old callbacks and resume.
const assert=require('node:assert/strict'),fs=require('node:fs');
const D=require('./drc.js'),W=require('./drc-waives.js'),N=require('./drc-notes.js'),P=require('./protocol.js');
const F=require('./drc-waives.test.cjs'),NF=require('./drc-notes.test.cjs');
const API='/api/v1/drc/review/waives',NOTES='/api/v1/drc/review/notes',clone=v=>JSON.parse(JSON.stringify(v));
const ids=new Set([...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
const nodes=new Map(),requests=[],moves=[],timers=new Map(),raf=new Map();let serial=0,groupRev='1',holdGeometry=false,late=null,saved=null;
let groups=[{check:'0',errors:['0']},{check:'1',errors:['1']}];
class Element {
    constructor(){this.children=[];this.style={};this.value='';this.checked=false;this.hidden=false;this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}set innerHTML(_){throw Error('HTML injection');}
    appendChild(v){this.children.push(v);return v;}setAttribute(k,v){this[k]=v;}focus(){}scrollIntoView(){}
    getContext(){return new Proxy({}, {get:()=>()=>{}});}
}
const el=id=>{assert(ids.has(id),'missing HTML '+id);if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
el('drc-waived').value='all';el('drc-markers').checked=true;
const view={id:F.context.view_id,source:'source',connected:true,pending:false,state:{connection_epoch:'8'.repeat(64),state_rev:'1',status:'idle',layers_isolated:false,bbox_dbu:['0','0','10','10'],pixels:[200,200],dbu_um:'1'}};
const cat={drc:{id:F.context.drc_id,revision:F.context.revision,title:'synthetic.db.ice',phase:'ready',source_id:'source',metadata:{checks:'2',errors:'4',format:'ice'}},notes:NF.catalog(),waives:F.catalog()};
const row=(ci='0',ei='0')=>({check:ci,local:ei,global:ei==='0'?'9007199254740993':'9007199254740994',kind:'p',status:cat.drc.revision===F.context.revision?0:1,bbox_um:['1','1','4','3'],points:'4'});
async function http(method,path,body,missing,token){
    requests.push({method,path,body,token});
    if(path==='/api/v1/drc')return clone(cat);
    if(path===NOTES){assert.equal(method,'GET','notes must not be saved');return clone(cat.notes);}
    if(path===NOTES+'/read')return NF.snapshot(body.context,body.errors.length);
    if(path===NOTES+'/prepare')return NF.prepared(body.context,2,body.text);
    if(path===NOTES+'/revoke'||path===API+'/revoke')return null;
    if(path===API){if(method==='GET')return clone(cat.waives);
        assert(el('drc-frame').disabled,'old DRC controls remained enabled during submission');
        saved=clone(body);cat.waives=F.catalog([F.op(body.seq,'queued')]);return clone(cat.waives.operations.history[0]);}
    if(path===API+'/read')return F.snapshot(body.context,body.errors.length,{review_rev:cat.waives.review_rev});
    if(path===API+'/prepare')return F.prepared(body.context,groups.length||1,body.waived,{review_rev:cat.waives.review_rev});
    if(path.endsWith('/selection')){assert.equal(method,'GET');return {revision:cat.drc.revision,view_id:view.id,state:{selection_rev:groupRev,total:String(groups.reduce((n,g)=>n+g.errors.length,0)),limit:5000,rules:clone(groups)}};}
    const q=body.body;
    if(q.kind==='rules')return {rows:[{check:'0',name:'WIDTH',errors:'2',waived:cat.drc.revision===F.context.revision?'0':'2'},{check:'1',name:'SPACE',errors:'2',waived:'0'}],next:null};
    if(q.kind==='rule')return {name:'WIDTH',description:'Synthetic rule',errors:'2',waived:cat.drc.revision===F.context.revision?'0':'2'};
    if(q.kind==='list')return {rows:[row(q.check),row(q.check,'1')],next:null,scanned:'2',bbox_um:null,selection_rev:null};
    if(q.kind==='records')return {rows:q.errors.map(e=>row(q.check,e))};
    if(q.kind==='comparison')return {...row(q.check,q.error),comparison:null};
    if(q.kind==='geometry'){
        const value={...row(q.check,q.error),precision:'1',points_dbu:[['1','1'],['4','1'],['4','3'],['1','3']].slice(0,q.limit),total:'4',start:'0',next:q.limit===1?'1':null};
        if(holdGeometry){return new Promise(resolve=>{late={token,reply:()=>resolve(value)};});}return value;
    }
    throw Error('Unexpected read '+q.kind);
}
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},
    window:{requestAnimationFrame:fn=>{raf.set(++serial,fn);return serial;},cancelAnimationFrame:id=>raf.delete(id)},
    protocol:P,rulers:require('./rulers.js'),groups:require('./drc-groups.js'),notes:N,waives:W,http,context:()=>view,
    session:()=>'f'.repeat(64),loadNotePending:()=>null,saveNotePending:()=>{},loadWaivePending:()=>null,saveWaivePending:()=>{},now:()=>100,
    setTimeout:(fn,ms)=>{timers.set(++serial,{fn,ms});return serial;},clearTimeout:id=>timers.delete(id),
    navigate:(...args)=>moves.push(args),restoreLayers:(...args)=>moves.push(args),resize(){},
    stateStore:{bind:o=>({attach:()=>o.apply(null),change(){},close(){}})}});
async function tick(){for(let i=0;i<200;i++)await Promise.resolve();}
const readCount=()=>requests.filter(r=>r.body&&r.body.body).length;
(async()=>{
    await panel.init();await tick();assert(!el('waives-read').disabled);assert.equal(el('drc-review-mode').textContent,'OWNER REVIEW');
    await el('notes-read').onclick();el('notes-text').value='Preserve this unapproved note';el('notes-text').oninput();await el('notes-prepare').onclick();
    assert(panel.key('w'));await tick();assert.deepEqual(requests.at(-1).body.errors,[{check:'0',error:'0'},{check:'1',error:'1'}]);
    assert.equal(el('waives-action').value,'waive');assert.equal(requests.filter(r=>r.path===API&&r.method==='POST').length,0);
    el('waives-action').value='waive';el('waives-action').onchange();await el('waives-prepare').onclick();
    const reads=requests.filter(r=>r.path===API+'/read').length;view.pending=true;view.state.state_rev='2';panel.contextChanged();await tick();
    assert(!el('waives-review').hidden);assert.equal(requests.filter(r=>r.path===API+'/read').length,reads);view.pending=false;
    holdGeometry=true;el('drc-errors').children[0].onclick();await tick();assert(late);assert(!late.token.cancelled);
    el('waives-consent').checked=true;el('waives-consent').onchange();await el('waives-approve').onclick();await tick();
    assert(saved);assert(late.token.cancelled);assert(!el('waives-paused').hidden);assert(el('notes-review').hidden);assert.equal(el('notes-text').value,'Preserve this unapproved note');
    const paused=readCount();late.reply();await tick();assert.equal(readCount(),paused);assert.equal(moves.length,0);assert(!panel.key('n'));
    panel.flush();assert(el('drc-canvas').hidden);
    cat.waives=F.catalog([F.op('1','refreshing_reader',{reader_applied:false})]);await panel.refresh();await tick();assert.equal(readCount(),paused);
    cat.waives=F.catalog([F.op()]);await el('waives-refresh').onclick();await tick();assert(!el('waives-paused').hidden);assert.equal(readCount(),paused,'terminal receipt alone resumed old catalog');
    holdGeometry=false;groups=[];groupRev='1';cat.drc.revision=F.op().reader_revision;await panel.refresh();await tick();
    assert(el('waives-paused').hidden);assert(readCount()>paused);assert.equal(el('notes-text').value,'Preserve this unapproved note');
    assert.equal(requests.filter(r=>r.path===NOTES&&r.method==='POST').length,0);assert.equal(requests.filter(r=>r.path===API&&r.method==='POST').length,1);
    el('drc-errors').children[0].onclick();await tick();
    assert(!el('waives-read').disabled,JSON.stringify({selection:el('waives-selection').textContent,message:el('waives-message').textContent,status:el('waives-status').textContent,editorHidden:el('waives-editor').hidden,group:el('drc-group-count').textContent,selected:el('drc-selected').textContent}));
    await el('waives-read').onclick();
    assert.deepEqual(requests.filter(r=>r.path===API+'/read').at(-1).body.errors,[{check:'0',error:'0'}]);
    el('waives-action').value='clear';el('waives-action').onchange();await el('waives-prepare').onclick();
    view.state.connection_epoch='7'.repeat(64);panel.contextChanged();assert(el('waives-review').hidden);assert.equal(moves.length,0);
    el('notes-autosave').checked=true;el('notes-autosave').onchange();el('waives-autosave').checked=true;el('waives-autosave').onchange();
    assert(el('notes-autosave').checked);assert(el('waives-autosave').checked);
    view.connected=false;panel.contextChanged();assert(!el('notes-autosave').checked);assert(!el('waives-autosave').checked);
    assert(el('notes-autosave').disabled);assert(el('waives-autosave').disabled);
    view.connected=true;view.state.connection_epoch='8'.repeat(64);panel.contextChanged();
    assert(!el('notes-autosave').checked);assert(!el('waives-autosave').checked);
    panel.stop();assert.equal(timers.size,0);assert.equal(raf.size,0);
    console.log('WEB DRC WAIVES PANEL: ALL OK (cross-rule selection, pan no-read, approval pause, stale geometry cancel, matching-revision resume, note preservation, no layout edits, epoch cleanup)');
})().catch(e=>{panel.stop();console.error(e);process.exitCode=1;});
