'use strict';
const assert=require('node:assert/strict'),P=require('./protocol.js');
const clone=v=>JSON.parse(JSON.stringify(v));
(async()=>{
    for(const kind of ['notes','waives']){
        const U=require('./drc-'+kind+'.js'),T=require('./drc-'+kind+'.test.cjs'),API='/api/v1/drc/review/'+kind;
        const h=T.harness();h.shared.model.binding_id='a'.repeat(64);h.init();await h.read();
        if(kind==='notes'){h.text('old approved note');}else{h.choose('waive');}
        await h.prepare();let lost=true;
        h.override=r=>{if(r.path===API&&r.method==='POST'&&lost){lost=false;h.execute(r);throw Error('lost ACK');}};
        await h.approve();assert.equal(h.shared.writes,1);assert(h.shared.raw);
        const original=clone(h.calls.find(r=>r.path===API&&r.method==='POST').body);
        const detached={...clone(h.shared.model),available:false,detached:true};if(kind==='notes'){detached.editable=false;}
        U.catalog(detached,P);assert.throws(()=>U.catalog({...detached,available:true},P));
        h.shared.model=detached;h.c.context.drc_id='7'.repeat(64);
        const reader={id:h.c.context.drc_id,revision:h.c.context.revision,phase:'ready'};
        h.panel.attach(clone(detached),reader);
        assert(h.el(kind+'-read').disabled);assert(h.el(kind+'-prepare').disabled);assert(!h.el(kind+'-autosave').checked);
        assert.match(h.el(kind+'-owner').textContent,/detached/);
        if(kind==='notes'){assert(!h.el('notes-authoring').hidden,'receipt parent must remain visible');}
        if(kind==='waives'){assert(!h.panel.suspended(),'old reader receipt suspended the new DRC');}
        const writes=h.shared.writes;await h.read();assert.equal(h.shared.writes,writes);
        h.panel.stop();const again=T.harness(h.shared);again.init();
        assert(!again.el(kind+'-uncertain').hidden,'detachment must preserve unknown receipt recovery');
        if(kind==='notes'){assert(!again.el('notes-authoring').hidden,'recovery hidden by ancestor after reload');}
        await again.el(kind+'-resolve').onclick();assert.equal(again.shared.raw,null);assert.equal(again.shared.writes,1);
        assert.deepEqual(again.calls.find(r=>r.path===API&&r.method==='POST').body,original);
        // A new binding keeps receipts but resets consent, even when a client
        // missed the intermediate detached catalogue. Late old catalogues do
        // not restore the prior binding or its auto-save grant.
        const connected={...clone(h.shared.model),binding_id:'b'.repeat(64),available:true,detached:false,review_rev:'2'};
        if(kind==='notes'){connected.editable=true;}
        again.c.context.drc_id=reader.id;
        again.shared.model=connected;again.panel.attach(clone(connected),reader);
        assert(!again.el(kind+'-autosave').checked);assert(!again.el(kind+'-read').disabled);
        assert.match(again.el(kind+'-status').textContent,/Earlier review registration/);
        again.panel.attach(clone(detached),reader);assert.match(again.el(kind+'-message').textContent,/Older .* state/);
        again.panel.stop(true);
        const grant=T.harness();grant.init();grant.el(kind+'-autosave').checked=true;grant.el(kind+'-autosave').onchange();
        assert(grant.el(kind+'-autosave').checked);const v={...T.catalog(),available:false,detached:true};if(kind==='notes'){v.editable=false;}
        grant.panel.attach(v,reader);assert(!grant.el(kind+'-autosave').checked);assert(grant.el(kind+'-autosave').disabled);grant.panel.stop(true);
        const missed=T.harness();missed.shared.model.binding_id='a'.repeat(64);missed.init();
        missed.el(kind+'-autosave').checked=true;missed.el(kind+'-autosave').onchange();assert(missed.el(kind+'-autosave').checked);
        const direct={...clone(missed.shared.model),binding_id:'b'.repeat(64),review_rev:'1'};
        missed.panel.attach(direct,reader);assert(!missed.el(kind+'-autosave').checked,'missed detach kept automatic-save consent');missed.panel.stop(true);
    }
    const notes=require('./drc-notes.test.cjs');
    const uncertain=notes.op('1','failed',{published:null,outcome_unknown:true,review_rev:'1',scope_id:'a'.repeat(64)});
    const isolated=notes.harness({model:{...notes.catalog([uncertain],'2'),binding_id:'b'.repeat(64)},raw:null,writes:0,records:new Map()});
    isolated.init();assert.equal(isolated.displayStates.at(-1).blocked,'','old-scope unknown receipt blocked new saved-note reads');
    assert.match(isolated.el('notes-status').textContent,/Earlier review registration.*\nSave outcome UNKNOWN/);isolated.panel.stop(true);
    console.log('WEB DRC DETACH: ALL OK (no new edits, no auto-save grant, old receipt recovery/replay, new-reader availability)');
})().catch(e=>{console.error(e);process.exitCode=1;});
