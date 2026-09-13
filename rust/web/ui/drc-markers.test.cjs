'use strict';
// Painted-target picking, not an all-pack geometry query. Real input dispatch
// and device scaling are also checked in the browser QA stage.
const assert = require('node:assert/strict');
const D = require('./drc.js'), P = require('./protocol.js');
const nodes=new Map(), raf=new Map(), requests=[], drawing=[], moves=[], saves=[];
let serial=0, rows=[], next=null, holdGeometry=false, held=null;
const ctx=new Proxy({}, {get:(t,k)=>k in t?t[k]:(...v)=>drawing.push([k,...v]),set:(t,k,v)=>(t[k]=v,true)});
let rect={left:120.25,top:40.5,width:50,height:40,right:170.25,bottom:80.5};
class Element {
    constructor(){this.children=[];this.value='';this.checked=false;this.hidden=false;this.style={};this.width=1;this.height=1;}
    set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}
    appendChild(v){this.children.push(v);return v;}
    setAttribute(k,v){this[k]=v;}
    getContext(){return ctx;}
    getBoundingClientRect(){return rect;}
}
const el=id=>{if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
el('drc-waived').value='all';el('drc-markers').checked=true;
let context={id:'view',source:'source',connected:true,pending:false,state:{state_rev:'1',status:'idle',dbu_um:'1',bbox_dbu:['0','0','100','80'],pixels:[100,80]}};
const row=(id,x,status=0)=>({check:'0',local:id,global:P.next(id),kind:'p',status,bbox_um:[String(x-10),'10',String(x+10),'30'],points:'4'});
const a=row('9007199254740993',20),b=row('9007199254740994',50,1),c=row('9007199254740995',58);
// This center is near the edge but its entire 7px marker is clipped away.
const invisible=row('9007199254740996',-4);
rows=[a,b,c,invisible];
function http(method,path,body,missing,token){
    const q=body&&body.body;requests.push({method,path,body,token});
    if(!q)return Promise.resolve({drc:{id:'drc',revision:'r1',source_id:'source',title:'synthetic',phase:'ready',metadata:{checks:'1',errors:'9007199254740997'}}});
    if(q.kind==='rules')return Promise.resolve({rows:[{check:'0',name:'R',errors:'9007199254740997',waived:'1'}],next:null});
    if(q.kind==='rule')return Promise.resolve({description:'test'});
    if(q.kind==='errors')return Promise.resolve({rows,next});
    if(q.kind==='focus')return Promise.resolve({navigation:{kind:'goto',center_um:['20','20'],width_um:'60'}});
    if(q.kind==='geometry'){
        if(holdGeometry)return new Promise(resolve=>{held={token,resolve};token.abort=()=>{};});
        const r=[a,b,c,invisible].find(r=>r.local===q.error),v=r.bbox_um;
        return Promise.resolve({...r,precision:'1',points_dbu:[[v[0],v[1]],[v[2],v[1]],[v[2],v[3]],[v[0],v[3]]],start:'0',total:'4',next:null});
    }
    throw new Error('unexpected read '+JSON.stringify(q));
}
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},
    window:{requestAnimationFrame:fn=>{raf.set(++serial,fn);return serial;},cancelAnimationFrame:i=>raf.delete(i)},
    protocol:P,http,context:()=>context,navigate:n=>moves.push(n),resize(){},
    stateStore:{bind:o=>{let ready=false;return {attach:async()=>{ready=false;await o.apply(null);ready=true;},change:d=>{if(ready)saves.push(d);},close(){ready=false;}};}}});
