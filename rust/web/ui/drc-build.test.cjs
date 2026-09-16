'use strict';
const assert = require('node:assert/strict'), B = require('./drc-build.js'), P = require('./protocol.js');
const nodes = new Map(), requests = [], timers = new Map(), adopted = [];
let serial = 0, retired = 0, focused = '', view = {id:'view',source:'source',connected:true,pending:false,state:{status:'idle',connection_epoch:'epoch1'}};
function el(id) {
    if (!nodes.has(id)) nodes.set(id, {hidden:false,disabled:false,value:'',checked:false,textContent:'',
        setAttribute(k,v){this[k]=v;},focus(){focused=id;}});
    return nodes.get(id);
}
function http(method,path,body,missing,token) {
    return new Promise((resolve,reject)=>{
        const q={method,path,body,token,done:false,resolve,reject};requests.push(q);
        token.abort=()=>{q.done=true;reject(new Error('aborted'));};
    });
}
const controller = B.bind({document:{getElementById:el},protocol:P,http,context:()=>view,
    setTimeout(fn,ms){timers.set(++serial,{fn,ms});return serial;},clearTimeout(id){timers.delete(id);},
    changed:v=>adopted.push(v),retire(){retired++;}});
const cat=(seq='0',op=null,id='drc1',phase='ready')=>({drc:id?{id,revision:id+'r',source_id:'source',title:'private <db>',phase}:null,
    build:{available:true,source_id:'source',jobs_min:1,jobs_max:16,jobs_default:4,
        operations:{last_seq:seq,active:op&&!['succeeded','failed','cancelled'].includes(op.phase)?seq:null,history:op?[op]:[]}}});
