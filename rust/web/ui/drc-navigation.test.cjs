'use strict';
// Navigation/selection contract, independent of real-browser input dispatch.
const assert = require('node:assert/strict');
const D = require('./drc.js'), P = require('./protocol.js');
const nodes = new Map(), requests = [], moves = [], saves = [];
let active = null;
class Element {
    constructor(){this.children=[];this.value='';this.checked=false;this.disabled=false;this.hidden=false;this.style={};}
    set textContent(v){this.text=v;this.children=[];} get textContent(){return this.text||'';}
    appendChild(e){this.children.push(e);return e;}
    setAttribute(k,v){this[k]=v;}
    getContext(){return null;}
    focus(){active=this;}
    scrollIntoView(){}
    contains(e){return this===e||this.children.some(c=>c.contains(e));}
}
const el=id=>{if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
const document={getElementById:el,createElement:()=>new Element(),get activeElement(){return active;}};
el('drc-waived').value='all';el('drc-markers').checked=true;
const row=i=>({check:'0',local:String(i),global:String(i+1),kind:'p',status:i%3===0?1:0,bbox_um:[String(i),'0',String(i+1),'1'],points:'4'});
let context={id:'v1',source:'source',connected:true,pending:false,state:{state_rev:'1',status:'idle',dbu_um:'1',bbox_dbu:['0','0','100','80'],pixels:[100,80]}};
let held=null, holdStep=false, heldPage=null, restoreData=null, waitRestore=false, releaseRestore=null;
const page=(start,waived)=>{let rows=[];let i=Number(start);for(;i<130&&rows.length<64;i++){const r=row(i);if(waived===null||(r.status===1)===waived)rows.push(r);}return {rows,next:i<130?String(i):null};};
function http(method,path,body,missing,token){
    const b=body&&body.body;requests.push({method,path,body});
    if(!b)return Promise.resolve({drc:{id:'drc',revision:'r1',source_id:'source',title:'test',phase:'ready',metadata:{checks:'1',errors:'130'}}});
    if(b.kind==='rules')return Promise.resolve({rows:[{check:'0',name:'R',name_truncated:false,errors:'130',waived:'44'}],next:null});
    if(b.kind==='rule')return Promise.resolve({check:'0',name:'R',description:'rule',errors:'130',waived:'44'});
    if(b.kind==='errors')return Promise.resolve(page(b.start,b.waived));
    if(b.kind==='geometry') {const r=row(Number(b.error)),points=[[r.bbox_um[0],'0'],[r.bbox_um[2],'0'],[r.bbox_um[2],'1'],[r.bbox_um[0],'1']];
        const start=Number(b.start),end=Math.min(4,start+b.limit);
        return Promise.resolve({...r,precision:'1',points_dbu:points.slice(start,end),start:b.start,total:'4',next:end<4?String(end):null});}
    if(b.kind==='focus')return Promise.resolve({navigation:{kind:'goto',center_um:[b.error,'0.5'],width_um:'4'}});
    if(b.kind==='measurements')return Promise.resolve({check:b.check,local:b.error,global:P.next(b.error),segments:[]});
    if(b.kind==='step') {
        if(holdStep)return new Promise(resolve=>{held={token,resolve,body:b};token.abort=()=>{};});
        if(heldPage){const v=heldPage;heldPage=null;return Promise.resolve(v);}
        let start=b.cursor?Number(b.cursor.next):b.after===null?(b.backwards?129:0):(Number(b.after)+(b.backwards?-1:1)+130)%130;
        for(let n=0;n<130;n++) {const i=(start+(b.backwards?-n:n)+130)%130,r=row(i);if(b.waived===null||(r.status===1)===b.waived)return Promise.resolve({hit:r,next:null,scanned:String(n+1)});}
        return Promise.resolve({hit:null,next:null,scanned:'130'});
    }
    throw new Error('unexpected request '+JSON.stringify(b));
}
const panel=D.bind({document,window:{requestAnimationFrame:()=>1,cancelAnimationFrame(){}},protocol:P,rulers:require('./rulers.js'),http,context:()=>context,navigate:v=>moves.push(v),resize(){},
    stateStore:{bind:o=>{let ready=false;return {attach:async()=>{ready=false;if(waitRestore)await new Promise(r=>{releaseRestore=r;});await o.apply(restoreData);ready=true;},change:d=>{if(ready)saves.push(JSON.parse(JSON.stringify(d)));},close(){ready=false;}};}}});
async function tick(){for(let i=0;i<60;i++)await Promise.resolve();}
const count=kind=>requests.filter(r=>r.body&&r.body.body.kind===kind).length;
(async()=>{
    await panel.init();await tick();
    assert.equal(el('drc-errors').children.length,64);
    const clicked=el('drc-errors').children[63];clicked.onclick();await tick();
    assert.equal(moves.length,0,'first click must only focus/select');
    assert.equal(el('drc-errors').children[63],clicked,'single click replaced the double-click target');
    assert(panel.key('n'));await tick();
    assert.equal(saves.at(-1).selected.error,'64','n did not cross the page boundary');
    assert.equal(moves.length,0);
    panel.key('p');await tick();assert.equal(saves.at(-1).selected.error,'63');
    el('drc-first').onclick();await tick();el('drc-errors').children[0].onclick();await tick();
    panel.key('p');await tick();assert.equal(saves.at(-1).selected.error,'129');
    el('drc-step-next').onclick();await tick();assert.equal(saves.at(-1).selected.error,'0');
    assert.equal(active.getAttribute?active.getAttribute('aria-label'):active['aria-label'],'Error 1, global 1');
    // Double click enters jump mode; click/n/p now move, Escape ends the
    // mode but preserves the position for the next keyboard step.
    el('drc-errors').children[0].ondblclick();await tick();assert.equal(moves.length,1);
    panel.key('n');await tick();assert.equal(moves.length,2);assert.equal(saves.at(-1).selected.error,'1');
    el('drc-errors').children[2].onclick();await tick();assert.equal(moves.length,3);
    assert(panel.key('Escape'));await tick();assert.equal(saves.at(-1).selected.error,'2');
    assert.equal(saves.at(-1).jump_active,false);assert.equal(saves.at(-1).focus_visible,false);
    panel.key('n');await tick();assert.equal(saves.at(-1).selected.error,'3');assert.equal(moves.length,3);
    el('drc-waived').value='waived';el('drc-waived').onchange();await tick();
    panel.key('p');await tick();assert.equal(saves.at(-1).selected.error,'129');
    panel.key('n');await tick();assert.equal(saves.at(-1).selected.error,'0');
    // Empty + continuation is shown and only resumes after explicit input.
    heldPage={hit:null,next:{next:'65',remaining:'65'},scanned:'65'};
    const before=count('step');panel.key('n');await tick();assert.equal(count('step'),before+1);
    assert.equal(el('drc-step-continue').hidden,false);assert(el('drc-message').textContent.includes('incomplete'));
    el('drc-step-continue').onclick();await tick();assert.equal(saves.at(-1).selected.error,'66');
    const resumed=requests.filter(r=>r.body&&r.body.body.kind==='step').at(-1).body.body;
    assert.equal(resumed.after,null);assert.deepEqual(resumed.cursor,{next:'65',remaining:'65'});
    // Repeated input has no unbounded queue. Esc cancels a held read and a
    // late response must not resurrect a cleared focus or move the view.
    holdStep=true;panel.key('n');await tick();const inFlight=count('step');
    for(let i=0;i<100;i++)panel.key('n');await tick();assert.equal(count('step'),inFlight);
    panel.key('Escape');assert(held.token.cancelled);held.resolve({hit:row(69),next:null,scanned:'3'});await tick();
    assert(!el('drc-message').textContent.includes('Searching'));
    assert.equal(saves.at(-1).selected.error,'66');assert.equal(moves.length,3);holdStep=false;
    // Same live view restores mode/position, but never executes a goto.
    restoreData=saves.at(-1);const beforeRestore=count('focus');
    await el('drc-reload').onclick();await tick();assert.equal(count('focus'),beforeRestore);
    assert.equal(el('drc-selected').textContent.includes('focus cleared'),true);
    panel.key('n');await tick();assert.equal(saves.at(-1).selected.error,'69');assert.equal(moves.length,3);
    // A later pan wins over a slow navigation lookup, even in jump mode.
    el('drc-frame').onclick();await tick();assert.equal(moves.length,4);
    holdStep=true;panel.key('n');await tick();
    context={...context,state:{...context.state,state_rev:'2',bbox_dbu:['10','0','110','80']}};panel.contextChanged();
    held.resolve({hit:row(72),next:null,scanned:'3'});await tick();
    assert.equal(saves.at(-1).selected.error,'72');assert.equal(moves.length,4,'late step displaced a newer pan');holdStep=false;
    // Clicking Reload review cancels before a potentially slow panel GET,
    // not only when the saved state finally arrives and apply() starts.
    restoreData=saves.at(-1);holdStep=true;panel.key('n');await tick();waitRestore=true;
    const reloading=el('drc-reload').onclick();assert(held.token.cancelled,'reload left an old step active');
    held.resolve({hit:row(75),next:null,scanned:'3'});await tick();releaseRestore();waitRestore=false;await reloading;await tick();
    assert.equal(el('drc-selected').textContent,'Global 73 · 4/4 vertices');assert.equal(moves.length,4);
    // Filter changes cancel a held lookup. Cursor state must not cross them.
    holdStep=true;panel.key('n');await tick();
    el('drc-waived').value='all';el('drc-waived').onchange();assert(held.token.cancelled);
    held.resolve({hit:row(75),next:null,scanned:'3'});await tick();assert.equal(saves.at(-1).selected,null);
    panel.stop();console.log('WEB DRC NAVIGATION: ALL OK (cross-page/wrap/filter, click/double-click, Escape/mode, bounded continuation/cancel, restoration)');
})().catch(e=>{console.error(e);process.exitCode=1;});