const base=D.projection({bbox_dbu:['-48','-48','148','128'],width:196,height:176},[48,48],'1');
const size={pixels:[100,80],dpr:2,left:.25,top:.5};
function paint(p=base){for(const [id,fn] of [...raf]){raf.delete(id);fn();}panel.paint(p,size);}
function hit(x,y=60,twice=false){return panel.click(rect.left+x/2,rect.top+y/2,twice);}
const count=kind=>requests.filter(r=>r.body&&r.body.body.kind===kind).length;
async function tick(){for(let i=0;i<40;i++)await Promise.resolve();}
(async()=>{
    await panel.init();await tick();paint();
    assert.equal(el('drc-canvas').style.left,'0.25px');
    assert.equal(hit(0),false,'fully clipped marker remained selectable');
    assert.equal(hit(20),true);await tick();paint();
    assert.equal(saves.at(-1).selected.error,a.local);assert.equal(count('focus'),0);assert.equal(moves.length,0);
    assert(drawing.some(v=>v[0]==='fillRect'&&v[3]===9),'selection removed its visible double-click anchor');
    assert.equal(hit(20,60,true),true);await tick();paint();assert.equal(moves.length,1);
    // Single canvas click stays focus-only even after entering jump mode.
    assert.equal(hit(58),true);await tick();paint();assert.equal(saves.at(-1).selected.error,c.local);assert.equal(moves.length,1);
    assert.equal(hit(50),true);await tick();paint();assert.equal(saves.at(-1).selected.error,b.local,'nearest marker lost to earlier row');
    assert.equal(hit(20-12),true,'inclusive 6 CSS px radius');await tick();paint();
    assert.equal(hit(20-12.01),false);assert.equal(panel.click(NaN,70,false),false);
    assert.equal(panel.click(rect.right,70,false),false);assert.equal(panel.click(130,rect.bottom,false),false);
    // The overlay moves with the *displayed* crop; requested coordinates may
    // already be different. There is no inverse world-coordinate rounding.
    const crop=D.shifted(base,[48,0]);paint(crop);
    assert.equal(hit(2),true);await tick();paint(crop);assert.equal(saves.at(-1).selected.error,b.local);
    rect={...rect,left:320.125,right:370.125,top:140.75,bottom:180.75};
    assert.equal(hit(10),true);await tick();paint(crop);assert.equal(saves.at(-1).selected.error,c.local);
    // Hidden markers/pending edits, changed state or context, and restoring
    // pages cannot reuse a previous frame's hit list.
    el('drc-markers').checked=false;assert.equal(hit(10),false);el('drc-markers').checked=true;
    context={...context,pending:true};assert.equal(hit(10),false);context={...context,pending:false,connected:false};assert.equal(hit(10),false);
    context={...context,connected:true,state:{...context.state,state_rev:'2'}};assert.equal(hit(10),false);paint();
    el('drc-markers').onchange();assert.equal(hit(20),false,'unpainted marker state reused old hits');paint();
    // Marker selection cancels the previous large outline read. Late pages
    // cannot become the new error's geometry or trigger navigation.
    panel.clear();paint();holdGeometry=true;assert(hit(20));await tick();paint();const old=held;
    holdGeometry=false;assert(hit(50));assert(old.token.cancelled);old.resolve({});await tick();paint();
    assert.equal(saves.at(-1).selected.error,b.local);assert(el('drc-selected').textContent.includes(b.global));assert.equal(moves.length,1);
    // Changing a page invalidates targets immediately, before its next rAF.
    rows=[c];el('drc-first').onclick();await tick();assert.equal(hit(20),false);paint();assert.equal(hit(20),false);
    assert.equal(count('query'),0);assert.equal(count('in_view'),0);assert.equal(count('step'),0);
    context={...context,source:'other'};assert.equal(hit(58),false);panel.contextChanged();paint();assert(el('drc-canvas').hidden);
    panel.stop();assert.equal(raf.size,0);assert.equal(hit(58),false);
    console.log('WEB DRC MARKERS: ALL OK (drawn-only, nearest, CSS/DPR/crop, u64, click/jump, stale/hidden/cancel, no spatial scans)');
})().catch(e=>{console.error(e);process.exitCode=1;});
