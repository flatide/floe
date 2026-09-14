'use strict';
const assert=require('node:assert/strict'),M=require('./minimap.js');
const elements=new Map(),fills=[],draws=[],requests=[],jumps=[];
function canvas(){return {width:0,height:0,hidden:false,style:{},handlers:{},getContext(){return {fillStyle:'',fillRect(...r){fills.push(r);},drawImage(...r){draws.push(r);}};},getBoundingClientRect(){return {left:20,top:30,width:360,height:360};},setAttribute(k,v){this[k]=v;},addEventListener(k,f){this.handlers[k]=f;}};}
for(const name of ['minimap','minimap-panel','minimap-note','minimap-retry']){elements.set(name,canvas());}
const el=k=>elements.get(k),tick=()=>new Promise(r=>setImmediate(r));
let s=null,ready=true,focus=0;
const base='0'.repeat(32400),snapshot=(depth='full',epoch='epoch')=>({view_id:'view',dataset_revision:'1',connection_epoch:epoch,
    minimap:{size:180,base:depth,die:[0,0,180,180],marks:[[10,10,5,5,4]]}});
const m=M.bind({el,document:{createElement:canvas},state:()=>s,ready:()=>ready,navigate:n=>jumps.push(n),focus(){focus++;},
    http(method,path){assert.equal(method,'GET');return new Promise((resolve,reject)=>requests.push({path,resolve,reject}));}});
const answer=(i,override={})=>requests[i].resolve({view_id:'view',dataset_revision:'1',base:requests[i].path.split('/').at(-1),size:180,pixels:base,...override});
(async()=>{
    assert(el('minimap-panel').hidden);
    s=snapshot();m.changed();assert.equal(requests.length,1);
    for(let i=0;i<30;i++){s={...s,minimap:{...s.minimap,marks:[[i,i,5,5,4]]}};m.changed();}
    assert.equal(requests.length,1,'pan started another overview read');answer(0);await tick();
    assert.match(el('minimap-note').textContent,/Die outline/);const painted=draws.length;
    m.changed();assert.equal(draws.length,painted,'unchanged projection repainted');
    s=snapshot('0');m.changed();s=snapshot('2');m.changed();assert.equal(requests.length,2,'depth inputs did not coalesce');
    answer(1);await tick();assert.equal(requests.length,3);answer(2);await tick();assert.match(el('minimap-note').textContent,/depth 2/);
    s=snapshot('0');m.changed();assert.equal(requests.length,3,'cached depth refetched');
    s=snapshot('3');m.changed();answer(3);await tick();s=snapshot();m.changed();assert.equal(requests.length,5,'cache did not evict to three bases');
    requests[4].reject(new Error('network'));await tick();assert(!el('minimap-retry').hidden);m.changed();assert.equal(requests.length,5,'failed GET loop');
    el('minimap-retry').onclick();assert.equal(requests.length,6);answer(5,{pixels:'x'.repeat(32400)});await tick();assert(!el('minimap-retry').hidden,'invalid palette accepted');
    el('minimap-retry').onclick();s=snapshot('0','new-epoch');m.changed();answer(6);await tick();assert.equal(requests.length,8,'old reply blocked new epoch read');
    assert.match(el('minimap-note').textContent,/Loading/);answer(7);await tick();assert.match(el('minimap-note').textContent,/depth 0/);
    const event={button:0,buttons:1,detail:1,clientX:200,clientY:210,preventDefault(){}};
    el('minimap').handlers.mousedown(event);assert.deepEqual(jumps,[{kind:'minimap',point:[90,90]}]);assert.equal(focus,1);
    for(const extra of [{button:2},{buttons:3},{detail:2},{ctrlKey:true},{altKey:true},{metaKey:true}]){el('minimap').handlers.mousedown({...event,...extra});}
    ready=false;el('minimap').handlers.mousedown(event);assert.equal(jumps.length,1);ready=true;
    el('minimap').handlers.keydown({key:'Enter',preventDefault(){}});assert.equal(jumps.length,2);
    el('minimap').handlers.keydown({key:' ',isComposing:true,preventDefault(){assert.fail();}});assert.equal(jumps.length,2);
    s={...s,minimap:{...s.minimap,marks:[[0,0,-1,1,4]]}};m.changed();assert(el('minimap').hidden);el('minimap').handlers.mousedown(event);assert.equal(jumps.length,2);
    s=snapshot(0,'new-epoch');m.changed();assert(el('minimap').hidden,'numeric base accepted');
    s=snapshot('2','new-epoch');m.changed();const suspended=requests.length-1;m.suspend();answer(suspended);await tick();m.changed();
    assert(el('minimap-panel').hidden);assert.equal(requests.length,suspended+1,'suspended panel kept reading');
    el('minimap').handlers.mousedown(event);assert.equal(jumps.length,2,'suspended navigation');
    s=snapshot('2','resumed-epoch');m.resume();assert.equal(requests.length,suspended+2);assert(!el('minimap-panel').hidden);
    answer(suspended+1);await tick();assert.match(el('minimap-note').textContent,/depth 2/);
    s=snapshot('3','resumed-epoch');m.changed();const pending=requests.length-1;m.stop();answer(pending);await tick();m.changed();m.resume();assert(el('minimap-panel').hidden);
    assert.equal(requests.length,pending+1,'closed panel kept reading');
    console.log('WEB MINIMAP: ALL OK (bounded bases, pan no reads, depth coalescing, epochs, invalid replies, retry, same-scale screen input, suspend/resume, stop)');
})().catch(e=>{console.error(e);process.exitCode=1;});
