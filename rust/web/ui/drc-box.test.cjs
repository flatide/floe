'use strict';
// Two-click boxes use the painted frame transform and current page candidates.
const assert=require('node:assert/strict'), D=require('./drc.js'), G=require('./drc-groups.js'), P=require('./protocol.js');
const nodes=new Map(), raf=new Map(), commands=[], reads=[], paints=[], moves=[];let serial=0, revision='1', saved=null, hold=false, held=null;
const chosen=new Map();
const ctx=new Proxy({}, {get:(t,k)=>k in t?t[k]:(...v)=>paints.push([k,...v]),set:(t,k,v)=>(t[k]=v,true)});
let rect={left:100.25,top:30.5,width:100,height:80,right:200.25,bottom:110.5};
class Element {
    constructor(){this.children=[];this.style={};this.value='';this.checked=false;this.hidden=false;this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}
    appendChild(v){this.children.push(v);return v;}setAttribute(k,v){this[k]=v;}
    getContext(){return ctx;}getBoundingClientRect(){return rect;}focus(){}scrollIntoView(){}
}
const el=id=>{if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
el('drc-waived').value='all';el('drc-markers').checked=true;
let context={id:'v1',source:'source',connected:true,pending:false,state:{state_rev:'1',status:'idle',dbu_um:'1',bbox_dbu:['0','0','100','80'],pixels:[200,160]}};
const ids=['9007199254740993','9007199254740994','9007199254740995'];
const row=(ci,id,x)=>({check:ci,local:id,global:P.next(id),kind:'p',status:0,bbox_um:[String(x),'10',String(x+20),'30'],points:'4'});
const all=[row('0',ids[0],10),row('0',ids[1],40),row('0',ids[2],70),row('1','0',10)];let page=0;
function snapshot(){const rules=[...chosen].filter(([ci,s])=>s.size).sort((a,b)=>P.compare(a[0],b[0])).map(([ci,s])=>({check:ci,errors:[...s].sort(P.compare)}));
    return {revision:'r1',view_id:context.id,state:{selection_rev:revision,total:String(rules.reduce((n,r)=>n+r.errors.length,0)),limit:5000,rules}};}
function command(body){
    commands.push(body);assert.equal(body.base_selection_rev,revision);const q=body.body;
    if(q.kind==='clear_all')chosen.clear();else{
        let s=q.mode==='replace'?new Set():new Set(chosen.get(q.check)||[]);
        for(const id of new Set(q.errors)){
            const r=all.find(r=>r.check===q.check&&r.local===id),b=r.bbox_um.map(Number),v=q.bbox_um&&q.bbox_um.map(Number);
            if(v&&(b[0]>v[2]||b[2]<v[0]||b[1]>v[3]||b[3]<v[1]))continue;
            if(q.mode==='toggle'&&s.has(id))s.delete(id);else s.add(id);
        }chosen.set(q.check,s);
    }revision=P.next(revision);return snapshot();
}
function http(method,path,body,missing,token){
    if(path.endsWith('/selection')){
        if(method==='GET')return Promise.resolve(snapshot());
        if(hold)return new Promise(resolve=>{held={resolve,token,body};token.abort=()=>{};});
        return Promise.resolve(command(body));
    }
    const q=body&&body.body;if(!q)return Promise.resolve({drc:{id:'drc',revision:'r1',source_id:'source',title:'synthetic',phase:'ready',metadata:{checks:'2',errors:'9007199254740996'}}});
    reads.push(q);
    if(q.kind==='rules')return Promise.resolve({rows:[{check:'0',name:'MASK',errors:'9007199254740996',waived:'0'},{check:'1',name:'OTHER',errors:'1',waived:'0'}],next:null});
    if(q.kind==='rule')return Promise.resolve({name:q.check==='0'?'MASK':'OTHER',description:'rule',errors:'9007199254740996',waived:'0'});
    if(q.kind==='errors')return Promise.resolve({rows:q.check==='0'?(page===0?all.slice(0,2):all.slice(2,3)):all.slice(3),next:null});
    if(q.kind==='records')return Promise.resolve({rows:q.errors.map(id=>all.find(r=>r.check===q.check&&r.local===id))});
    throw new Error('Unexpected read '+JSON.stringify(q));
}
let cursor='';
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},protocol:P,groups:G,rulers:require('./rulers.js'),http,
    window:{requestAnimationFrame:fn=>{raf.set(++serial,fn);return serial;},cancelAnimationFrame:i=>raf.delete(i)},
    context:()=>context,navigate:v=>moves.push(v),resize(){},cursor:()=>{cursor=panel.boxActive()?'crosshair':'';},
    stateStore:{bind:o=>{let ready=false;return {attach:async()=>{ready=false;await o.apply(saved);ready=true;},change:d=>{if(ready)saved=d;},close(){ready=false;}};}}});
