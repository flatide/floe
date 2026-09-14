'use strict';
// Real panel + build controller: stale read cancellation, current identity-only
// restore and no layout edit. Native HTTP/actor behavior has a separate gate.
const assert=require('node:assert/strict'), D=require('./drc.js'), P=require('./protocol.js');
const nodes=new Map(),requests=[],held=[],saves=[],attachments=[],moves=[],raf=new Map();let serial=0;
class Element {
    constructor(){this.children=[];this.style={};this.value='';this.checked=false;this.hidden=false;this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}
    set innerHTML(_){throw new Error('untrusted HTML');}
    appendChild(v){this.children.push(v);return v;}setAttribute(k,v){this[k]=v;}focus(){}scrollIntoView(){}
    getContext(){return new Proxy({}, {get:()=>()=>{}});}
}
const el=id=>{if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
el('drc-waived').value='all';el('drc-markers').checked=true;
const row={check:'0',local:'0',global:'1',kind:'p',status:0,bbox_um:['1','1','4','3'],points:'4'};
const view={id:'view',source:'source',connected:true,pending:false,state:{connection_epoch:'epoch',state_rev:'1',status:'idle',layers_isolated:true,bbox_dbu:['0','0','10','10'],pixels:[200,200],dbu_um:'1'}};
function catalog(id='old',phase='ready',seq='0',job=null){return {drc:id?{id,revision:id+'r',title:'sample.db',phase,source_id:'source',metadata:{checks:'1',errors:'1',format:id==='new'?'ice':'ascii'}}:null,
    build:{available:true,source_id:'source',jobs_min:1,jobs_max:16,jobs_default:4,operations:{last_seq:seq,active:job&&!['succeeded','failed','cancelled'].includes(job.phase)?seq:null,history:job?[job]:[]}}};}
let cat=catalog(),hold=false,buildRequest=null;
const initialPanel={search:'old search',metric:null,rule_start:'0',check:'0',error_start:'0',query:null,in_view:false,selected_only:false,waived:false,
    selected:{check:'0',error:'0'},markers:true,shown:true,jump_scale:'0.2',zoom_lock:true,jump_active:false,focus_visible:true,cd:null};
function http(method,path,body,missing,token){
    requests.push({method,path,body,token});
    if(path==='/api/v1/drc')return Promise.resolve(JSON.parse(JSON.stringify(cat)));
    if(path==='/api/v1/drc/builds')return new Promise((resolve,reject)=>{buildRequest={body,resolve,reject,token};token.abort=()=>reject(new Error('aborted'));});
    const id=path.split('/')[4],rev=cat.drc&&cat.drc.id===id?cat.drc.revision:id+'r';
    if(path.endsWith('/selection'))return Promise.resolve({revision:rev,view_id:'view',state:{selection_rev:'1',limit:5000,total:'0',rules:[]}});
    const q=body.body;
    if(q.kind==='rules')return Promise.resolve({rows:[{check:'0',name:'WIDTH',errors:'1',waived:'0'}],next:null});
    if(q.kind==='rule')return Promise.resolve({name:'WIDTH',description:'Synthetic rule',errors:'1',waived:'0'});
    if(q.kind==='list')return Promise.resolve({rows:[row],next:null,scanned:'1',bbox_um:null,selection_rev:null});
    if(q.kind==='comparison')return Promise.resolve({...row,comparison:null});
    if(q.kind==='geometry'){
        const answer={...row,precision:'1',points_dbu:id==='new'?[['1','1'],['4','1'],['4','3'],['1','3']].slice(0,q.limit):undefined,
            points_um:id==='new'?undefined:[['1','1'],['4','1'],['4','3'],['1','3']].slice(0,q.limit),total:'4',start:'0',next:q.limit===1?'1':null};
        if(hold)return new Promise(resolve=>{held.push({token,resolve:()=>resolve(answer)});token.abort=()=>{};});
        return Promise.resolve(answer);
    }
    if(q.kind==='focus')return new Promise(resolve=>{held.push({token,resolve:()=>resolve({})});token.abort=()=>{};});
    throw new Error('Unexpected read '+q.kind);
}
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},
    window:{requestAnimationFrame:fn=>{raf.set(++serial,fn);return serial;},cancelAnimationFrame:id=>raf.delete(id)},
    protocol:P,rulers:require('./rulers.js'),groups:require('./drc-groups.js'),builds:require('./drc-build.js'),http,context:()=>view,
    navigate:(...args)=>moves.push(args),restoreLayers:(...args)=>moves.push(args),resize(){},
    stateStore:{bind:o=>{let ready=false;return {attach:async address=>{ready=false;attachments.push(address);await o.apply(address.path.includes('/old/')?initialPanel:null);ready=true;},
        change:data=>{if(ready)saves.push(data);},close(){ready=false;}};}}});
