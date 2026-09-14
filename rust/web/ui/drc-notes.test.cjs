'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs');
const N=require('./drc-notes.js'),P=require('./protocol.js');
const API='/api/v1/drc/review/notes',clone=v=>JSON.parse(JSON.stringify(v));
const context={drc_id:'a'.repeat(64),revision:'b'.repeat(64),view_id:'c'.repeat(64)};
const report={skipped_lines:0,invalid_members:0,reassigned_members:0};
const flush=async()=>{for(let i=0;i<30;i++)await Promise.resolve();};
function op(seq='1',phase='succeeded',extra={}){
    const done=['succeeded','failed','cancelled'].includes(phase);
    return {seq,kind:'drc_note',phase,...(phase==='queued'?{}:{context:clone(context),elapsed_ms:'3',error:phase==='failed'?'review_changed':null,
        published:phase==='succeeded',outcome_unknown:false,directory_synced:phase==='succeeded'?true:null}),...(done?{review_rev:phase==='succeeded'?'1':'0'}:{}),...extra};
}
function catalog(history=[],rev){const last=history.at(-1);return {available:true,kind:'drc_note',reviewer:'fixed-owner',review_rev:rev||(last&&last.review_rev)||'0',
    operations:{last_seq:last?last.seq:'0',active:last&&!['succeeded','failed','cancelled'].includes(last.phase)?last.seq:null,history},note_bytes:65536,selection_limit:5000,preparing:false,autosave:false};}
function snapshot(c,count=2,extra={}){return {kind:'drc_note',phase:'snapshot',name:'.synthetic.db.notes.fixed-owner.fe',context:clone(c),token:'d'.repeat(64),
    review_rev:'0',reviewer:'fixed-owner',expires_in_ms:'120000',selected_count:String(count),existing_count:'0',mixed:false,text:null,exists:false,legacy_unverified:false,import_report:clone(report),...extra};}
function prepared(c,count=2,text='hello',extra={}){const v=snapshot(c,count);delete v.mixed;delete v.existing_count;delete v.exists;return {...v,phase:'prepared',token:'e'.repeat(64),
    expires_in_ms:'30000',text:text.trim(),clears:!text.trim(),replaces_existing:false,scope:'registered_reviewer_notes',...extra};}
