'use strict';
const assert=require('node:assert/strict'),I=require('./index-open.js'),P=require('./protocol.js');
const id=n=>n.toString(16).padStart(64,'0'),copy=v=>JSON.parse(JSON.stringify(v));
const preview={open_seq:'1',source_id:id(1),title:'한국 <img src=x>.oas',mode:'chip',levels:{mode:'only',ids:['1','-2']},display_policy:'window',jobs_available:3};
const first={seq:'1',kind:'open',phase:'failed',error:'index_unavailable',index_open:preview};
function rig(saved=null) {
    const nodes=new Map(),listeners={},calls=[],ops=new Map(),timers=new Map(),done=[];
    let last='1',timer=0,lose=false,drop=false,saveFail=false,held=false,release,ready=true,completionFail=false;
    const doc={activeElement:null,contains:n=>!!n,addEventListener(k,fn,capture){assert(capture);listeners[k]=fn;}};
    class Node {
        constructor(id){this.id=id;this.attrs={};this.children=[];this.value='';this.checked=false;this.hidden=false;this.disabled=false;this.textContent='';}
        focus(){doc.activeElement=this;}getClientRects(){return this.hidden?[]:[{}];}
        setAttribute(k,v){this.attrs[k]=v;}getAttribute(k){return this.attrs[k]===undefined?null:this.attrs[k];}removeAttribute(k){delete this.attrs[k];}
        contains(n){return n===this||this.children.includes(n);}querySelectorAll(){return this.children;}
    }
    function el(k){if(!nodes.has(k)){nodes.set(k,new Node(k));}return nodes.get(k);}
    el('index-open-dialog').hidden=true;el('index-open-dialog').children=[el('index-open-close'),el('index-open-approve')];
    function all(){return {last_seq:last,active:[...ops.values()].find(v=>!['succeeded','failed','incomplete','cancelled'].includes(v.phase))?.seq||null,history:[first,...ops.values()]};}
    function terminal(r,phase='succeeded',stage='open'){return {seq:r.seq,kind:'index_open',open_seq:r.open_seq,request_id:r.request_id,phase,stage,view_id:id(9),error:phase==='failed'?'busy':null,index:{phase:stage==='open'?'succeeded':phase,skipped:1}};}
    const env={el,document:doc,protocol:P,message:String,session:()=>id(20),pixels:()=>[137,103],randomId:()=>id(30),ready:()=>ready,
        changed(){},loadPending:()=>saved,savePending:v=>{if(saveFail){throw Error('storage unavailable');}saved=v;},
        async completed(v){if(completionFail){throw Error('view reload failed');}done.push(copy(v));},
        setTimeout(fn){const n=++timer;timers.set(n,fn);return n;},clearTimeout:n=>timers.delete(n),
        async http(method,path,body,missing,signal) {
            calls.push({method,path,body:body&&copy(body)});
            if(path.endsWith('/index-open')){assert.equal(method,'GET');return copy(preview);}
            if(path==='/api/v1/view'){return {view:{view_id:id(8),state_rev:'7',status:'idle'}};}
            if(path==='/api/v1/operations'&&method==='GET'){return all();}
            if(path==='/api/v1/operations'&&method==='POST'){
                assert(saved,'journal must precede mutation');assert.deepEqual(JSON.parse(saved).request,body);
                if(drop){throw Error('lost before admission');}
                let v=ops.get(body.seq);
                if(!v){v={seq:body.seq,kind:body.kind,open_seq:body.open_seq,request_id:body.request_id,phase:'running',stage:'index',index:{phase:'running',native:{phase:'building'},total:2,completed:0}};ops.set(body.seq,v);last=body.seq;}
                if(held){await new Promise(r=>{release=r;});}
                if(lose){throw Error('lost ACK');}return copy(v);
            }
            const seq=path.split('/')[4],v=ops.get(seq);
            if(!v){throw Object.assign(Error('expired'),{status:410,code:'operation_expired'});}
            if(path.endsWith('/cancel')){assert.equal(method,'POST');ops.set(seq,terminal(v,'cancelled','index'));return ops.get(seq);}
            assert.equal(method,'GET');return copy(v);
        }};
    const api=I.bind(env);
    return {api,el,env,doc,listeners,calls,ops,done,terminal,all,get saved(){return saved;},set lose(v){lose=v;},set drop(v){drop=v;},
        set saveFail(v){saveFail=v;},set held(v){held=v;},set ready(v){ready=v;},set completionFail(v){completionFail=v;},set last(v){last=v;},release:()=>release(),
        async timer(){const [n,fn]=timers.entries().next().value||[];if(fn){timers.delete(n);await fn();}},
        async open(){api.observe(all());await el('index-open').onclick();},
        async approve(){await el('index-open-approve').onclick();}};
}
(async()=>{
    for(const bad of [{jobs_available:17},{source_id:'no'},{levels:{mode:'only',ids:[]}},{levels:{mode:'only',ids:['1','1']}}]){assert.throws(()=>I.preview({...preview,...bad},P));}
    const off=rig();await off.api.init(false);assert(!off.calls.length);off.api.stop();
    const r=rig();await r.api.init(true);await r.open();assert(r.api.blocked());assert.match(r.el('index-open-preview').textContent,/<img src=x>/);
    assert.equal(r.calls.filter(c=>c.method==='POST').length,0);assert.equal(r.el('index-open-jobs').value,'3');
    assert.equal(r.doc.activeElement,r.el('index-open-close'),'approval is not default focus');
    r.el('index-open-force').checked=true;r.el('index-open-close').onclick();await r.open();assert.equal(r.el('index-open-force').checked,false,'fresh review resets force');
    r.el('index-open-jobs').value='2';r.el('index-open-lod').checked=true;
    await r.approve();const record=JSON.parse(r.saved),req=record.request;
    assert.equal(req.options.jobs,2);assert.equal(req.options.force,false);assert.equal(req.options.lod,true);assert.equal(req.open_seq,'1');
    assert.deepEqual(req.target,{kind:'replace',view_id:id(8),state_rev:'7'});assert.deepEqual(req.pixels,[137,103]);
    assert.match(r.el('index-open-status').textContent,/building/);assert(r.el('index-open-approve').hidden);
    r.ops.set(req.seq,r.terminal(req,'failed'));await r.el('index-open-check').onclick();
    assert.equal(r.saved,null);assert.match(r.el('index-open-status').textContent,/Index succeeded.*not opened.*remain/);r.api.stop();

    const noStore=rig();await noStore.api.init(true);await noStore.open();noStore.saveFail=true;await noStore.approve();
    assert.equal(noStore.calls.filter(c=>c.method==='POST').length,0);assert.match(noStore.el('index-open-status').textContent,/storage/);noStore.api.stop();
    const lost=rig();await lost.api.init(true);await lost.open();lost.lose=true;await lost.approve();
    const saved=lost.saved;assert(saved);assert.equal(lost.done.length,0);lost.api.stop();
    const reload=rig(saved);reload.ops.set(req.seq,reload.terminal(req));reload.last=req.seq;await reload.api.init(true);
    assert.equal(reload.done.length,1);assert.equal(reload.saved,null);assert.equal(reload.calls.filter(c=>c.method==='POST').length,0);reload.api.stop();

    const retry=rig(saved);await retry.api.init(true);assert.match(retry.el('index-open-status').textContent,/unknown/);
    await retry.el('index-open-cancel').onclick();assert.match(retry.el('index-open-status').textContent,/unconfirmed/);assert.equal(retry.saved,saved);
    assert.equal(retry.calls.filter(c=>c.method==='POST').length,0,'unknown admission is never cancelled by starting work');
    await retry.el('index-open-check').onclick();assert.deepEqual(retry.calls.find(c=>c.method==='POST').body,JSON.parse(saved).request);
    await retry.el('index-open-cancel').onclick();assert.equal(retry.saved,null);assert.match(retry.el('index-open-status').textContent,/cancelled.*remain/);retry.api.stop();

    const collision=rig(saved);collision.ops.set(req.seq,{...collision.terminal(req),request_id:id(999)});collision.last=req.seq;
    await collision.api.init(true);await collision.el('index-open-cancel').onclick();
    assert.equal(collision.done.length,0);assert.equal(collision.saved,saved);assert.equal(collision.calls.filter(c=>c.method==='POST').length,0);
    assert.match(collision.el('index-open-status').textContent,/different request/);collision.api.stop();
    const expired=rig(saved);expired.last='40';await expired.api.init(true);await expired.el('index-open-check').onclick();
    assert.equal(expired.calls.filter(c=>c.method==='POST').length,0);assert.equal(expired.saved,saved);assert.match(expired.el('index-open-status').textContent,/expired/);expired.api.stop();

    const partial=rig(saved);partial.ops.set(req.seq,partial.terminal(req,'incomplete','index'));partial.last=req.seq;await partial.api.init(true);
    assert.equal(partial.saved,null);assert.match(partial.el('index-open-status').textContent,/not opened.*Skipped sources: 1/);partial.api.stop();
    const adopt=rig(saved);adopt.completionFail=true;adopt.ops.set(req.seq,adopt.terminal(req));adopt.last=req.seq;await adopt.api.init(true);
    assert(adopt.saved);assert.match(adopt.el('index-open-status').textContent,/view reload failed/);adopt.completionFail=false;
    await adopt.el('index-open-check').onclick();assert.equal(adopt.saved,null);assert.equal(adopt.calls.filter(c=>c.method==='POST').length,0);adopt.api.stop();

    const late=rig();await late.api.init(true);await late.open();late.held=true;const flight=late.approve();
    for(let i=0;i<20&&!late.saved;i++){await new Promise(setImmediate);}assert(late.saved);
    late.api.stop();late.release();await flight;assert.equal(late.done.length,0);assert(late.saved);
    const lateReq=JSON.parse(late.saved).request;late.ops.set(lateReq.seq,late.terminal(lateReq));await late.api.resume();assert.equal(late.saved,null);late.api.stop();

    const invalid=rig('{');await invalid.api.init(true);assert.match(invalid.el('index-open-status').textContent,/invalid/);assert(invalid.api.blocked());
    invalid.el('index-open-close').onclick();assert(invalid.api.blocked(),'close never forgets unknown approval');assert(invalid.el('index-open-dialog').hidden);
    assert.equal(invalid.calls.length,0);invalid.api.stop();
    const keyboard=rig();await keyboard.api.init(true);await keyboard.open();
    let prevented=0;keyboard.doc.activeElement=keyboard.el('index-open-approve');
    keyboard.listeners.keydown({key:'Tab',stopPropagation(){},preventDefault(){prevented++;}});assert.equal(keyboard.doc.activeElement,keyboard.el('index-open-close'));assert(prevented);
    keyboard.listeners.keydown({key:'Escape',isComposing:true,stopPropagation(){},preventDefault(){throw Error('IME');}});assert(keyboard.api.blocked());
    keyboard.listeners.keydown({key:'Escape',stopPropagation(){},preventDefault(){}});assert(!keyboard.api.blocked());
    assert.equal(keyboard.calls.filter(c=>c.method==='POST').length,0);keyboard.api.stop();
    console.log('WEB INDEX OPEN: ALL OK (original selection, consent/force, slots, write-ahead approval, read-only recovery, exact retry, identity, cancel, expiry, late response, partial outcome, keyboard)');
})().catch(e=>{console.error(e);process.exitCode=1;});