async function tick(){for(let i=0;i<150;i++)await Promise.resolve();}
function paint(){for(const [id,fn]of[...raf]){raf.delete(id);fn();}}
(async()=>{
    await panel.init();await tick();assert.equal(el('drc-search').value,'old search');assert.match(el('drc-selected').textContent,/Global 1/);
    assert(!el('drc-restore-layers').disabled);assert.equal(moves.length,0);const unchanged=JSON.stringify(view);
    // Start a slow previous-revision geometry and prepared focus, then approve.
    hold=true;el('drc-clear').onclick();el('drc-errors').children[0].ondblclick();await tick();assert(held.length>=2);
    const reads=held.length;await panel.refresh();await tick();assert.equal(held.length,reads);assert(held.every(q=>!q.token.cancelled),'idle polling restarted a slow outline');
    el('drc-build-open').onclick();const approved=el('drc-build-form').onsubmit({preventDefault(){}});await tick();
    assert(buildRequest);assert.equal(buildRequest.body.drc_id,'old');assert(held.every(r=>r.token.cancelled));
    const beforeWrites=saves.length;assert.equal(el('drc-errors').children.length,0);assert(el('drc-canvas').hidden);
    cat=catalog(null,'ready','1',{seq:'1',kind:'drc_build',phase:'running'});buildRequest.resolve(cat.build.operations.history[0]);await approved;await tick();
    assert(!el('drc-panel').hidden);assert(!el('drc-build-cancel').hidden);assert.equal(moves.length,0);
    held.forEach(q=>q.resolve());await tick();paint();assert.equal(el('drc-errors').children.length,0);assert(el('drc-canvas').hidden);assert.equal(moves.length,0);
    // Fresh catalog selects no old errors or local filters and never writes
    // retired state into the new panel. Layout isolation snapshot remains usable.
    hold=false;cat=catalog('new','opening','1',{seq:'1',kind:'drc_build',phase:'succeeded'});await panel.refresh();await tick();
    assert(el('drc-error-next').disabled);assert.match(el('drc-build-review').textContent,/Opening/);
    cat.drc.phase='ready';await panel.refresh();await tick();
    assert.equal(attachments.at(-1).path,'/api/v1/drc/new/views/view/panel');assert.equal(attachments.at(-1).revision,'newr');
    assert.equal(el('drc-search').value,'');assert.equal(el('drc-waived').value,'all');assert(!el('drc-restore-layers').disabled);
    assert(!el('drc-selected').textContent.includes('Global'));assert.equal(saves.length,beforeWrites);assert.equal(JSON.stringify(view),unchanged);assert.equal(moves.length,0);
    // Same geometry id, new waive revision: an old already-produced response
    // cannot repaint or dispatch its prepared focus after catalog adoption.
    hold=true;held.length=0;el('drc-errors').children[0].ondblclick();await tick();assert(held.length>=2);
    const revisionWrites=saves.length;
    cat.drc.revision='status2';cat.drc.phase='updating';await panel.refresh();await tick();
    assert(held.every(r=>r.token.cancelled));assert(el('drc-canvas').hidden);assert.equal(el('drc-errors').children.length,0);
    held.forEach(q=>q.resolve());await tick();paint();assert.equal(moves.length,0);
    hold=false;cat.drc.phase='ready';await panel.refresh();await tick();
    assert.equal(attachments.at(-1).revision,'status2');assert(!el('drc-selected').textContent.includes('Global'));
    assert.equal(saves.length,revisionWrites);assert.equal(JSON.stringify(view),unchanged);
    // Another owner tab begins a build: catalog polling drops active outlines,
    // including on resume directly into drc:null, while leaving progress visible.
    el('drc-errors').children[0].onclick();await tick();assert.match(el('drc-selected').textContent,/Global/);
    cat=catalog(null,'ready','2',{seq:'2',kind:'drc_build',phase:'closing_review'});await panel.refresh();await tick();
    assert(el('drc-canvas').hidden);assert.equal(el('drc-errors').children.length,0);
    panel.stop();const posts=requests.filter(r=>r.path==='/api/v1/drc/builds').length;
    await panel.resume();await tick();assert(!el('drc-panel').hidden);assert(!el('drc-build-cancel').hidden);
    assert.equal(requests.filter(r=>r.path==='/api/v1/drc/builds').length,posts);panel.stop();assert.equal(raf.size,0);
    console.log('WEB DRC BUILD PANEL: ALL OK (old read/focus retirement, busy reload, new identity/empty filters, no layout edits or stale autosave)');
})().catch(e=>{panel.stop();console.error(e);process.exitCode=1;});
