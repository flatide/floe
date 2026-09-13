'use strict';
// Simulated asynchronous reads/DOM; actual browser presentation is separate.
const assert=require('node:assert/strict'), D=require('./drc.js'), P=require('./protocol.js'), R=require('./rulers.js');
const nodes=new Map(), raf=new Map(), requests=[], saves=[], moves=[], drawing=[];
let serial=0, cdHold=false, focusHold=false, heldCD=null, heldFocus=null, restoreData=null, restoreWait=false, releaseRestore=null, badCD=false;
const ctx=new Proxy({measureText:s=>({width:s.length*6})},{get:(t,k)=>k in t?t[k]:(...v)=>drawing.push([k,...v])});
class Element {
    constructor(){this.children=[];this.style={};this.hidden=false;this.checked=false;this.value='';this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}
    set innerHTML(_){throw new Error('No HTML insertion');}
    appendChild(v){this.children.push(v);return v;}setAttribute(k,v){this[k]=v;}
    getContext(){return ctx;}focus(){}scrollIntoView(){}
    getBoundingClientRect(){return {left:0,top:0,right:200,bottom:160,width:200,height:160};}
}
const el=id=>{if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
el('drc-waived').value='all';el('drc-markers').checked=true;
let context={id:'v1',source:'source',connected:true,pending:false,state:{state_rev:'1',status:'idle',dbu_um:'1',bbox_dbu:['0','0','200','160'],pixels:[200,160]}};
const row=i=>({check:'0',local:String(i),global:String(i+1),kind:i===1?'e':'p',status:0,bbox_um:i===0?['10','10','70','50']:['90','50','160','50'],points:i===1?'2':'4'});
const seg=(a,b,d,offset=false)=>({endpoints_um:[a.map(String),b.map(String)],distance_um:String(d),offset});
function cd(q) {return {check:q.check,local:q.error,global:P.next(q.error),segments:badCD?[{distance_um:'NaN'}]:q.error==='0'?
    [seg([10,30],[70,30],60),seg([40,10],[40,50],40)]:q.error==='1'?[seg([90,50],[160,50],70,true)]:[]};}
function http(method,path,body,missing,token){
    if(path.endsWith('/selection'))return Promise.resolve({revision:'r1',view_id:context.id,state:{selection_rev:'1',total:'0',limit:5000,rules:[]}});
    const q=body&&body.body;requests.push({method,path,q,token});
    if(!q)return Promise.resolve({drc:{id:'drc',revision:'r1',source_id:'source',title:'CD test',phase:'ready',metadata:{checks:'1',errors:'3'}}});
    if(q.kind==='rules')return Promise.resolve({rows:[{check:'0',name:'R',errors:'3',waived:'0'}],next:null});
    if(q.kind==='rule')return Promise.resolve({name:'R',description:'test',errors:'3',waived:'0'});
    if(q.kind==='list')return Promise.resolve({rows:[row(0),row(1),row(2)],next:null,scanned:'3',bbox_um:null,selection_rev:null});
    if(q.kind==='geometry'){
        const r=row(Number(q.error)),pts=r.kind==='e'?[['90','50'],['160','50']]:[['10','10'],['70','10'],['70','50'],['10','50']];
        return Promise.resolve({...r,precision:'1',points_dbu:pts.slice(0,q.limit),start:'0',total:String(pts.length),next:q.limit<pts.length?String(q.limit):null});
    }
    if(q.kind==='focus'){
        if(focusHold)return new Promise(resolve=>{heldFocus={resolve,token};token.abort=()=>{};});
        return Promise.resolve({navigation:{kind:'goto',center_um:['40','30'],width_um:'200'}});
    }
    if(q.kind==='measurements'){
        if(cdHold)return new Promise(resolve=>{heldCD={resolve,token,q};token.abort=()=>{};});
        return Promise.resolve(cd(q));
    }
    throw new Error('Unexpected read '+JSON.stringify(q));
}
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},
    window:{requestAnimationFrame:fn=>{raf.set(++serial,fn);return serial;},cancelAnimationFrame:id=>raf.delete(id)},
    protocol:P,rulers:R,groups:require('./drc-groups.js'),http,context:()=>context,navigate:n=>moves.push(n),resize(){},stateStore:{bind:o=>{
        let ready=false;return {attach:async()=>{ready=false;if(restoreWait)await new Promise(r=>{releaseRestore=r;});await o.apply(restoreData);ready=true;},
            change:v=>{if(ready)saves.push(JSON.parse(JSON.stringify(v)));},close(){ready=false;}};
    }}});
