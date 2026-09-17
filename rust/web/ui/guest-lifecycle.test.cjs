'use strict';
const assert=require('node:assert/strict');
const {environment,tick,auth,id}=require('./guest.test.cjs');
const key='floe-guest-session:http://127.0.0.1:1234:'+id;
let cases=0;
function pause(e,kind){if(kind==='hidden'){e.doc.hidden=true;e.events.visibilitychange();}else{e.events.pagehide();}}
function resume(e,kind){if(kind==='hidden'){e.doc.hidden=false;e.events.visibilitychange();}else{e.events.pageshow({persisted:true});}}
function isolated(e){
    assert.equal(e.requests.filter(r=>r.path.endsWith('/exchange')).length,1,'one-shot invite is never replayed');
    assert.equal(e.storage.get('floe-session:http://127.0.0.1:1234'),'OWNER');
    assert.equal(e.storage.get('floe-default-pending:OWNER'),'PRIVATE');
    assert(e.reads.every(k=>k===key),'no owner storage reads');
    assert(e.requests.every(r=>r.method==='GET'||r.path.endsWith('/exchange')),'no saved files or view changes');
}
(async()=>{
    for(const mode of ['follow','explore']){
        for(const kind of ['hidden','pagehide']){
            for(const deferred of [false,true]){
                for(const restoreFirst of [false,true]){
                    const e=environment(mode);if(deferred){e.defer('POST','/exchange');}
                    const started=e.c.start();pause(e,kind);
                    if(restoreFirst){resume(e,kind);}
                    if(deferred){assert(!e.delayed[0].aborted,'keep the bounded one-shot exchange in flight');e.delayed[0].reply();}
                    await started;await tick();
                    assert.deepEqual(JSON.parse(e.storage.get(key)||'null'),auth,'keep a successful exchange even when hidden');
                    if(!restoreFirst){
                        assert.equal(e.requests.length,1,'no follow-up HTTP while suspended');assert.equal(e.sockets.length,0);
                        assert(e.observers.every(v=>!v.active),'no resize observer while suspended');resume(e,kind);await tick();
                    }
                    assert.equal(e.sockets.length,1);assert.equal(e.requests.length,2);assert(e.observers[0].active);
                    isolated(e);e.c.stop();cases++;
                }
            }
        }
        // A visibility event (or a stale button handler) cannot restore a page
        // which has not received its persisted pageshow yet.
        const parked=environment(mode);await parked.c.start();parked.events.pagehide();
        const count=parked.requests.length;parked.events.visibilitychange();parked.el('guest-reconnect').onclick();await tick();
        assert.equal(parked.requests.length,count);assert(parked.sockets.every(w=>w.readyState===3));
        parked.events.pageshow({persisted:true});await tick();assert.equal(parked.sockets.length,2);
        parked.events.visibilitychange();parked.el('guest-reconnect').onclick();await tick();
        assert.equal(parked.sockets.length,2,'late visible/reconnect events must not duplicate an admitted socket');
        isolated(parked);parked.c.stop();cases++;

        const hidden=environment(mode,undefined,false,{hidden:true});await hidden.c.start();
        assert.equal(hidden.requests.length,1);assert.equal(hidden.sockets.length,0);assert(!hidden.observers[0].active);
        hidden.events.pagehide();hidden.events.pageshow({persisted:true});await tick();assert.equal(hidden.requests.length,1);
        hidden.doc.hidden=false;hidden.events.visibilitychange();await tick();assert.equal(hidden.sockets.length,1);
        assert(hidden.observers[0].active);isolated(hidden);hidden.c.stop();cases++;

        // An unknown exchange is terminal, not an excuse to replay an invite.
        for(const result of ['timeout','unauthorized','invalid']){
            const e=environment(mode);e.defer('POST','/exchange');const started=e.c.start();pause(e,'pagehide');
            if(result==='timeout'){e.delayed[0].fail();}else{e.delayed[0].reply(result==='unauthorized'?401:200,{});}
            await started;resume(e,'pagehide');e.events.visibilitychange();await tick();
            assert(!e.storage.has(key));assert.equal(e.sockets.length,0);assert(e.el('guest-leave').disabled);
            isolated(e);cases++;
        }
        // A response/error already queued at stop cannot resurrect auth or
        // overwrite the terminal UI. Test both real abort and raced delivery.
        for(const ignoreAbort of [false,true]){
            for(const result of ['success','error']){
                const e=environment(mode,undefined,false,{ignoreAbort});e.defer('POST','/exchange');const started=e.c.start();
                e.c.stop();assert(e.delayed[0].aborted);e.delayed[0].reply(result==='success'?200:503);
                await started;assert.equal(e.el('guest-status').textContent,'Guest stopped.');assert(!e.storage.has(key));
                assert.equal(e.sockets.length,0);isolated(e);cases++;
            }
        }
        // A retired GET (including 401) must not revoke the newly resumed socket.
        for(const code of [200,401,503]){
            const e=environment(mode,undefined,false,{ignoreAbort:true});e.defer('GET','/session');const started=e.c.start();await tick();
            assert.equal(e.delayed.length,1);e.events.pagehide();e.events.pageshow({persisted:true});await tick();
            assert.equal(e.sockets.length,1);e.delayed[0].reply(code);await started;await tick();
            assert.equal(e.sockets.length,1);assert.equal(e.sockets[0].readyState,1);assert(e.storage.has(key));
            isolated(e);e.c.stop();cases++;
        }
        const stale=environment(mode);await stale.c.start();const oldError=stale.sockets[0].onerror;
        stale.events.pagehide();stale.events.pageshow({persisted:true});await tick();const message=stale.el('guest-status').textContent;
        oldError();assert.equal(stale.el('guest-status').textContent,message);assert.equal(stale.sockets.at(-1).readyState,1);
        isolated(stale);stale.c.stop();cases++;
    }
    console.log('WEB GUEST LIFECYCLE: ALL OK ('+cases+' cases, one-shot exchange, suspended admission, stale/stop isolation)');
})().catch(e=>{console.error(e);process.exitCode=1;});
