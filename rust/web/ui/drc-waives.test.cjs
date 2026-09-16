'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs');
const W=require('./drc-waives.js'),P=require('./protocol.js');
const API='/api/v1/drc/review/waives',clone=v=>JSON.parse(JSON.stringify(v));
const context={drc_id:'a'.repeat(64),revision:'b'.repeat(64),view_id:'c'.repeat(64)};
const flush=async()=>{for(let i=0;i<50;i++)await Promise.resolve();};
function op(seq='1',phase='succeeded',extra={}){
    const done=['succeeded','failed','cancelled'].includes(phase),saved=['succeeded','refreshing_reader'].includes(phase);
    return {seq,kind:'drc_waive',phase,...(phase==='queued'?{}:{context:clone(context),elapsed_ms:'3',error:phase==='failed'?'review_changed':null,
        published:saved,outcome_unknown:false,directory_synced:saved?true:null}),...(done?{review_rev:saved?'1':'0',reader_applied:saved,reader_revision:seq.padStart(64,'0')}:{}),...extra};
}
function catalog(history=[],rev){const last=history.at(-1);return {available:true,kind:'drc_waive',reviewer:'fixed-owner',review_rev:rev||(last&&last.review_rev)||'0',
    operations:{last_seq:last?last.seq:'0',active:last&&!['succeeded','failed','cancelled'].includes(last.phase)?last.seq:null,history},note_bytes:65536,selection_limit:5000,preparing:false,autosave:false};}
function snapshot(c,count=2,extra={}){return {kind:'drc_waive',phase:'snapshot',name:'.synthetic.db.waive.fixed-owner',context:clone(c),token:'d'.repeat(64),
    review_rev:'0',reviewer:'fixed-owner',expires_in_ms:'120000',selected_count:String(count),waived_count:'0',reserved_count:'0',exists:false,legacy_unverified:false,...extra};}
function prepared(c,count=2,waived=true,extra={}){const v=snapshot(c,count);delete v.waived_count;delete v.exists;return {...v,phase:'prepared',token:'e'.repeat(64),
    expires_in_ms:'30000',waived,changed_count:String(count),replaces_existing:false,scope:'registered_reviewer_waives',...extra};}
