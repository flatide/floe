'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs');
const D=require('./defaults.js'),P=require('./protocol.js'),Q=require('./query.js');
const clone=v=>JSON.parse(JSON.stringify(v));
const flush=async()=>{for(let i=0;i<12;i++)await Promise.resolve();};
function catalog(history=[]){const last=history.at(-1);return {available:true,kind:'design_default',scope:'shared_design_default',max_bytes:'4194304',jobs:1,
    operations:{last_seq:last?last.seq:'0',active:last&&!['succeeded','failed','cancelled'].includes(last.phase)?last.seq:null,history}};}
function op(seq='1',phase='succeeded',extra={}){return {seq,kind:'design_default',phase,...(phase==='queued'?{}:{view_id:'a'.repeat(64),state_rev:'9007199254740993',name:'mask.jb.layerprops'}),
    ...(phase==='succeeded'?{published:true,directory_synced:true}:phase==='failed'||phase==='cancelled'?{published:false,error:phase==='failed'?'io_error':'cancelled'}:{}),...extra};}
function draft(c,extra={}){return {token:'d'.repeat(64),view_id:c.id,state_rev:c.rev,name:'<mask>.chip-by-level.jb.layerprops',bytes:'125',replaces_existing:true,
    expires_in_ms:'30000',scope:'shared_design_default',affects:'future_opens',mode:'chip',levels:['-2','9007199254740993'],title:'<img onerror=bad>',rows:5,...extra};}
