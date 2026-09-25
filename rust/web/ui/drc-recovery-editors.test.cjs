'use strict';
const assert=require('node:assert/strict');
const tick=async()=>{for(let i=0;i<60;i++)await Promise.resolve();};
(async()=>{
    for(const kind of ['notes','waives']){
        const T=require('./drc-'+kind+'.test.cjs'),h=T.harness();
        assert(!h.panel.recoveryReady());h.init();assert(h.panel.recoveryReady());
        h.panel.transferLock(true);assert(!h.panel.recoveryReady(),'an upload owns the editor');
        h.panel.transferLock(false);assert(h.panel.recoveryReady());
        h.c.ready=false;assert(h.panel.recoveryReady(),'recovery does not require a selection');h.c.ready=true;
        await h.read();assert(!h.panel.recoveryReady(),'do not discard an open editor');
        h.el(kind+'-discard').onclick();await tick();assert(h.panel.recoveryReady());
        h.panel.stop();assert(!h.panel.recoveryReady());assert.equal(h.shared.writes,0);
        const unknown=T.harness({...h.shared,raw:'not a recoverable receipt'});unknown.init();
        assert(!unknown.panel.recoveryReady(),'local unresolved approvals must be checked first');unknown.panel.stop();
    }
    const T=require('./drc-waives.test.cjs');
    for(const applied of [false,null]){
        const h=T.harness({model:T.catalog([T.op('1','succeeded',{reader_applied:applied,reader_error:'drc_reopen_required'})]),raw:null,writes:0,records:new Map()});
        h.reader.phase='error';h.c.ready=false;h.init();
        assert(h.panel.suspended());assert(!h.panel.transferReady(true));
        assert(h.panel.recoveryReady(),'failed reader must not block explicit file recovery');
        assert.equal(h.calls.length,0);assert.equal(h.shared.writes,0);h.panel.stop();
    }
    const active=T.harness({model:T.catalog([T.op('1','queued')]),raw:null,writes:0,records:new Map()});active.init();
    assert(!active.panel.recoveryReady());active.panel.stop();
    console.log('WEB RECOVERY EDITORS: ALL OK (failed reader allowed; edits, transfers, active and unknown approvals fenced)');
})().catch(e=>{console.error(e);process.exitCode=1;});