const count=k=>requests.filter(r=>r.q&&r.q.kind===k).length;
const last=()=>saves.at(-1),values=()=>el('drc-cd-values').children.map(n=>n.textContent);
const base=D.projection({bbox_dbu:['0','0','200','160'],width:400,height:320},[0,0],'1'),size={pixels:[400,320],dpr:2,left:0,top:0};
function paint(p=base){for(const [id,fn] of [...raf]){raf.delete(id);fn();}panel.paint(p,size);}
async function tick(){for(let i=0;i<70;i++)await Promise.resolve();}
async function jump(i){el('drc-errors').children[i].ondblclick();await tick();paint();}
(async()=>{
    await panel.init();await tick();paint();
    el('drc-errors').children[0].onclick();await tick();assert.equal(count('measurements'),0,'plain selection measured');
    await jump(0);assert.deepEqual(values(),['Width 60.0000 µm','Height 40.0000 µm']);
    assert.equal(count('measurements'),1);assert.equal(last().cd.target.error,'0');
    assert(drawing.some(v=>v[0]==='fillText'&&v[1]==='Height 40.0000 µm'));
    const before=count('measurements');context={...context,state:{...context.state,state_rev:'2'}};panel.contextChanged();paint(D.shifted(base,[20,-10]));
    assert.equal(count('measurements'),before,'pan reread measurements');
    // A marker selection is not a jump: keep CD from global 1 while global 2
    // is selected, including independent saved restoration without a goto.
    paint();assert(panel.click(125,110,false));await tick();paint();assert.equal(moves.length,1);
    assert.equal(last().selected.error,'1');assert.equal(last().cd.target.error,'0');assert.match(el('drc-cd-title').textContent,/global 1/);
    drawing.length=0;
    panel.paint(D.projection({bbox_dbu:['0','0','80','160'],width:400,height:320},[0,0],'1'),size);
    assert(drawing.some(v=>v[0]==='fillText'&&v[1]==='Width 60.0000 µm'),'offscreen selected error hid a different visible CD');paint();
    assert(panel.key('k'));assert.deepEqual(values(),['Width 60.0000 µm']);assert.equal(last().cd.remaining,1);
    restoreData=last();await el('drc-reload').onclick();await tick();paint();
    assert.equal(moves.length,1,'restore navigated');assert.equal(count('measurements'),before+1);assert.deepEqual(values(),['Width 60.0000 µm']);
    assert.match(el('drc-selected').textContent,/Global 2/);assert.match(el('drc-cd-title').textContent,/global 1/);
    // Ruler removal has priority over focus; position/mode survive the first
    // Escape, then the next Escape ends focus. Empty CD needs only one Escape.
    assert(panel.key('Escape'));assert.equal(last().jump_active,true);assert.equal(last().focus_visible,true);assert.equal(last().cd.remaining,0);
    const reads=count('measurements');restoreData=last();await el('drc-reload').onclick();await tick();assert.equal(count('measurements'),reads);
    assert(panel.key('Escape'));assert.equal(last().jump_active,false);assert.equal(last().cd,null);assert.equal(last().selected.error,'1');
    await jump(2);assert.deepEqual(values(),[]);assert.match(el('drc-cd-title').textContent,/no supported/);assert(panel.key('Escape'));assert.equal(last().jump_active,false);
    await jump(1);assert.deepEqual(values(),['Length 70.0000 µm']);
    el('drc-markers').checked=false;el('drc-markers').onchange();paint();assert(el('drc-canvas').hidden);assert.equal(values().length,1);
    el('drc-markers').checked=true;el('drc-markers').onchange();paint();assert(!el('drc-canvas').hidden);
    assert(panel.key('K'));assert.equal(last().cd.remaining,0);await jump(1);assert.equal(values().length,1,'same-error jump did not restore cleared ruler');
    // Dismissing rulers cancels a slow replacement focus too.
    focusHold=true;el('drc-errors').children[0].onclick();await tick();assert(heldFocus);
    assert(panel.key('Escape'));assert(heldFocus.token.cancelled);const navCount=moves.length;
    heldFocus.resolve({navigation:{kind:'goto',center_um:['0','0'],width_um:'100'}});await tick();assert.equal(moves.length,navCount);focusHold=false;
    // A dismissed/replaced/closed ruler never resurrects from a late read.
    cdHold=true;await jump(0);const old=heldCD;assert(panel.key('Escape'));assert(old.token.cancelled);
    old.resolve(cd(old.q));await tick();assert.deepEqual(values(),[]);assert.equal(last().cd.remaining,0);
    await jump(1);const previous=heldCD;await jump(0);assert(previous.token.cancelled);
    previous.resolve(cd(previous.q));await tick();assert.deepEqual(values(),[]);
    heldCD.resolve(cd(heldCD.q));await tick();assert.equal(values().length,2);cdHold=false;
    // Invalid responses have a visible failure, not invented values, and do
    // not discard the outline or turn unsupported geometry into a measurement.
    badCD=true;await jump(1);assert.match(el('drc-cd-title').textContent,/unavailable/);assert.deepEqual(values(),[]);
    assert.match(el('drc-selected').textContent,/2\/2 vertices/);badCD=false;
    el('drc-message').textContent='View input changed; select the error again to move.';
    await jump(1);assert.equal(values().length,1,'explicit jump did not retry failed CD');assert(!el('drc-message').textContent.includes('input changed'));
    // Pending panel GET cancels immediately; it must not be possible for an
    // old measurement to render during a slow reload before apply() runs.
    restoreData=last();cdHold=true;await jump(0);const stale=heldCD;restoreWait=true;
    const reloading=el('drc-reload').onclick();assert(stale.token.cancelled);stale.resolve(cd(stale.q));await tick();assert.equal(values().length,0);
    cdHold=false;releaseRestore();restoreWait=false;await reloading;await tick();assert.equal(values()[0],'Length 70.0000 µm');
    // Rule changes and source changes are explicit CD boundaries.
    el('drc-rules').children[0].onclick();await tick();assert.equal(last().cd,null);assert.deepEqual(values(),[]);
    cdHold=true;await jump(0);const closing=heldCD;context={...context,source:'other',id:'v2'};panel.contextChanged();assert(closing.token.cancelled);
    closing.resolve(cd(closing.q));await tick();paint();assert.deepEqual(values(),[]);assert(el('drc-canvas').hidden);
    panel.stop();assert.equal(raf.size,0);
    console.log('WEB DRC CD: ALL OK (jump ownership, no pan reread, markers, k/K/Escape, restore, delayed/cancelled/invalid/context reads)');
})().catch(e=>{console.error(e);process.exitCode=1;});