function error(status,code){return Object.assign(new Error(code||'network lost'),{status,code});}
function harness(shared={model:catalog(),raw:null,writes:0,records:new Map()}){
    const ids=[...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]);
    const nodes=new Map(),calls=[],timers=new Map(),displayStates=[];let now=0,serial=0,override=null,storageError=false,session='f'.repeat(64);
    const c={context:clone(context),epoch:'9'.repeat(64),key:'groups:1',caption:'2 selected errors across rules',ready:true,
        rows:[{check:'0',error:'9007199254740993'},{check:'7',error:'0'}]};
    function el(id){assert(ids.includes(id),'missing HTML '+id);if(!nodes.has(id)){nodes.set(id,{value:'',textContent:'',disabled:false,hidden:false,checked:false,
        focus(){if(!this.disabled&&!this.hidden&&!el('notes-panel').hidden&&
            (!['notes-text','notes-consent'].includes(id)||!el('notes-editor').hidden)&&
            (id!=='notes-consent'||!el('notes-review').hidden)){this.focused=true;}},set innerHTML(v){throw Error('HTML injection '+v);}});}return nodes.get(id);}
    function execute(r){
        if(r.method==='GET')return clone(shared.model);
        if(r.path.endsWith('/read'))return snapshot(r.body.context,r.body.errors.length,{review_rev:shared.model.review_rev});
        if(r.path.endsWith('/prepare'))return prepared(r.body.context,c.rows.length,r.body.text,{review_rev:shared.model.review_rev});
        if(r.path.endsWith('/revoke'))return null;
        if(r.path.endsWith('/cancel'))return clone(shared.model.operations.history.at(-1));
        assert.equal(r.path,API);const prior=shared.records.get(r.body.seq);
        if(prior){assert.deepEqual(r.body,prior.request,'replayed a different approval');return clone(prior.result);}
        const result=op(r.body.seq,'succeeded',{context:clone(r.body.context),review_rev:P.next(shared.model.review_rev)});
        shared.records.set(r.body.seq,{request:clone(r.body),result});shared.writes++;
        shared.model=catalog(shared.model.operations.history.concat([result]).slice(-32));return clone(result);
    }
    const panel=N.bind({el,protocol:P,session:()=>session,now:()=>now,
        displayState:s=>displayStates.push(clone(s)),
        selection:()=>c.ready?{context:clone(c.context),epoch:c.epoch,key:c.key,caption:c.caption,count:c.rows.length,references:()=>clone(c.rows)}:null,
        setTimeout(fn,ms){timers.set(++serial,{fn,at:now+ms});return serial;},clearTimeout:id=>timers.delete(id),
        loadPending(){if(storageError)throw Error('no storage');return shared.raw;},savePending(v){if(storageError)throw Error('no storage');shared.raw=v;},
        async http(method,path,body,missing,token){const r={method,path,body,token};calls.push(r);if(override){const v=override(r);if(v!==undefined)return await v;}return execute(r);}});
    return {panel,el,c,calls,timers,displayStates,shared,execute,init:()=>panel.attach(clone(shared.model)),read:()=>el('notes-read').onclick(),prepare:()=>el('notes-prepare').onclick(),
        text(v){el('notes-text').value=v;el('notes-text').oninput();},approve(legacy=false){el('notes-consent').checked=true;el('notes-legacy').checked=legacy;el('notes-consent').onchange();return el('notes-approve').onclick();},
        tick(ms){now+=ms;for(const[id,t]of[...timers])if(t.at<=now){timers.delete(id);t.fn();}},
        set override(v){override=v;},set session(v){session=v;},set storageError(v){storageError=v;}};
}
const writes=h=>h.calls.filter(r=>r.method==='POST'&&r.path===API);
async function test(){
    const imported=harness();imported.init();imported.c.ready=false;imported.panel.transferLock(true);
    assert.match(imported.displayStates.at(-1).blocked,/transfer.*no save is implied/);
    assert(imported.panel.transferReady(true),'whole import must not require selected errors');
    const whole={context:clone(context),token:'e'.repeat(64),reviewer:'fixed-owner',review_rev:'0'};
    assert.equal(await imported.panel.publishTransfer(whole,()=>false),false);assert.equal(writes(imported).length,0);
    let dropImport=true;imported.override=r=>{if(r.path===API&&r.method==='POST'&&dropImport){dropImport=false;imported.execute(r);return Promise.reject(error(0));}};
    await imported.panel.publishTransfer(whole,()=>true);assert.equal(imported.shared.writes,1);assert(imported.shared.raw);assert(!imported.el('notes-uncertain').hidden);
    await imported.el('notes-resolve').onclick();assert.equal(imported.shared.writes,1);assert.equal(imported.shared.raw,null);assert.equal(writes(imported)[0].body.confirm_legacy,true);imported.panel.stop(true);
    const guarded=harness();guarded.init();let valid=true;guarded.override=r=>{if(r.method==='GET'){valid=false;return guarded.execute(r);}};
    assert.equal(await guarded.panel.publishTransfer(whole,()=>valid),false);assert.equal(writes(guarded).length,0);guarded.panel.stop(true);
    const h=harness();h.panel.attach(null);assert(h.el('notes-panel').hidden);assert.equal(h.calls.length,0);
    h.init();assert(!h.el('notes-read').disabled);await h.read();assert.deepEqual(h.calls.at(-1).body.errors,h.c.rows);
    assert(h.el('notes-text').focused,'snapshot must reveal and enable editor before focus');
    h.text('  한글 <script> & text\nsecond line  ');await h.prepare();assert.equal(writes(h).length,0);assert(h.el('notes-approve').disabled);
    assert.equal(h.el('notes-preview').textContent,'한글 <script> & text\nsecond line');assert(h.el('notes-consent').focused,'preview must enable consent before focus');await h.el('notes-approve').onclick();assert.equal(writes(h).length,0);
    await h.approve();assert.equal(h.shared.writes,1);assert.equal(h.shared.raw,null);assert.equal(h.el('notes-text').value,'');assert.match(h.el('notes-status').textContent,/Saved.*#1/);
    assert(h.displayStates.some(s=>s&&s.blocked.includes('pending')));assert.equal(h.displayStates.at(-1).review_rev,'1');assert.equal(h.displayStates.at(-1).blocked,'');assert.equal(h.displayStates.at(-1).read_turn,1);
    assert.equal(h.calls.filter(r=>r.path.endsWith('/revoke')).length,0,'approved token revoked');
    await h.read();h.text('local draft');const n=h.calls.length;h.c.pan='any zoom or pan';h.panel.changed();assert.equal(h.calls.length,n,'pan reread notes');
    h.c.key='groups:2';h.panel.changed();await flush();assert(h.el('notes-prepare').disabled);assert.equal(h.el('notes-text').value,'local draft');assert(h.calls.at(-1).path.endsWith('/revoke'));
    h.c.key='groups:1';h.panel.changed();await h.el('notes-reload').onclick();assert.equal(h.el('notes-text').value,'local draft');assert(!h.el('notes-prepare').disabled);
    h.tick(120001);await flush();assert(h.el('notes-prepare').disabled);assert.match(h.el('notes-message').textContent,/expired/);
    await h.el('notes-reload').onclick();await h.prepare();h.tick(30001);await flush();assert(h.el('notes-review').hidden);assert.equal(h.el('notes-text').value,'local draft');
    for(const change of [()=>h.c.context.revision='1'.repeat(64),()=>h.c.epoch='2'.repeat(64),()=>h.c.ready=false]){
        h.el('notes-discard').onclick();h.c.ready=true;await h.read();h.text('retained');await h.prepare();change();h.panel.changed();assert(h.el('notes-review').hidden);assert.equal(h.el('notes-text').value,'retained');
    }
    h.panel.stop();assert.equal(h.timers.size,0);

    const mixed=harness();mixed.override=r=>r.path.endsWith('/read')?snapshot(r.body.context,2,{mixed:true,existing_count:'1',exists:true,import_report:{...report,skipped_lines:2}}):
        r.path.endsWith('/prepare')?prepared(r.body.context,2,r.body.text,{legacy_unverified:true,replaces_existing:true}):undefined;
    mixed.init();await mixed.read();assert.match(mixed.el('notes-message').textContent,/differ/);assert.match(mixed.el('notes-message').textContent,/skipped_lines: 2/);
    mixed.text(' \n ');await mixed.prepare();assert.match(mixed.el('notes-preview').textContent,/Clear notes/);assert(!mixed.el('notes-legacy-row').hidden);
    await mixed.approve();assert.equal(writes(mixed).length,0);await mixed.approve(true);assert.equal(writes(mixed)[0].body.confirm_legacy,true);mixed.panel.stop();

    const failed=harness();failed.init();await failed.read();failed.text('must retain on failure');await failed.prepare();
    failed.override=r=>r.path===API&&r.method==='POST'?Promise.reject(error(409,'review_changed')):undefined;
    await failed.approve();assert.equal(failed.el('notes-text').value,'must retain on failure');assert.equal(failed.shared.raw,null);assert(failed.el('notes-prepare').disabled);
    await failed.el('notes-reload').onclick();assert(!failed.el('notes-prepare').disabled);assert.equal(failed.el('notes-text').value,'must retain on failure');failed.panel.stop();

    // The same sequence from another tab is not proof OUR text was saved.
    const conflict=harness();conflict.init();await conflict.read();conflict.text('not another tab');await conflict.prepare();
    conflict.override=r=>{if(r.path===API&&r.method==='POST'){conflict.shared.model=catalog([op()]);return Promise.reject(error(409,'operation_conflict'));}};
    await conflict.approve();assert.equal(conflict.el('notes-text').value,'not another tab');conflict.panel.stop();

    const lost=harness();lost.init();await lost.read();lost.text('PRIVATE NOTE NOT IN STORAGE');await lost.prepare();let drop=true;
    lost.override=r=>{if(r.path===API&&r.method==='POST'&&drop){drop=false;lost.execute(r);return Promise.reject(error(0));}};
    await lost.approve();assert.equal(lost.shared.writes,1);assert.equal(lost.el('notes-text').value,'PRIVATE NOTE NOT IN STORAGE');assert(!lost.shared.raw.includes('PRIVATE'));
    assert(!lost.el('notes-uncertain').hidden);const approved=clone(writes(lost)[0].body);await lost.panel.refresh();assert.equal(writes(lost).length,1);
    lost.panel.stop();assert.equal(lost.timers.size,0);const again=harness(lost.shared);again.init();await again.panel.refresh();assert.equal(writes(again).length,0,'reload replayed a write');
    again.c.ready=false;await again.el('notes-resolve').onclick();assert.deepEqual(writes(again)[0].body,approved);assert.equal(again.shared.writes,1);assert.equal(again.shared.raw,null);again.panel.stop();

    for(const final of ['cancelled','succeeded']){
        const c=harness();c.init();await c.read();c.text('cancel pending');await c.prepare();
        c.override=r=>{if(r.path===API&&r.method==='POST'){const v=op('1','publishing');c.shared.model=catalog([v]);return v;}
            if(r.path.endsWith('/cancel')){const v=op('1',final);c.shared.model=catalog([v]);return v;}};
        await c.approve();assert(!c.el('notes-cancel').hidden);await c.el('notes-cancel').onclick();assert.equal(c.el('notes-text').value,final==='succeeded'?'':'cancel pending');
        assert.match(c.el('notes-status').textContent,final==='succeeded'?/Saved/:/Cancelled before/);c.panel.stop();assert.equal(c.timers.size,0);
    }
    const unknown=harness();unknown.init();await unknown.read();unknown.text('unknown outcome');await unknown.prepare();
    unknown.override=r=>{if(r.path===API&&r.method==='POST'){const v=op('1','failed',{published:null,outcome_unknown:true,error:'review_unavailable',review_rev:'1'});unknown.shared.model=catalog([v]);return v;}};
    await unknown.approve();assert.match(unknown.el('notes-status').textContent,/UNKNOWN.*#1/);assert.equal(unknown.el('notes-text').value,'unknown outcome');await unknown.panel.refresh();assert.equal(writes(unknown).length,1);unknown.panel.stop();
    assert(unknown.displayStates.some(s=>s&&s.review_rev==='1'&&s.blocked.includes('unconfirmed')));

    for(const kind of ['read','prepare']){
        const late=harness();late.init();if(kind==='prepare'){await late.read();late.text('before context change');}
        let reply;late.override=r=>r.path.endsWith('/'+kind)?new Promise(resolve=>{reply=()=>resolve(kind==='read'?snapshot(r.body.context):prepared(r.body.context,2,r.body.text));}):undefined;
        const wait=kind==='read'?late.read():late.prepare();late.c.key='changed';late.panel.changed();reply();await wait;await flush();
        assert(late.el('notes-review').hidden);assert.equal(writes(late).length,0);assert(late.calls.some(r=>r.path.endsWith('/revoke')));late.panel.stop();
    }
    const large=harness();large.init();await large.read();large.text('한'.repeat(21846));assert(large.el('notes-prepare').disabled);
    const before=large.calls.length;await large.prepare();assert.equal(large.calls.length,before);
    large.text('\\'.repeat(65536));assert(!large.el('notes-prepare').disabled);await large.prepare();assert.equal(large.el('notes-preview').textContent.length,65536);
    const requestCount=large.calls.length;large.el('notes-editor').onkeydown({key:'Enter',ctrlKey:true,isComposing:true});assert.equal(large.calls.length,requestCount);
    large.el('notes-editor').onkeydown({key:'Escape',preventDefault(){},stopPropagation(){}});assert(large.el('notes-editor').hidden);large.panel.stop();

    const storage=harness();storage.storageError=true;storage.init();assert(!storage.el('notes-uncertain').hidden);
    storage.el('notes-checked').checked=true;storage.el('notes-checked').onchange();storage.el('notes-forget').onclick();assert.match(storage.el('notes-message').textContent,/storage unavailable/);storage.panel.stop();
    const durable=op('18446744073709551615','succeeded',{directory_synced:false});assert.match(N.statusText(durable),/durability is unconfirmed/);N.operation(durable,P);
    const good=prepared(context);for(const mutate of [v=>v.context.view_id='wrong',v=>v.reviewer=8,v=>v.scope='elsewhere',v=>v.selected_count='3',v=>v.expires_in_ms='30001',v=>v.text='x'.repeat(65537),v=>v.extra=true]){
        const v=clone(good);mutate(v);assert.throws(()=>N.preview(v,context,2,P,true));
    }
    for(const mutate of [v=>v.published=false,v=>v.outcome_unknown=true,v=>v.seq=1,v=>v.context.revision='한'.repeat(64),v=>v.directory_synced=null,v=>v.extra=true]){
        const v=op();mutate(v);assert.throws(()=>N.operation(v,P));
    }
    assert.throws(()=>N.refs([{check:'0',error:'1'},{check:'0',error:'1'}],P));assert.throws(()=>N.refs([{check:'00',error:'0'}],P));
    assert.equal(N.refs(Array.from({length:5000},(_,i)=>({check:'0',error:String(i)})),P).length,5000);
    console.log('WEB DRC NOTES UI: ALL OK (selection, explicit consent, text retention, expiry/context, legacy, failure/unknown, replay-only recovery, cancel/commit, u64/UTF8, lifecycle)');
}
module.exports={catalog,op,snapshot,prepared,context,report};
if(require.main===module)test().catch(e=>{console.error(e);process.exitCode=1;});
