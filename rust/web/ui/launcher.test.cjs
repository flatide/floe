'use strict';
const assert=require('node:assert/strict'), Launcher=require('./launcher.js'), P=require('./protocol.js');
const tick=()=>new Promise(setImmediate);
async function wait(test){for(let n=0;n<100;n++){if(test()){return;}await tick();}throw Error('launcher did not progress');}
function rig(){
    const nodes=new Map(),el=id=>{if(!nodes.has(id)){nodes.set(id,{});}return nodes.get(id);};
    let snapshot={revision:'0',pending:null},ready=true,saved=null,receipt=null,failPost=false,failGet=false;
    let pollResolve=null,postResolve=null,holdPost=false;
    const calls=[],opens=[],timers=new Map();let next=0,completed=0,presents=0,prepared=0;
    const env={el,protocol:P,ready:()=>ready,changed(){},loadPending:()=>saved,savePending:v=>{saved=v;},
        setTimeout(fn,ms){const id=++next;timers.set(id,{fn,ms});return id;},clearTimeout:id=>timers.delete(id),
        prepare:async()=>{prepared++;},present:()=>presents++,completed:async()=>completed++,
        open:async(item,send)=>{opens.push(item.id);await send({action:'open',seq:'9007199254740993',pixels:[812,800],levels:{mode:'all'}});},
        http:async(method,path,input,missing,token)=>{
            calls.push({method,path,input});
            if(method==='POST'){
                assert(saved,'journal must precede HTTP mutation');
                if(holdPost){await new Promise(r=>postResolve=r);}
                if(failPost){throw Error('lost ACK');}
                receipt={phase:input.action==='open'?'submitted':input.action==='present'?'presented':'dismissed'};
                return receipt;
            }
            if(path.startsWith('/api/v1/launch/poll/')){
                return new Promise((resolve,reject)=>{pollResolve=resolve;token.abort=()=>reject(Error('aborted'));});
            }
            if(path==='/api/v1/launch'){return snapshot;}
            if(failGet){throw Error('lost read');}
            return {phase:'ready',receipt};
        }};
    const api=Launcher.bind(env);
    return {api,el,calls,opens,env,get saved(){return saved;},get completed(){return completed;},get presents(){return presents;},get prepared(){return prepared;},
        set ready(v){ready=v;},set saved(v){saved=v;},set receipt(v){receipt=v;},set failPost(v){failPost=v;},set failGet(v){failGet=v;},set holdPost(v){holdPost=v;},
        releasePost(){postResolve();},
        async update(value){snapshot=value;if(pollResolve){const r=pollResolve;pollResolve=null;r(value);}else{await this.runTimer();}await tick();},
        async runTimer(){const it=timers.entries().next().value;if(it){timers.delete(it[0]);it[1].fn();}await tick();}};
}
const item=(id='a',confirm=false)=>({revision:'2',pending:{id:id.repeat(64),phase:'ready',request:{source_id:'opaque',body:{}},confirm_levels:confirm}});
(async()=>{
    const disabled=rig();await disabled.api.init(false);assert.equal(disabled.calls.length,0);
    const r=rig();r.ready=false;await r.api.init(true);await r.runTimer();await r.update(item());
    assert.equal(r.opens.length,0);assert.equal(r.prepared,0);r.ready=true;
    for(let i=0;i<4;i++){await r.runTimer();}await wait(()=>r.completed===1);
    assert.equal(r.opens.length,1);assert.equal(r.saved,null);assert(!r.api.blocked());
    await r.runTimer();await r.update(item());assert.equal(r.opens.length,1,'late poll must not resurrect a retired proposal');r.api.stop();
    const levels=rig();await levels.api.init(true);await levels.runTimer();await levels.update(item('b',true));
    await wait(()=>levels.prepared===1);assert.equal(levels.opens.length,0);assert(levels.api.blocked());
    levels.el('launch-open').onclick();await wait(()=>levels.completed===1);levels.api.stop();
    const lost=rig();lost.failPost=true;lost.failGet=true;await lost.api.init(true);await lost.runTimer();await lost.update(item('c'));
    await wait(()=>lost.el('launch-check').disabled===false&&!lost.el('launch-check').hidden);
    const original=JSON.parse(lost.saved);assert.equal(lost.calls.filter(x=>x.method==='POST').length,1);
    assert(lost.el('launch-dismiss').disabled);lost.el('launch-dismiss').onclick();await tick();
    assert.deepEqual(JSON.parse(lost.saved),original);lost.api.stop();
    const reload=rig();reload.saved=lost.saved;await reload.api.init(true);
    assert.equal(reload.calls.filter(x=>x.method==='POST').length,0,'resume is read-only');
    reload.el('launch-check').onclick();await wait(()=>reload.completed===1);
    assert.deepEqual(reload.calls.find(x=>x.method==='POST').input,original.input);reload.api.stop();
    const committed=rig();committed.saved=lost.saved;committed.receipt={phase:'submitted'};await committed.api.init(true);
    assert.equal(committed.completed,1);assert.equal(committed.calls.filter(x=>x.method==='POST').length,0);committed.api.stop();
    const present=rig();await present.api.init(true);await present.runTimer();const p=item('d');p.pending.request=null;await present.update(p);
    await wait(()=>present.presents===1);assert.equal(present.opens.length,0);assert.equal(present.completed,0);present.api.stop();
    const slow=rig();slow.holdPost=true;await slow.api.init(true);await slow.runTimer();await slow.update(item('e'));
    await wait(()=>slow.saved);slow.api.stop();slow.releasePost();await wait(()=>slow.completed===1);
    await slow.api.resume();assert.equal(slow.opens.length,1);slow.api.stop();
    console.log('WEB LAUNCHER: ALL OK (queued input, level confirmation, stale polls, journal-before-send, read-only recovery, identical retry, present, pause)');
})().catch(e=>{console.error(e);process.exit(1);});
