'use strict';
// Real editor state machines and race controls, with a deterministic HTTP peer.
const assert=require('node:assert/strict'),N=require('./drc-notes.test.cjs'),W=require('./drc-waives.test.cjs');
const flush=async()=>{for(let i=0;i<100;i++)await Promise.resolve();};
const error=(status,code)=>Object.assign(new Error(code||'network lost'),{status,code});
function wrap(kind,shared){
    const F=kind==='notes'?N:W,h=F.harness(shared),API='/api/v1/drc/review/'+kind;
    return Object.assign(h,{F,API,kind,mode(v){h.el(kind+'-autosave').checked=v;h.el(kind+'-autosave').onchange();},
        edit(){if(kind==='notes')h.text('한글 confirmed note');else h.el('waives-action').value='waive';},
        drafts(){return h.el(kind==='notes'?'notes-text':'waives-action').value;},
        posts(){return h.calls.filter(r=>r.path===API&&r.method==='POST');},
        finish(){h.panel.stop();assert.equal(h.timers.size,0);assert(!h.el(kind+'-autosave').checked);}});
}
async function preparedEditor(kind){const h=wrap(kind);h.init();await h.read();h.edit();h.mode(true);return h;}
async function test(){
    for(const kind of ['notes','waives']){
        const h=wrap(kind);h.mode(true);assert(!h.el(kind+'-autosave').checked);assert.equal(h.calls.length,0);
        h.init();assert(!h.el(kind+'-autosave').checked);h.mode(true);assert.equal(h.calls.length,0,'opt-in caused traffic');
        await h.read();h.edit();assert.equal(h.posts().length,0,'read/typing caused save');
        await h.prepare();assert.equal(h.shared.writes,1,kind+' confirmed edit did not save');
        assert.equal(h.posts()[0].body.confirm_legacy,false);assert.equal(h.shared.raw,null);assert(h.el(kind+'-autosave').checked);
        assert.equal(h.drafts(),'');h.mode(false);assert.equal(h.shared.writes,1);h.finish();

        const old=wrap(kind);old.init();await old.read();old.edit();await old.prepare();old.mode(true);await flush();
        assert.equal(old.posts().length,0,'opt-in approved a pre-existing preview');await old.approve();assert.equal(old.shared.writes,1);old.finish();

        for(const phase of ['prepare','GET']){
            const late=await preparedEditor(kind);let reply;
            late.override=r=>((phase==='prepare'&&r.path.endsWith('/prepare'))||(phase==='GET'&&r.method==='GET'))?
                new Promise(resolve=>{reply=()=>resolve(late.execute(r));}):undefined;
            const pending=late.prepare();await flush();assert(reply);assert(!late.el(kind+'-autosave').disabled);
            late.mode(false);late.mode(true); // busy opt-in is refused; old permission cannot come back
            reply();await pending;assert.equal(late.posts().length,0,phase+' ignored opt-out');assert(late.drafts());
            assert.match(late.el(kind+'-message').textContent,/permission changed|stopped before/);late.finish();
        }
        for(const change of ['selection','revision','expiry','disconnect','connection','epoch']){
            const late=await preparedEditor(kind);let reply;
            late.override=r=>r.method==='GET'?new Promise(resolve=>{reply=()=>resolve(late.execute(r));}):undefined;
            const pending=late.prepare();await flush();assert(reply);
            if(change==='selection'){late.c.key='other';late.panel.changed();}
            if(change==='revision'){late.c.context.revision='8'.repeat(64);late.panel.changed();}
            if(change==='connection'){late.c.connected=false;late.panel.changed();}
            if(change==='epoch'){late.c.epoch='6'.repeat(64);late.panel.changed();}
            if(change==='expiry')late.tick(30001);
            if(change==='disconnect')late.panel.stop();
            reply();await pending;assert.equal(late.posts().length,0,kind+' '+change+' crossed approval fence');
            if(['connection','epoch'].includes(change))assert(!late.el(kind+'-autosave').checked);late.finish();
        }
        const legacy=await preparedEditor(kind);
        legacy.override=r=>r.path.endsWith('/prepare')?legacy.F.prepared(r.body.context,2,kind==='notes'?r.body.text:r.body.waived,{legacy_unverified:true}):undefined;
        await legacy.prepare();assert.equal(legacy.posts().length,0);assert.match(legacy.el(kind+'-message').textContent,/Legacy/);
        await legacy.approve(false);assert.equal(legacy.posts().length,0);await legacy.approve(true);assert.equal(legacy.shared.writes,1);legacy.finish();

        const failed=await preparedEditor(kind);failed.override=r=>r.path===failed.API&&r.method==='POST'?Promise.reject(error(409,'review_changed')):undefined;
        await failed.prepare();assert(failed.drafts());assert.equal(failed.shared.writes,0);const failures=failed.posts().length;
        await failed.panel.refresh();assert.equal(failed.posts().length,failures);assert.match(failed.el(kind+'-message').textContent,/changed|lock/);failed.finish();

        const committed=await preparedEditor(kind);let savedReply;
        committed.override=r=>{if(r.path===committed.API&&r.method==='POST'){const result=committed.execute(r);return new Promise(resolve=>{savedReply=()=>resolve(result);});}};
        const pending=committed.prepare();await flush();assert(savedReply);assert.equal(committed.shared.writes,1);
        committed.mode(false);savedReply();await pending;assert.equal(committed.shared.writes,1);assert.match(committed.el(kind+'-status').textContent,/Saved|Save completed/);committed.finish();

        const lost=await preparedEditor(kind);let dropped=false;
        lost.override=r=>{if(r.path===lost.API&&r.method==='POST'&&!dropped){dropped=true;lost.execute(r);return Promise.reject(error(0));}};
        await lost.prepare();assert.equal(lost.shared.writes,1);assert(lost.drafts());assert(lost.shared.raw);assert(!lost.shared.raw.includes('confirmed note'));
        await lost.panel.refresh();assert.equal(lost.posts().length,1);const submitted=lost.posts()[0].body;lost.finish();
        const recovered=wrap(kind,lost.shared);recovered.init();await recovered.panel.refresh();assert(!recovered.el(kind+'-autosave').checked);
        assert.equal(recovered.posts().length,0);await recovered.el(kind+'-resolve').onclick();assert.deepEqual(recovered.posts()[0].body,submitted);
        assert.equal(recovered.shared.writes,1);recovered.finish();

        const a=wrap(kind),b=wrap(kind,a.shared);a.init();b.init();a.mode(true);assert(!b.el(kind+'-autosave').checked);a.finish();b.finish();
    }
    const choose=wrap('waives');choose.init();choose.mode(true);await choose.read();await choose.choose('waive');assert.equal(choose.shared.writes,1);choose.finish();
    const open=wrap('waives');open.init();open.mode(true);await open.read();open.el('waives-action').value='clear';const openCalls=open.calls.length;
    open.panel.open();await flush();assert.equal(open.calls.length,openCalls);assert.equal(open.el('waives-action').value,'clear');open.finish();
    const clear=await preparedEditor('notes');clear.text(' \n ');await clear.prepare();assert.equal(clear.shared.writes,1);
    assert.equal(clear.calls.find(r=>r.path.endsWith('/prepare')).body.text,' \n ');clear.finish();
    const report=await preparedEditor('notes');report.override=r=>r.path.endsWith('/prepare')?
        N.prepared(r.body.context,2,r.body.text,{import_report:{skipped_lines:1,invalid_members:0,reassigned_members:0}}):undefined;
    await report.prepare();assert.equal(report.posts().length,0);assert.match(report.el('notes-message').textContent,/parse warnings/);
    await report.approve();assert.equal(report.shared.writes,1);report.finish();
    const shortcut=wrap('waives');shortcut.init();shortcut.mode(true);assert(shortcut.panel.open());assert(shortcut.panel.open());await flush();
    assert.equal(shortcut.shared.writes,1);assert.equal(shortcut.calls.filter(r=>r.path.endsWith('/read')).length,1);shortcut.finish();
    const readRace=wrap('waives');readRace.init();readRace.mode(true);let reply;
    readRace.override=r=>r.path.endsWith('/read')?new Promise(resolve=>{reply=()=>resolve(readRace.execute(r));}):undefined;
    readRace.panel.open();readRace.mode(false);reply();await flush();assert.equal(readRace.posts().length,0);assert.equal(readRace.calls.filter(r=>r.path.endsWith('/prepare')).length,0);readRace.finish();
    const ime=await preparedEditor('notes'),count=ime.calls.length;
    ime.el('notes-text').oncompositionstart();ime.el('notes-editor').onkeydown({key:'Enter',ctrlKey:true});
    ime.el('notes-editor').onkeydown({key:'Escape'});assert.equal(ime.calls.length,count);assert(ime.drafts());
    ime.el('notes-text').oncompositionend();ime.el('notes-editor').onkeydown({key:'Enter',ctrlKey:true,keyCode:229});assert.equal(ime.calls.length,count);
    ime.el('notes-editor').onkeydown({key:'Enter',ctrlKey:true,preventDefault(){},stopPropagation(){}});await flush();assert.equal(ime.shared.writes,1);ime.finish();
    console.log('REVIEW AUTOSAVE: ALL OK (notes/waives opt-in, confirm, w/action/IME, read/prepare/approve races, legacy/CAS, submitted opt-out, replay-only recovery, tab/lifecycle isolation)');
}
test().catch(e=>{console.error(e);process.exitCode=1;});
