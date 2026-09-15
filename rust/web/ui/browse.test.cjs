'use strict';
const assert=require('node:assert/strict'),B=require('./browse.js'),P=require('./protocol.js');
const token=n=>n.toString(16).padStart(64,'0'),tick=()=>new Promise(setImmediate);
async function wait(f){for(let n=0;n<100;n++){if(f()){return;}await tick();}throw Error('picker did not progress');}
function listing(start=0,total=2,directory=token(1)){
    return {snapshot:token(2),directory,breadcrumbs:[{handle:token(1),name:'Approved'}],start,total,next:start+128<total?start+128:null,
        rows:Array.from({length:Math.min(128,total-start)},(_,n)=>({handle:token(n+start+10),name:n===0?'한국 <img src=x>.oas':String(n+start)+'.oas',kind:'file',bytes:'128',modified_seconds:'1'})),skipped_names:0,skipped_links:0};
}
function rig(saved=null){
    const nodes=new Map(),listeners={},calls=[],operations=new Map(),timers=new Map();let timerId=0,last=0,lose=false,hold=false,selected=0,active=null,release=null;
    const doc={activeElement:null,contains:n=>!!n,addEventListener(k,fn,capture){assert(capture);listeners[k]=fn;},createElement:tag=>new Node('',tag)};
    class Node{
        constructor(id,tag='div'){this.id=id;this.tag=tag;this.children=[];this.attrs={};this.value='';this.hidden=false;this.disabled=false;this._text='';}
        get textContent(){return this._text;}set textContent(v){this._text=v;this.children=[];}
        appendChild(n){this.children.push(n);if(n.tag==='option'&&!this.value){this.value=n.value;}return n;}
        focus(){doc.activeElement=this;}getClientRects(){return this.hidden?[]:[{}];}
        setAttribute(k,v){this.attrs[k]=v;}getAttribute(k){return this.attrs[k]===undefined?null:this.attrs[k];}removeAttribute(k){delete this.attrs[k];}
        contains(n){return n===this||this.children.some(c=>c.contains(n));}
        querySelectorAll(){return this.children.flatMap(n=>n.tag==='button'?[n]:n.querySelectorAll());}
    }
    function el(id){if(!nodes.has(id)){nodes.set(id,new Node(id));}return nodes.get(id);}
    el('browse-filter').value='all_files';el('browse-dialog').hidden=true;
    const env={el,document:doc,protocol:P,available:()=>true,changed(){},selected(){selected++;},loadPending:()=>saved,savePending:v=>{saved=v;},
        setTimeout(fn,ms){const id=++timerId;timers.set(id,fn);return id;},clearTimeout:id=>timers.delete(id),
        async http(method,path,body,missing,signal){
            calls.push({method,path,body:body&&JSON.parse(JSON.stringify(body))});
            if(path==='/api/v1/browse'&&method==='GET'){return {roots:[{handle:token(1),name:'Approved'}],last_seq:String(last),active};}
            if(method==='POST'&&path==='/api/v1/browse'){
                assert(saved,'journal must precede admission');assert.equal(body.seq,JSON.parse(saved).request.seq);
                let state=operations.get(body.seq);if(!state){last=Number(body.seq);state={seq:body.seq,kind:body.kind,request:JSON.parse(JSON.stringify(body)),phase:'succeeded',
                    result:body.kind==='select'?{source_id:token(7),launch_id:token(8)}:{page:listing(body.start||0,300)}};operations.set(body.seq,state);}
                if(hold){await new Promise(r=>{release=r;});}
                if(lose){throw Error('lost ACK');}return state;
            }
            const seq=path.split('/')[4],state=operations.get(seq);
            if(path.endsWith('/cancel')){assert(saved);state.phase='cancelled';state.error='browse_cancelled';active=null;return state;}
            assert.equal(method,'GET');return state||null;
        }};
    const api=B.bind(env);
    return {api,el,env,doc,listeners,calls,operations,get saved(){return saved;},get selected(){return selected;},set lose(v){lose=v;},set hold(v){hold=v;},release:()=>release(),
        async timer(){const it=timers.entries().next().value;if(it){timers.delete(it[0]);it[1]();}await tick();}};
}
(async()=>{
    const p=listing();assert.equal(B.page(p),p);
    for(const change of [{total:100001},{rows:Array(129).fill(p.rows[0])},{next:128},{directory:'../'},{start:1},{breadcrumbs:[]}]){assert.throws(()=>B.page({...p,...change}));}
    const disabled=rig();await disabled.api.init(false,true);assert.equal(disabled.calls.length,0);
    const r=rig();await r.api.init(true,true);await wait(()=>r.el('browse-entries').children.length===128);
    assert(r.api.blocked());assert.equal(r.el('app-header').getAttribute('aria-hidden'),'true');
    assert.match(r.el('browse-entries').children[0].children[0].textContent,/<img src=x>/,'literal filename, no HTML');
    r.el('browse-next').onclick();await wait(()=>/129–256/.test(r.el('browse-status').textContent));
    r.el('browse-prev').onclick();await wait(()=>/1–128/.test(r.el('browse-status').textContent));
    assert(r.calls.some(c=>c.body&&c.body.kind==='page'&&c.body.start===128));
    r.el('browse-entries').children[0].children[0].onclick();assert(!r.el('browse-select').disabled);
    r.el('browse-select').onclick();await wait(()=>r.selected===1);
    assert(!r.api.blocked());assert.equal(r.saved,null);assert(r.el('browse-dialog').hidden);assert.equal(r.el('app-header').getAttribute('aria-hidden'),null);
    assert(r.calls.every(c=>!c.path.includes('upload')&&!c.path.includes('operations')));r.api.stop();

    const lost=rig();await lost.api.init(true,true);await wait(()=>lost.el('browse-entries').children.length);
    lost.el('browse-entries').children[0].children[0].onclick();lost.lose=true;lost.el('browse-select').onclick();
    await wait(()=>/lost ACK/.test(lost.el('browse-status').textContent));const saved=lost.saved,record=JSON.parse(saved).request;
    assert(saved);assert.equal(lost.selected,0);assert(lost.el('browse-close').disabled);lost.api.stop();
    const reload=rig(saved);reload.operations.set(record.seq,{seq:record.seq,kind:'select',request:record,phase:'succeeded',result:{source_id:token(7),launch_id:token(8)}});
    await reload.api.init(true,false);await wait(()=>reload.selected===1);assert.equal(reload.calls.filter(c=>c.method==='POST').length,0,'recovery only reads');reload.api.stop();

    const retry=rig(saved);await retry.api.init(true,false);await wait(()=>/receipt/.test(retry.el('browse-status').textContent));
    retry.el('browse-check').onclick();await wait(()=>retry.selected===1);
    assert.deepEqual(retry.calls.find(c=>c.method==='POST').body,record,'only the identical request may retry');retry.api.stop();

    const collision=rig(saved);collision.operations.set(record.seq,{seq:record.seq,kind:'list',request:{...record,handle:token(99)},phase:'succeeded',result:{page:listing()}});
    await collision.api.init(true,false);await wait(()=>/different request/.test(collision.el('browse-status').textContent));
    assert.equal(collision.selected,0);assert.equal(collision.saved,saved);assert.equal(collision.calls.filter(c=>c.method==='POST').length,0);collision.api.stop();

    const cancelled=rig(saved);cancelled.operations.set(record.seq,{seq:record.seq,kind:'select',request:record,phase:'running'});
    await cancelled.api.init(true,false);await wait(()=>!cancelled.el('browse-cancel').disabled);
    cancelled.el('browse-cancel').onclick();await wait(()=>/cancelled/.test(cancelled.el('browse-status').textContent));
    assert.equal(cancelled.selected,0);assert.equal(cancelled.saved,null);cancelled.el('browse-close').onclick();assert(!cancelled.api.blocked());cancelled.api.stop();

    const late=rig();late.hold=true;await late.api.init(true,true);await wait(()=>late.saved);late.api.stop();late.release();await tick();
    assert.equal(late.el('browse-entries').children.length,0,'hidden late result must not paint');assert(late.saved);
    await late.api.resume();await wait(()=>late.el('browse-entries').children.length===128);late.api.stop();
    const invalid=rig('{');await invalid.api.init(true,false);assert(invalid.api.blocked());assert.match(invalid.el('browse-status').textContent,/invalid/);assert(invalid.el('browse-close').disabled);invalid.api.stop();
    console.log('WEB FILE PICKER: ALL OK (paging, literal names, journal-before-send, read-only recovery, identical retry, sequence collision, cancellation, late response, invalid storage)');
})().catch(e=>{console.error(e);process.exitCode=1;});