function error(status,code){return Object.assign(new Error(code||'network lost'),{status,code});}
function harness(shared={model:catalog(),raw:null,writes:0,records:new Map()}){
    const ids=new Set([...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
    const nodes=new Map(),calls=[],timers=new Map(),pauses=[];let now=0,serial=0,override=null,storageError=false,session='f'.repeat(64),reviewReads=0,panel;
    const reader={id:context.drc_id,revision:context.revision,phase:'ready'};
    const c={context:clone(context),epoch:'9'.repeat(64),key:'groups:1',caption:'2 selected errors across rules',ready:true,
        rows:[{check:'0',error:'9007199254740993'},{check:'7',error:'0'}]};
    function el(id){assert(ids.has(id),'missing HTML '+id);if(!nodes.has(id))nodes.set(id,{value:'',textContent:'',disabled:false,hidden:false,checked:false,
        focus(){if(!this.disabled&&!this.hidden&&!el('waives-panel').hidden&&
            (!['waives-action','waives-consent'].includes(id)||!el('waives-editor').hidden)&&
            (id!=='waives-consent'||!el('waives-review').hidden)){this.focused=true;}},set innerHTML(_){throw Error('HTML injection');}});return nodes.get(id);}
    function execute(r){
        if(r.method==='GET')return clone(shared.model);
        if(r.path.endsWith('/read'))return snapshot(r.body.context,r.body.errors.length,{review_rev:shared.model.review_rev});
        if(r.path.endsWith('/prepare'))return prepared(r.body.context,c.rows.length,r.body.waived,{review_rev:shared.model.review_rev});
        if(r.path.endsWith('/revoke'))return null;
        if(r.path.endsWith('/cancel'))return clone(shared.model.operations.history.at(-1));
        assert.equal(r.path,API);const prior=shared.records.get(r.body.seq);
        if(prior){assert.deepEqual(r.body,prior.request,'replayed a different approval');return clone(prior.result);}
        assert(panel.suspended(),'read suspension must precede write submission');
        const result=op(r.body.seq,'succeeded',{context:clone(r.body.context),review_rev:P.next(shared.model.review_rev)});
        const binding=shared.model.binding_id;if(binding){result.scope_id=binding;}
        shared.records.set(r.body.seq,{request:clone(r.body),result});shared.writes++;
        shared.model=catalog(shared.model.operations.history.concat([result]).slice(-32));if(binding){shared.model.binding_id=binding;}return clone(result);
    }
    panel=W.bind({el,protocol:P,session:()=>session,now:()=>now,
        connection:()=>c.connected===false?null:c.epoch,
        selection:()=>c.ready&&!(panel&&panel.suspended())?{context:clone(c.context),epoch:c.epoch,key:c.key,caption:c.caption,count:c.rows.length,references:()=>clone(c.rows)}:null,
        changed(){if(panel){pauses.push(panel.suspended());panel.changed();}},refreshReview(){reviewReads++;},
        setTimeout(fn,ms){timers.set(++serial,{fn,at:now+ms});return serial;},clearTimeout:id=>timers.delete(id),
        loadPending(){if(storageError)throw Error('no storage');return shared.raw;},savePending(v){if(storageError)throw Error('no storage');shared.raw=v;},
        async http(method,path,body,missing,token){const r={method,path,body,token};calls.push(r);if(override){const v=override(r);if(v!==undefined)return await v;}return execute(r);}});
    return {panel,el,c,reader,calls,timers,shared,pauses,execute,init:()=>panel.attach(clone(shared.model),clone(reader)),read:()=>el('waives-read').onclick(),prepare:()=>el('waives-prepare').onclick(),
        choose(v){el('waives-action').value=v;return el('waives-action').onchange();},approve(legacy=false){el('waives-consent').checked=true;el('waives-legacy').checked=legacy;el('waives-consent').onchange();return el('waives-approve').onclick();},
        sync(){reader.revision=shared.model.operations.history.at(-1).reader_revision||reader.revision;c.context.revision=reader.revision;panel.attach(clone(shared.model),clone(reader));},
        tick(ms){now+=ms;for(const[id,t]of[...timers])if(t.at<=now){timers.delete(id);t.fn();}},
        get reviewReads(){return reviewReads;},set override(v){override=v;},set session(v){session=v;},set storageError(v){storageError=v;}};
}
const writes=h=>h.calls.filter(r=>r.method==='POST'&&r.path===API);
async function test(){
    for(const waived of ['0','1','2']){
        const key=harness();assert.equal(key.panel.open(),false);key.init();
        key.override=r=>r.path.endsWith('/read')?snapshot(r.body.context,2,{waived_count:waived}):undefined;
        assert(key.panel.open());assert(key.panel.open());await flush();
        assert.equal(key.calls.filter(r=>r.path.endsWith('/read')).length,1);
        assert.equal(key.el('waives-action').value,waived==='2'?'clear':'waive');
        assert(key.el('waives-action').focused,'snapshot must reveal and enable editor before focus');
        assert.equal(writes(key).length,0);assert.equal(key.calls.filter(r=>r.path.endsWith('/prepare')).length,0);
        key.choose('clear');key.panel.open();assert.equal(key.el('waives-action').value,'clear','key overwrote existing draft');
        key.panel.stop();assert.equal(key.panel.open(),false);
    }
    const imported=harness();imported.init();imported.c.ready=false;imported.panel.transferLock(true);
    const whole={context:clone(context),token:'e'.repeat(64),reviewer:'fixed-owner',review_rev:'0'};
    assert(imported.panel.transferReady(true));assert.equal(await imported.panel.publishTransfer(whole,()=>false),false);assert.equal(writes(imported).length,0);
    let dropImport=true;imported.override=r=>{if(r.path===API&&r.method==='POST'&&dropImport){dropImport=false;imported.execute(r);return Promise.reject(error(0));}};
    await imported.panel.publishTransfer(whole,()=>true);assert.equal(imported.shared.writes,1);assert(imported.panel.suspended());assert(imported.shared.raw);
    await imported.el('waives-resolve').onclick();assert.equal(imported.shared.writes,1);imported.sync();assert(!imported.panel.suspended());assert.equal(imported.shared.raw,null);
    assert.equal(writes(imported)[0].body.confirm_legacy,true);imported.panel.stop(true);
    const actionHtml=fs.readFileSync(__dirname+'/index.html','utf8').match(/<select id="waives-action">([\s\S]*?)<\/select>/)[1];
    assert.deepEqual([...actionHtml.matchAll(/<option value="([^"]*)">([^<]+)<\/option>/g)].map(m=>[m[1],m[2]]),[['','Choose an action…'],['waive','Waive'],['clear','Clear waive']]);
    const h=harness();h.panel.attach(null,h.reader);assert(h.el('waives-panel').hidden);assert.equal(h.calls.length,0);h.init();await h.read();
    assert.deepEqual(h.calls.at(-1).body.errors,h.c.rows);assert(h.el('waives-prepare').disabled);await h.prepare();assert.equal(h.calls.length,1);
    h.choose('waive');await h.prepare();assert.equal(writes(h).length,0);assert.match(h.el('waives-preview').textContent,/Waive 2/);assert(h.el('waives-approve').disabled);
    assert(h.el('waives-consent').focused,'preview must enable consent before focus');
    await h.el('waives-approve').onclick();assert.equal(writes(h).length,0);await h.approve();await flush();
    assert.equal(h.shared.writes,1);assert.equal(h.shared.raw,null);assert.equal(h.el('waives-action').value,'');assert(h.panel.suspended());assert(h.reviewReads>0);
    assert.match(h.el('waives-status').textContent,/file saved/);assert.match(h.el('waives-status').textContent,/Reader updated/);
    h.panel.attach(clone(h.shared.model),clone(h.reader));assert(h.panel.suspended(),'old catalog reopened old statuses');h.sync();assert(!h.panel.suspended());assert.deepEqual(h.pauses,[true,false]);
    await h.read();h.choose('clear');await h.prepare();assert.match(h.el('waives-preview').textContent,/Clear waive on/);
    const n=h.calls.length;h.c.pan='zoom';h.panel.changed();assert.equal(h.calls.length,n);assert(!h.el('waives-review').hidden);
    h.c.key='groups:2';h.panel.changed();await flush();assert(h.el('waives-review').hidden);assert.equal(h.el('waives-action').value,'clear');assert(h.el('waives-prepare').disabled);
    h.c.key='groups:1';await h.el('waives-reload').onclick();assert.equal(h.el('waives-action').value,'clear');h.tick(120001);assert(h.el('waives-prepare').disabled);
    await h.el('waives-reload').onclick();await h.prepare();h.tick(30001);assert(h.el('waives-review').hidden);
    for(const change of [()=>h.c.context.revision='2'.repeat(64),()=>h.c.epoch='3'.repeat(64),()=>h.c.ready=false]){
        h.el('waives-discard').onclick();h.c.ready=true;await h.read();h.choose('waive');await h.prepare();change();h.panel.changed();assert(h.el('waives-review').hidden);assert.equal(writes(h).length,1);
    }
    h.panel.stop();assert.equal(h.timers.size,0);assert(h.el('waives-paused').hidden);assert(h.el('waives-read').disabled);

    const legacy=harness();legacy.override=r=>r.path.endsWith('/read')?snapshot(r.body.context,2,{waived_count:'1',reserved_count:'1'}):
        r.path.endsWith('/prepare')?prepared(r.body.context,2,r.body.waived,{legacy_unverified:true,replaces_existing:true,reserved_count:'1'}):undefined;
    legacy.init();await legacy.read();legacy.choose('clear');await legacy.prepare();assert.match(legacy.el('waives-reserved').textContent,/1 selected reserved.*0/);
    await legacy.approve();assert.equal(writes(legacy).length,0);await legacy.approve(true);assert.equal(writes(legacy)[0].body.confirm_legacy,true);legacy.panel.stop();

    for(const state of [false,null]){
        const failed=harness();failed.init();await failed.read();failed.choose('waive');await failed.prepare();
        failed.override=r=>{if(r.path===API&&r.method==='POST'){const v=op('1','succeeded',{reader_applied:state,reader_error:state===null?'drc_apply_unknown':'review_changed',directory_synced:false});failed.shared.model=catalog([v]);return v;}};
        await failed.approve();failed.sync();assert(failed.panel.suspended());assert.match(failed.el('waives-status').textContent,/file saved/);assert.match(failed.el('waives-status').textContent,/Reopen/);
        assert.match(failed.el('waives-status').textContent,/durability/);await failed.panel.refresh();assert.equal(writes(failed).length,1);
        failed.reader.id='9'.repeat(64);failed.panel.attach(clone(failed.shared.model),clone(failed.reader));assert(!failed.panel.suspended(),'new geometry stayed blocked by old receipt');failed.panel.stop();
    }
    const cancelled=harness();cancelled.init();await cancelled.read();cancelled.choose('waive');await cancelled.prepare();
    cancelled.override=r=>{if(r.path===API&&r.method==='POST'){const v=op('1','publishing');cancelled.shared.model=catalog([v]);return v;}
        if(r.path.endsWith('/cancel')){const v=op('1','cancelled');cancelled.shared.model=catalog([v]);return v;}};
    await cancelled.approve();assert(cancelled.panel.suspended());await cancelled.el('waives-cancel').onclick();assert(cancelled.panel.suspended());cancelled.sync();assert(!cancelled.panel.suspended());
    assert.equal(cancelled.el('waives-action').value,'waive');assert.match(cancelled.el('waives-status').textContent,/Cancelled before/);cancelled.panel.stop();

    const lost=harness();lost.init();await lost.read();lost.choose('waive');await lost.prepare();let drop=true;
    lost.override=r=>{if(r.path===API&&r.method==='POST'&&drop){drop=false;lost.execute(r);return Promise.reject(error(0));}};
    await lost.approve();assert.equal(lost.shared.writes,1);assert(lost.panel.suspended());assert(!lost.shared.raw.includes('9007199'));const request=clone(writes(lost)[0].body);
    await lost.panel.refresh();assert.equal(writes(lost).length,1);lost.panel.stop();const again=harness(lost.shared);again.init();await again.panel.refresh();assert.equal(writes(again).length,0);
    assert(again.panel.suspended());again.c.ready=false;await again.el('waives-resolve').onclick();assert.deepEqual(writes(again)[0].body,request);assert.equal(again.shared.writes,1);
    again.sync();assert(!again.panel.suspended());assert.equal(again.shared.raw,null);again.panel.stop();

    const conflict=harness();conflict.init();await conflict.read();conflict.choose('clear');await conflict.prepare();
    conflict.override=r=>{if(r.path===API&&r.method==='POST'){conflict.shared.model=catalog([op()]);return Promise.reject(error(409,'operation_conflict'));}};
    await conflict.approve();assert.equal(conflict.el('waives-action').value,'clear');assert.equal(conflict.shared.raw,null);conflict.sync();assert(!conflict.panel.suspended());conflict.panel.stop();
    const unknown=harness();unknown.init();await unknown.read();unknown.choose('waive');await unknown.prepare();
    unknown.override=r=>{if(r.path===API&&r.method==='POST'){const v=op('1','failed',{published:null,outcome_unknown:true,reader_applied:null,error:'review_unavailable',review_rev:'1'});unknown.shared.model=catalog([v]);return v;}};
    await unknown.approve();unknown.sync();assert(unknown.panel.suspended());assert.match(unknown.el('waives-status').textContent,/UNKNOWN/);unknown.panel.stop();

    for(const kind of ['read','prepare']){
        const late=harness();late.init();if(kind==='prepare'){await late.read();late.choose('waive');}let reply;
        late.override=r=>r.path.endsWith('/'+kind)?new Promise(resolve=>{reply=()=>resolve(kind==='read'?snapshot(r.body.context):prepared(r.body.context,2,r.body.waived));}):undefined;
        const task=kind==='read'?late.read():late.prepare();late.c.key='changed';late.panel.changed();reply();await task;await flush();
        assert(late.el('waives-review').hidden);assert.equal(writes(late).length,0);assert(late.calls.some(r=>r.path.endsWith('/revoke')));late.panel.stop();
    }
    const storage=harness();storage.storageError=true;storage.init();assert(storage.panel.suspended());assert(!storage.el('waives-uncertain').hidden);
    storage.el('waives-checked').checked=true;storage.el('waives-checked').onchange();storage.el('waives-forget').onclick();assert.match(storage.el('waives-message').textContent,/storage unavailable/);storage.panel.stop();
    const invalid=harness();invalid.init();invalid.panel.attach({...catalog(),autosave:true},invalid.reader);assert(invalid.panel.suspended());
    invalid.init();assert(!invalid.panel.suspended());invalid.panel.attach(null,invalid.reader);assert(invalid.panel.suspended());invalid.init();assert(!invalid.panel.suspended());
    invalid.override=r=>r.method==='GET'?Promise.reject(error(0)):undefined;await invalid.panel.refresh();assert(invalid.panel.suspended());invalid.override=null;await invalid.panel.refresh();assert(!invalid.panel.suspended());invalid.panel.stop();
    for(const v of [op(),op('1','refreshing_reader',{reader_applied:false}),op('1','failed'),op('1','queued'),op('18446744073709551615')])W.operation(v,P);
    for(const patch of [{reader_applied:'true'},{reader_applied:true,reader_revision:undefined},{phase:'refreshing_reader',reader_applied:true},{path:'/private/file'},{seq:1}])assert.throws(()=>W.operation({...op(),...patch},P));
    assert.throws(()=>W.preview(snapshot(context,2,{reserved_count:'2',waived_count:'1'}),context,2,P,false));
    assert.throws(()=>W.preview(prepared(context,2,true,{changed_count:'3'}),context,2,P,true));
    assert.throws(()=>W.refs([{check:'0',error:'1'},{check:'0',error:'1'}],P));assert.throws(()=>W.refs([{check:'00',error:'0'}],P));
    assert.equal(W.refs(Array.from({length:5000},(_,i)=>({check:'0',error:String(i)})),P).length,5000);
    console.log('WEB DRC WAIVES UI: ALL OK (consent, frozen selection, reserved/legacy, paused reads, disk/reader outcomes, revision resume, cancellation, replay-only recovery, stale/expiry, u64, cleanup)');
}
module.exports={catalog,op,snapshot,prepared,context,harness};
if(require.main===module)test().catch(e=>{console.error(e);process.exitCode=1;});