const op=(seq,phase,extra={})=>({seq,kind:'drc_build',phase,...extra});
const queue=()=>requests.filter(q=>!q.done);
function next(method='GET',path='/api/v1/drc') {const q=queue().find(q=>q.method===method&&q.path===path);assert(q,method+' '+path+' missing');return q;}
function reply(q,v){q.done=true;q.token.abort=null;q.resolve(v);}
function fail(q,status){q.done=true;q.token.abort=null;q.reject(Object.assign(new Error(status?'HTTP '+status:'network unavailable'),status?{status}:{}));}
async function tick(){for(let i=0;i<30;i++)await Promise.resolve();}
async function refresh(v){const p=controller.refresh();reply(next(),v);await p;}
const postCount=()=>requests.filter(q=>q.method==='POST').length;
function open(){el('drc-build-open').onclick();}
function approve(){return el('drc-build-form').onsubmit({preventDefault(){}});}
async function submitted(v=cat()) {
    open();const p=approve();reply(next(),v);await tick();return {p,q:next('POST','/api/v1/drc/builds')};
}
(async()=>{
    await refresh(cat());assert.equal(postCount(),0);assert(!el('drc-build-open').disabled);
    open();assert.equal(focused,'drc-build-jobs');assert.equal(el('drc-build-jobs').value,'4');assert.equal(el('drc-build-force').checked,false);
    assert.equal(el('drc-build-source').textContent,'private <db>');assert(controller.escape());assert(!controller.escape());
    assert.equal(focused,'drc-build-open');assert.equal(postCount(),0);
    open();el('drc-build-force').checked=true;el('drc-build-dismiss').onclick();open();assert(!el('drc-build-force').checked,'force approval carried over');
    for(const value of ['0','17','1.5','1e1',' 4','NaN']){el('drc-build-jobs').value=value;await approve();assert.equal(postCount(),0);}
    controller.escape();
    // An approval cannot outlive its view, connection epoch or DRC revision.
    open();view={...view,connected:false};controller.contextChanged();assert(el('drc-build-form').hidden);await approve();assert.equal(postCount(),0);
    view={...view,connected:true};controller.contextChanged();open();const changed=approve();view={...view,state:{...view.state,connection_epoch:'epoch2'}};
    reply(next(),cat());await changed;assert.equal(postCount(),0);assert(el('drc-build-form').hidden);
    const high='9007199254740993';await refresh(cat(high,op(high,'succeeded')));
    const running=op(P.next(high),'running',{elapsed_ms:'9012',native:{checks:'1',total_checks:'2',errors:high,output_bytes:'0',dropped_lines:'0'}});
    const sent=await submitted(cat(high,op(high,'succeeded')));
    assert.deepEqual(sent.q.body,{seq:running.seq,drc_id:'drc1',revision:'drc1r',view_id:'view',approve:true,force:false,jobs:4});
    assert(controller.suspended());assert(retired>0);assert(el('drc-build-open').disabled);
    const n=postCount();await approve();assert.equal(postCount(),n);
    await refresh(cat());assert(controller.suspended());assert.equal(adopted.at(-1).drc,null);
    reply(sent.q,running);await tick();reply(next(),cat(running.seq,running,null));await sent.p;
    assert(!el('drc-build').hidden);assert(!el('drc-build-cancel').hidden);assert.match(el('drc-build-status').textContent,/1\/2 checks reported/);
    assert.match(el('drc-build-status').textContent,new RegExp(high));assert.equal(adopted.at(-1).drc,null);
    assert.equal([...timers.values()][0].ms,500);
    // Cancellation requests target one seq, never a newer job. Publication can win.
    const cancel=el('drc-build-cancel').onclick();assert(el('drc-build-cancel').disabled);
    const cq=next('POST','/api/v1/drc/builds/'+running.seq+'/cancel');
    const success=op(running.seq,'succeeded',{outcome:{reused:false,checks:'1',errors:'2',bytes:'500',directory_synced:false},cleanup_warning:true});
    reply(cq,success);await tick();reply(next(),cat(success.seq,success,'drc2','opening'));await cancel;
    assert.match(el('drc-build-status').textContent,/Pack ready/);assert.match(el('drc-build-status').textContent,/durability/);
    assert.match(el('drc-build-status').textContent,/cleanup/);assert.match(el('drc-build-review').textContent,/Opening/);
    await refresh(cat(success.seq,success,'drc2'));assert(!controller.suspended());assert.equal(adopted.at(-1).drc.id,'drc2');
    // Unknown receipt: GET/resume never sends a new operation. Explicit retry
    // retains the exact original seq, force, jobs, source/view/review identity.
    const delayed=await submitted(cat(success.seq,success,'drc2'));const body={...delayed.q.body};fail(delayed.q);await tick();
    reply(next(),cat(success.seq,success,'drc2'));await delayed.p;
    assert(!el('drc-build-resolve').hidden);assert(el('drc-build-open').disabled);const before=postCount();
    controller.stop();assert.equal(timers.size,0);const resumed=controller.resume();reply(next(),cat(success.seq,success,'drc3'));await resumed;
    assert.equal(postCount(),before);assert(controller.suspended());
    const resolved=el('drc-build-resolve').onclick();const repeated=next('POST','/api/v1/drc/builds');assert.deepEqual(repeated.body,body);
    fail(repeated,409);await tick();reply(next(),cat(success.seq,success,'drc3'));await resolved;
    assert(el('drc-build-resolve').hidden);assert(!controller.suspended());assert.match(el('drc-build-note').textContent,/409/);
    open();el('drc-build-force').checked=true;el('drc-build-jobs').value='16';const forced=approve();
    assert(!controller.suspended(),'read-only preflight retired the review');reply(next(),cat(success.seq,success,'drc3'));await tick();
    const forcedRequest=next('POST','/api/v1/drc/builds');assert.equal(forcedRequest.body.jobs,16);assert(forcedRequest.body.force);
    fail(forcedRequest,429);await tick();reply(next(),cat(success.seq,success,'drc3'));await forced;
    assert(!controller.suspended());assert(el('drc-build-resolve').hidden);assert.match(el('drc-build-note').textContent,/429/);
    const lost=await submitted(cat(success.seq,success,'drc3'));controller.stop();await lost.p;assert(lost.q.token.cancelled);
    const beforeResume=postCount();const resume=controller.resume();reply(next(),cat(success.seq,success,'drc4'));await resume;
    assert.equal(postCount(),beforeResume);assert(!el('drc-build-resolve').hidden);
    const retry=el('drc-build-resolve').onclick();const rq=next('POST','/api/v1/drc/builds');reply(rq,op(rq.body.seq,'cancelled'));
    await tick();reply(next(),cat(rq.body.seq,op(rq.body.seq,'cancelled'),'drc4'));await retry;
    assert(!controller.suspended());
    // Late GETs cannot re-adopt old revisions; failed polling suspends overlays.
    const oldRefresh=controller.refresh(),old=next();await refresh(cat(rq.body.seq,op(rq.body.seq,'cancelled'),'drc5'));
    reply(old,cat());await oldRefresh;assert.equal(adopted.at(-1).drc.id,'drc5');
    const missing=controller.refresh();fail(next());await missing;assert(controller.suspended());assert.equal(adopted.at(-1).drc,null);
    await refresh(cat(rq.body.seq,op(rq.body.seq,'cancelled'),'drc5'));assert(!controller.suspended());
    const readonly=cat();readonly.build.available=false;await refresh(readonly);assert(el('drc-build-open').hidden);assert.match(el('drc-build-note').textContent,/read-only/);
    const total=postCount();open();await approve();assert.equal(postCount(),total);
    await refresh({drc:null});assert(el('drc-build').hidden);assert.equal(timers.size,0);
    await refresh(cat());open();const preflight=approve();controller.stop();await preflight;
    const resumedPreflight=controller.resume();reply(next(),cat());await resumedPreflight;assert(!el('drc-build-open').disabled,'stopped preflight kept UI busy');
    const expired=controller.refresh();fail(next(),401);await expired;assert.equal(timers.size,0);assert(controller.suspended());controller.stop();
    for(const v of [null,{},cat('2',op('1','running')),cat('0',op('1','succeeded')),cat('1',op('1','unknown')),
        cat('1',op('1','running',{elapsed_ms:1})),cat('1',op('1','succeeded',{outcome:{}}))])assert.throws(()=>B.validate(v,P));
    const tooMany=cat('33',op('33','succeeded'));tooMany.build.operations.history=Array.from({length:33},(_,i)=>op(String(i+1),'succeeded'));assert.throws(()=>B.validate(tooMany,P));
    assert.match(B.status(op('1','failed',{noninteger:true})),/Fractional/);
    assert.match(B.status(op('1','failed',{migration:{directory_synced:true}})),/Legacy pack renamed/);
    assert.match(B.status(op('1','cancelled',{migration:{directory_synced:false}})),/directory sync failed/);
    for(const migration of [{},true,{directory_synced:'yes'},{directory_synced:true,path:'/private'}]) {
        assert.throws(()=>B.validate(cat('1',op('1','failed',{migration})),P));
    }
    assert.match(B.status(op('1','succeeded',{outcome:{reused:true,checks:'1',errors:'2',bytes:'500',directory_synced:true}})),/reused/);
    console.log('WEB DRC BUILD UI: ALL OK (approval, u64, identity, progress, cancel/commit, unknown receipt, retry/stop/resume, read-only, malformed DTO)');
})().catch(e=>{controller.stop();console.error(e);process.exitCode=1;});