function error(status,code){return Object.assign(new Error(code||'network loss'),{status,code});}
function harness(shared={raw:null,model:catalog(),writes:0,records:new Map()}){
    const c={id:'a'.repeat(64),epoch:'b'.repeat(64),rev:'9007199254740993',ready:true,idle:true};
    const nodes=new Map(),timers=new Map(),requests=[];let now=0,serial=0,override=null,storageError=false,session='c'.repeat(64);
    const ids=[...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]);
    function el(id){assert(ids.includes(id),'missing HTML '+id);if(!nodes.has(id))nodes.set(id,{hidden:false,disabled:false,checked:false,textContent:'',focus(){this.focused=true;},set innerHTML(v){throw new Error('HTML injection '+v);}});return nodes.get(id);}
    function execute(r){
        const {method,path,body}=r;
        if(method==='GET')return clone(shared.model);
        if(path.endsWith('/prepare'))return draft(c);
        if(path.endsWith('/revoke'))return null;
        if(path.endsWith('/cancel')){const v=shared.model.operations.history.at(-1);return clone(v);}
        assert.equal(path,'/api/v1/defaults');
        const prior=shared.records.get(body.seq);
        if(prior){assert.deepEqual(body,prior.request,'replay changed the approved body');return clone(prior.result);}
        shared.writes++;const result=op(body.seq,'succeeded',{view_id:body.view_id,state_rev:body.state_rev});
        shared.records.set(body.seq,{request:clone(body),result});shared.model=catalog(shared.model.operations.history.concat(result).slice(-32));return clone(result);
    }
    const panel=D.bind({el,protocol:P,query:Q,context:()=>c,session:()=>session,now:()=>now,
        setTimeout(fn,ms){timers.set(++serial,{fn,at:now+ms});return serial;},clearTimeout:id=>timers.delete(id),
        loadPending(){if(storageError)throw Error('no storage');return shared.raw;},savePending(value){if(storageError)throw Error('no storage');shared.raw=value;},
        async http(method,path,body,missing,token){const r={method,path,body,token};requests.push(r);if(override){const x=override(r);if(x!==undefined)return await x;}return execute(r);}});
    return {panel,c,el,requests,timers,shared,execute,init:()=>panel.init(true),prepare:()=>el('default-prepare').onclick(),
        approve:()=>{el('default-consent').checked=true;el('default-consent').onchange();return el('default-approve').onclick();},
        tick:ms=>{now+=ms;for(const [id,t]of [...timers])if(t.at<=now){timers.delete(id);t.fn();}},
        set override(v){override=v;},set session(v){session=v;},set storageError(v){storageError=v;}};
}
const posts=h=>h.requests.filter(r=>r.method==='POST'&&r.path==='/api/v1/defaults');
(async()=>{
    const h=harness();await h.panel.init(false);assert.equal(h.requests.length,0);assert(h.el('default-panel').hidden);
    await h.init();assert(!h.el('default-prepare').disabled);await h.prepare();
    assert.equal(posts(h).length,0);assert(h.el('default-approve').disabled);
    assert.match(h.el('default-target').textContent,/<img onerror=bad>/);assert.match(h.el('default-levels').textContent,/9007199254740993.*NOT merged/);
    await h.el('default-approve').onclick();assert.equal(posts(h).length,0,'unchecked approval submitted');
    await h.approve();assert.equal(h.shared.writes,1);assert.equal(h.shared.raw,null);assert.match(h.el('default-status').textContent,/Published.*#1/);
    assert.equal(h.requests.filter(r=>r.path.endsWith('/revoke')).length,0,'approved token was revoked');
    await h.prepare();h.c.rev=P.next(h.c.rev);h.panel.changed();await flush();assert(h.el('default-review').hidden);assert.equal(posts(h).length,1);assert(h.requests.some(r=>r.path.endsWith('/revoke')));
    await h.prepare();h.tick(30001);await flush();assert(h.el('default-review').hidden);assert.match(h.el('default-note').textContent,/expired/);
    await h.prepare();h.el('default-review').onkeydown({key:'Escape',preventDefault(){},stopPropagation(){}});assert(h.el('default-review').hidden);assert(h.el('default-prepare').focused);
    for(const change of [()=>h.c.id='e'.repeat(64),()=>h.c.epoch='f'.repeat(64),()=>h.c.idle=false,()=>h.c.ready=false]){
        h.c.idle=h.c.ready=true;await h.prepare();change();h.panel.changed();assert(h.el('default-review').hidden);
    }
    h.panel.stop();assert.equal(h.timers.size,0);

    // Delayed preparation cannot revive a draft after navigation/reconnect.
    const late=harness();await late.init();let reply;late.override=r=>r.path.endsWith('/prepare')?new Promise(resolve=>{reply=resolve;}):undefined;
    const task=late.prepare(),old=draft(late.c);late.c.rev=P.next(late.c.rev);late.panel.changed();reply(old);await task;
    assert(late.el('default-review').hidden);assert.equal(posts(late).length,0);late.panel.stop();

    // Approval accepted but response lost: refresh/reload never submit. Only
    // an explicit Resolve sends exactly the original body, even after view close.
    const lost=harness();await lost.init();await lost.prepare();let drop=true;
    lost.override=r=>{if(r.path==='/api/v1/defaults'&&r.method==='POST'&&drop){drop=false;lost.execute(r);return Promise.reject(error(0));}};
    await lost.approve();assert.equal(lost.shared.writes,1);assert(lost.el('default-prepare').disabled);assert(!lost.el('default-uncertain').hidden);
    const approved=clone(posts(lost)[0].body);assert.deepEqual(JSON.parse(lost.shared.raw).request,approved);
    await lost.panel.refresh();lost.tick(3000);await flush();assert.equal(posts(lost).length,1);lost.panel.stop();
    const reload=harness(lost.shared);reload.c.ready=false;await reload.init();assert.equal(posts(reload).length,0);assert(!reload.el('default-uncertain').hidden);
    await reload.el('default-resolve').onclick();assert.deepEqual(posts(reload)[0].body,approved);assert.equal(reload.shared.writes,1);assert.equal(reload.shared.raw,null);assert(reload.el('default-uncertain').hidden);reload.panel.stop();

    // A response after pagehide is ignored, but its request stays recoverable.
    const hidden=harness();await hidden.init();await hidden.prepare();let finish;
    hidden.override=r=>r.method==='POST'&&r.path==='/api/v1/defaults'?new Promise(resolve=>{finish=()=>resolve(hidden.execute(r));}):undefined;
    hidden.el('default-checked').checked=true;
    const sending=hidden.approve();await flush();assert(finish);assert.equal(hidden.el('default-checked').checked,false);hidden.panel.stop();finish();await sending;
    assert(hidden.shared.raw);await hidden.panel.resume();assert(!hidden.el('default-uncertain').hidden);assert.equal(posts(hidden).length,1);
    hidden.override=null;await hidden.el('default-resolve').onclick();assert.equal(hidden.shared.writes,1);hidden.panel.stop(true);assert.equal(hidden.shared.raw,null);

    // Expired history cannot claim "not published". Clearing is a separate,
    // checked local action, not a publication or automatic retry.
    const expired=harness();await expired.init();await expired.prepare();expired.override=r=>r.method==='POST'&&r.path==='/api/v1/defaults'?Promise.reject(error(410,'operation_expired')):undefined;
    await expired.approve();assert(!expired.el('default-uncertain').hidden);expired.el('default-forget').onclick();assert(expired.shared.raw);
    expired.el('default-checked').checked=true;expired.el('default-checked').onchange();expired.el('default-forget').onclick();assert.equal(expired.shared.raw,null);assert.equal(posts(expired).length,1);expired.panel.stop();

    const denied=harness();await denied.init();await denied.prepare();denied.override=r=>r.method==='POST'&&r.path==='/api/v1/defaults'?Promise.reject(error(409,'stale_state')):undefined;
    await denied.approve();assert(denied.el('default-uncertain').hidden);assert.equal(denied.shared.raw,null);assert.equal(posts(denied).length,1);denied.panel.stop();
    const timed=harness();await timed.init();await timed.prepare();timed.override=r=>r.method==='POST'&&r.path==='/api/v1/defaults'?Promise.reject(error(408,'timeout')):undefined;
    await timed.approve();assert(!timed.el('default-uncertain').hidden);assert(timed.shared.raw);timed.panel.stop();

    // Cancellation is a request, not a fabricated terminal outcome. Late
    // successful commit and durability warning remain successful.
    const cancel=harness();cancel.shared.model=catalog([op('1','publishing')]);await cancel.init();
    cancel.override=r=>{if(r.path.endsWith('/cancel')){cancel.shared.model=catalog([op('1','succeeded',{directory_synced:false})]);return clone(cancel.shared.model.operations.history[0]);}};
    await cancel.el('default-cancel').onclick();assert.match(cancel.el('default-status').textContent,/Published.*#1/);assert.match(cancel.el('default-status').textContent,/Durability is unconfirmed/);assert(cancel.el('default-cancel').hidden);cancel.panel.stop();

    // Another tab publishing during the pre-approval GET invalidates consent.
    const race=harness();await race.init();await race.prepare();race.shared.model=catalog([op()]);await race.approve();assert.equal(posts(race).length,0);assert(race.el('default-review').hidden);race.panel.stop();
    const huge=harness();huge.shared.model=catalog([op('9007199254740993')]);await huge.init();await huge.prepare();await huge.approve();assert.equal(posts(huge)[0].body.seq,'9007199254740994');huge.panel.stop();
    const max=harness();max.shared.model=catalog([op('18446744073709551615')]);await max.init();await max.prepare();await max.approve();assert.equal(posts(max).length,0);max.panel.stop();

    // Malformed/wrong-session stored requests are never transmitted.
    for(const raw of ['x','x'.repeat(2049),JSON.stringify({session_id:'e'.repeat(64),request:approved}),JSON.stringify({session_id:'c'.repeat(64),request:{...approved,path:'/tmp/no'}})]){
        const bad=harness();bad.shared.raw=raw;await bad.init();assert(bad.el('default-prepare').disabled);assert(bad.el('default-resolve').disabled);assert.equal(posts(bad).length,0);bad.panel.stop();
    }
    const unavailable=harness();unavailable.storageError=true;await unavailable.init();assert(unavailable.el('default-prepare').disabled);unavailable.panel.stop();
    for(const change of [v=>v.published=false,v=>v.directory_synced='false',v=>v.path='/x',v=>v.seq=1]){const v=op();change(v);assert.throws(()=>D.operation(v,P));}
    assert.throws(()=>D.operation(op('1','queued',{state_rev:1}),P));
    for(const change of [v=>v.rows=65537,v=>v.bytes='4194305',v=>v.levels=['1','1'],v=>v.levels=['9223372036854775808'],v=>v.state_rev='1',v=>v.path='/x']){const v=draft(h.c);change(v);assert.throws(()=>D.prepared(v,h.c,P,Q));}
    const duplicate=catalog([op(),op()]);assert.throws(()=>D.catalog(duplicate,P));
    console.log('WEB DEFAULTS: ALL OK (read-only preview, checked consent, expiry/context, revoke, same-request reload recovery, no automatic replay, cancel/commit, durability, storage/schema/u64)');
})().catch(e=>{console.error(e);process.exitCode=1;});
