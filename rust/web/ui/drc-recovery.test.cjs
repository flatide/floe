'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs');
const R=require('./drc-recovery.js'),P=require('./protocol.js');
const clone=v=>JSON.parse(JSON.stringify(v)),id=c=>c.repeat(64),C={drc_id:id('a'),revision:id('b'),view_id:id('c')};
const ids=new Set([...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
function catalog(k='notes'){return {kind:'review_recovery',review_kind:k==='notes'?'drc_note':'drc_waive',available:true,reviewer:'synthetic',binding_id:id('d'),operations:{last_seq:'0',active:null,history:[]}};}
function preview(k='notes'){return {kind:'review_recovery',phase:'prepared',review_kind:k==='notes'?'drc_note':'drc_waive',name:'.synthetic.notes.fe',bytes:'23',scope:'exact_staging_link_only',token:id('e'),context:clone(C),review_rev:'0',reviewer:'synthetic',expires_in_ms:'30000'};}
function receipt(phase='succeeded'){return {kind:'review_recovery',seq:'1',phase,scope_id:id('d'),context:clone(C),reader_revision:id('f'),review_rev:'1',recovered:phase==='uncertain'?null:phase==='succeeded',directory_synced:phase==='succeeded'?true:null,error:null,reopen_required:phase!=='failed'};}
const tick=async()=>{for(let i=0;i<40;i++)await Promise.resolve();};
function harness(saved=null){
    const nodes=new Map(),calls=[],timers=new Map();let serial=0,time=100,source=clone(C),session=id('1'),value=catalog(),handler=null,stored=saved,failStore=false,ready=true;
    const el=n=>{assert(ids.has(n),n);if(!nodes.has(n))nodes.set(n,{value:'',hidden:false,disabled:false,checked:false,textContent:''});return nodes.get(n);};
    const api=R.bind({el,protocol:P,context:()=>source,session:()=>session,loadPending:()=>stored,savePending:v=>{if(failStore)throw Error('storage disabled');stored=v;},editors:{notes:{recoveryReady:()=>ready}},
        now:()=>time,setTimeout:(fn,ms)=>{timers.set(++serial,{fn,ms});return serial;},clearTimeout:n=>timers.delete(n),
        http:async(method,path,body,missing,token)=>{calls.push({method,path,body:body&&clone(body),token});if(handler)return handler(method,path,body,token);
            if(method==='GET')return clone(value);if(path.endsWith('/prepare'))return preview(path.includes('/waives/')?'waives':'notes');
            if(path.endsWith('/revoke'))return null;
            value.operations={last_seq:'1',active:null,history:[receipt()]};return receipt();}});
    function attach(write=true,waives=true){api.attach({review_grant:{available:false,reviewer:'synthetic',notes_editable:write,waives_editable:write&&waives},
        notes:{kind:'drc_note',available:true,detached:false,editable:write,reviewer:'synthetic',binding_id:id('d')},
        waives:write&&waives?{kind:'drc_waive',available:true,detached:false,reviewer:'synthetic',binding_id:id('f')}:null});}
    return {api,el,calls,timers,attach,stored:()=>stored,posts:()=>calls.filter(c=>c.method==='POST'&&c.path.endsWith('/recovery')),
        context:c=>{source=c;api.changed();},session:s=>{session=s;api.changed();},handler:h=>{handler=h;},model:m=>{value=m;},time:n=>{time=n;},failStore:()=>{failStore=true;},ready:v=>{ready=v;}};
}
async function prepare(h){h.attach();await tick();await h.el('recovery-prepare').onclick();assert(!h.el('recovery-preview').hidden);assert(h.el('recovery-approve').disabled);h.el('recovery-consent').checked=true;h.el('recovery-consent').onchange();}
(async()=>{
    R.catalog(catalog(),'notes',P);R.preview(preview(),'notes',P);R.operation(receipt(),P);
    for(const change of [v=>v.path='/tmp/x',v=>v.operations.active='1',v=>v.operations.last_seq=1,v=>v.binding_id='bad']){const v=catalog();change(v);assert.throws(()=>R.catalog(v,'notes',P));}
    for(const change of [v=>v.name='../x',v=>v.scope='all',v=>v.token='x',v=>v.expires_in_ms='30001',v=>v.extra=true]){const v=preview();change(v);assert.throws(()=>R.preview(v,'notes',P));}
    assert.throws(()=>R.operation({...receipt('uncertain'),recovered:false},P));
    let h=harness();h.attach(false);await tick();assert.equal(h.calls.length,0);assert(h.el('recovery-panel').hidden);await h.el('recovery-prepare').onclick();assert.equal(h.calls.length,0);h.api.stop();
    h=harness();h.attach(true,false);await tick();assert(h.el('recovery-waives-choice').disabled);h.el('recovery-kind').value='waives';h.el('recovery-kind').onchange();assert.equal(h.el('recovery-kind').value,'notes');assert(!h.el('recovery-panel').hidden);h.api.stop();
    h=harness();await prepare(h);assert.equal(h.posts().length,0);await Promise.all([h.el('recovery-approve').onclick(),h.el('recovery-approve').onclick()]);
    assert.equal(h.posts().length,1);assert.equal(h.posts()[0].body.approve_recovery,true);assert.equal(h.stored(),null);assert.match(h.el('recovery-status').textContent,/Open DRC/);h.api.stop();assert.equal(h.timers.size,0);
    // Expiry/context/permission changes and an existing edit invalidate consent.
    h=harness();await prepare(h);h.time(30101);await h.el('recovery-approve').onclick();assert.equal(h.posts().length,0);h.api.stop();
    h=harness();await prepare(h);h.context({...C,revision:id('2')});await h.el('recovery-approve').onclick();assert.equal(h.posts().length,0);assert(h.el('recovery-preview').hidden);h.api.stop();
    h=harness();await prepare(h);h.attach(false);await h.el('recovery-approve').onclick();assert.equal(h.posts().length,0);h.api.stop();
    h=harness();h.attach();await tick();h.ready(false);await h.el('recovery-prepare').onclick();assert.equal(h.calls.filter(c=>c.method==='POST').length,0);h.api.stop();
    h=harness();await prepare(h);h.failStore();await h.el('recovery-approve').onclick();assert.equal(h.posts().length,0);h.api.stop();
    // A lost approval survives reload. GET of a same-sequence receipt is not
    // proof that this tab's signature was accepted; only explicit replay is.
    h=harness();await prepare(h);h.handler((method,path)=>{if(method==='GET')return clone(catalog());throw Error('response lost');});
    await h.el('recovery-approve').onclick();assert.equal(h.posts().length,1);const saved=h.stored(),sent=clone(h.posts()[0].body);assert(saved);h.api.stop();
    h=harness(saved);h.model({...catalog(),operations:{last_seq:'1',active:null,history:[receipt()]}});h.attach();await tick();assert.equal(h.posts().length,0);assert(!h.el('recovery-resolve').hidden);
    await h.el('recovery-resolve').onclick();assert.deepEqual(h.posts()[0].body,sent);assert.equal(h.stored(),null);h.api.stop();
    h=harness(saved);h.session(id('2'));h.attach();await tick();assert(h.el('recovery-resolve').hidden);assert.equal(h.posts().length,0);h.api.stop();
    // Uncertain results never automatically POST, including resume/timers.
    h=harness();h.model({...catalog(),operations:{last_seq:'1',active:'1',history:[receipt('uncertain')]}});h.attach();await tick();assert(!h.el('recovery-check').hidden);assert.equal(h.timers.size,0);h.api.stop();h.api.resume();await tick();assert.equal(h.posts().length,0);
    await h.el('recovery-check').onclick();assert.equal(h.calls.filter(c=>c.path.endsWith('/reconcile')).length,1);assert.equal(h.posts().length,0);h.api.stop();
    // Late preview and approval ACKs cannot revive a stopped controller.
    h=harness();h.attach();await tick();let resolve;h.handler(()=>new Promise(r=>{resolve=r;}));const p=h.el('recovery-prepare').onclick();h.api.stop();resolve(preview());await p;assert(h.el('recovery-preview').hidden);assert.equal(h.posts().length,0);
    h=harness();await prepare(h);h.handler(method=>method==='GET'?clone(catalog()):new Promise(r=>{resolve=r;}));const a=h.el('recovery-approve').onclick();await tick();assert(h.stored());h.api.stop();resolve(receipt());await a;assert(h.stored());assert.equal(h.timers.size,0);
    console.log('WEB REVIEW RECOVERY UI: ALL OK (strict wire, explicit consent, no autosave, lost ACK/reload, same-token replay, context/permission/expiry fences, read-only reconcile)');
})().catch(e=>{console.error(e);process.exitCode=1;});
module.exports={catalog,preview,receipt};