const base=D.projection({bbox_dbu:['-48','-48','148','128'],width:392,height:352},[96,96],'1'),size={pixels:[200,160],dpr:2,left:.25,top:.5};
function paint(p=base){for(const [i,fn] of [...raf]){raf.delete(i);fn();}panel.paint(p,size);}
const click=(x,y,mods)=>panel.click(rect.left+x,rect.top+80-y,false,mods);
const selected=ci=>[...(chosen.get(ci)||[])].sort(P.compare);
async function tick(){for(let i=0;i<80;i++)await Promise.resolve();}
async function box(a,b,mods){paint();assert(click(...a));paint();assert(click(...b,mods));await tick();paint();}
(async()=>{
    await panel.init();await tick();paint();assert(!el('drc-box').disabled,JSON.stringify({message:el('drc-message').textContent,sync:el('drc-group-status').textContent,reads}));assert(panel.key('e'));paint();assert.equal(cursor,'crosshair');
    await box([9,9],[11,11]);assert.deepEqual(selected('0'),[ids[0]],'bbox edge, not center membership');
    assert.equal(commands.at(-1).state_rev,'1');assert.deepEqual(commands.at(-1).body.errors,ids.slice(0,2));
    assert.deepEqual(commands.at(-1).body.bbox_um,['9','9','11','11']);assert(panel.boxActive());
    assert(el('drc-errors').children[0].className.includes('grouped'));assert.equal(el('drc-errors').children.length,2);
    await box([39,9],[41,11],{shiftKey:true});assert.deepEqual(selected('0'),ids.slice(0,2));
    await box([9,9],[11,11],{metaKey:true});assert.deepEqual(selected('0'),[ids[1]],'Cmd alias');
    await box([9,9],[11,11],{metaKey:true});assert.deepEqual(selected('0'),ids.slice(0,2));
    await box([9,9],[11,11],{ctrlKey:true,shiftKey:true});assert.deepEqual(selected('0'),[ids[1]],'Ctrl must take priority');
    await box([1,1],[2,2]);assert.deepEqual(selected('0'),[],'empty replace did not clear just this rule');
    paint();assert(click(10,10));paint();panel.move(rect.left+30,rect.top+60);panel.move(rect.left+40,rect.top+50);paint();
    assert.deepEqual(paints.filter(p=>p[0]==='strokeRect').at(-1),['strokeRect',20,140,60,-40],'rAF retained an old pointer instead of the latest');
    assert(paints.some(p=>p[0]==='lineTo'&&p[1]===28&&p[2]===140),'missing first-corner cross');assert(panel.key('Escape'));
    // The first corner remains a world coordinate across a margin pan.
    paint();assert(click(10,10));paint();context={...context,state:{...context.state,state_rev:'2'}};panel.contextChanged();
    const shifted=D.shifted(base,[20,0]);paint(shifted);assert(click(10,20));await tick();paint();
    assert.deepEqual(commands.at(-1).body.bbox_um,['10','10','20','20']);assert.equal(commands.at(-1).state_rev,'2');
    assert.deepEqual(selected('0'),[ids[0]]);assert.equal(moves.length,0);assert(!reads.some(q=>['query','in_view','geometry','focus'].includes(q.kind)));
    // Escape cancels the first corner, then exits the tool, then clears this rule.
    paint();assert(click(10,10));assert(panel.key('Escape'));assert(panel.boxActive());assert(panel.key('Escape'));assert(!panel.boxActive());
    assert(panel.key('Escape'));await tick();assert.deepEqual(selected('0'),[]);
    // Row modifiers mutate groups without triggering a focus/geometry read.
    el('drc-errors').children[0].onclick({ctrlKey:true,detail:1});await tick();assert.deepEqual(selected('0'),[ids[0]]);
    el('drc-errors').children[1].onclick({shiftKey:true,detail:1});await tick();assert.deepEqual(selected('0'),ids.slice(0,2));
    el('drc-rules').children[1].onclick();await tick();paint();el('drc-errors').children[0].onclick({shiftKey:true});await tick();
    assert.deepEqual(selected('1'),['0']);el('drc-group-clear').onclick();await tick();assert.deepEqual(selected('0'),ids.slice(0,2));
    el('drc-rules').children[0].onclick();await tick();paint();assert(el('drc-errors').children[0].className.includes('grouped'));
    // Page changes invalidate an unfinished box, but preserve earlier groups.
    assert(panel.key('e'));paint();assert(click(11,11));page=1;el('drc-first').onclick();await tick();paint();
    const n=commands.length;assert(click(70,10));assert.equal(commands.length,n,'stale first corner applied to a new page');
    assert(panel.key('Escape'));assert(panel.key('Escape'));await el('drc-reload').onclick();await tick();paint();
    assert.deepEqual(selected('0'),ids.slice(0,2));assert(reads.some(q=>q.kind==='records'),'off-page selected markers were not restored from metadata');
    assert.match(el('drc-group-count').textContent,/2 selected/);assert.equal(moves.length,0);
    // One pending command: no duplicate submission; no actions on stale input.
    hold=true;el('drc-errors').children[0].onclick({ctrlKey:true});await tick();assert(held);assert(el('drc-box').disabled);
    const pending=held;el('drc-errors').children[0].onclick({ctrlKey:true});assert.equal(held,pending);
    held.resolve(command(held.body));hold=false;await tick();paint();
    assert(panel.key('e'));paint();context={...context,pending:true};assert(!click(1,1));context={...context,pending:false,connected:false};assert(!click(1,1));
    context={...context,connected:true};el('drc-markers').checked=false;el('drc-markers').onchange();assert(!panel.boxActive());
    el('drc-markers').checked=true;el('drc-markers').onchange();paint();assert(!panel.click(NaN,30,false));
    context={...context,id:'other',source:'other'};panel.contextChanged();paint();assert(!panel.boxActive());assert(el('drc-canvas').hidden);
    panel.stop();assert.equal(raf.size,0);
    console.log('WEB DRC BOX: ALL OK (two corners, CSS/DPR/margin inverse, page bbox candidates, modes, groups, row modifiers, restore, Escape/stale/cancel)');
})().catch(e=>{console.error(e);process.exitCode=1;});
