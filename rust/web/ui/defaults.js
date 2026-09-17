/* ES2017. Shared design-default publication is separate from settings download.
 * Preparation and polling never approve a write. Uncertain POSTs retain the
 * original request; only an explicit Resolve click can replay that request. */
(function(root) {
    'use strict';
    const phases={queued:'Queued',publishing:'Publishing',succeeded:'Published',failed:'Not published',cancelled:'Cancelled before publication'};
    const errors={default_changed:'The file or its attributes changed, or another publisher holds its lock. Review again.',
        stale_state:'The view changed. Review the current settings again.',view_unavailable:'The prepared view closed or changed.',
        default_draft_expired:'The preparation expired or was replaced. Review again.',
        unsupported:'These settings or file attributes cannot be published safely. Custom bitmaps and some inherited widths need Native JSON; use Save settings.',
        invalid_request:'The settings or derived default file cannot be used. Registered inputs and private files are protected.',
        busy:'Another publication or preparation is active. Nothing is retried automatically.',
        io_error:'The default file or directory could not be accessed. Check local permissions and service diagnostics.',
        operation_conflict:'This sequence belongs to a different approved request. This request was not accepted.',
        operation_sequence:'Another publication used the sequence. Review again; no new request was submitted.',
        operation_expired:'The receipt left server history. Verify the shared file before clearing this local record.',
        closed:'The server session ended. Verify the shared file if the last outcome was unknown.'};
    function fail(){throw new Error('Invalid design-default response');}
    function keys(v,required,optional){
        if(!v||typeof v!=='object'||Array.isArray(v)||required.some(function(k){return !Object.prototype.hasOwnProperty.call(v,k);})||
            Object.keys(v).some(function(k){return !required.includes(k)&&!(optional||[]).includes(k);})){fail();}
    }
    function id(v){if(typeof v!=='string'||!/^[a-f0-9]{64}$/.test(v)){fail();}return v;}
    function text(v,n){if(typeof v!=='string'||!v.length||v.length>n){fail();}return v;}
    function terminal(v){return !!v&&['succeeded','failed','cancelled'].includes(v.phase);}
    function operation(v,P){
        keys(v,['seq','kind','phase'],['view_id','state_rev','name','published','directory_synced','error']);P.counter(v.seq);
        if(v.kind!=='design_default'||!Object.prototype.hasOwnProperty.call(phases,v.phase)){fail();}
        if(v.view_id!==undefined){id(v.view_id);}if(v.state_rev!==undefined){P.counter(v.state_rev);}if(v.name!==undefined){text(v.name,4096);}
        if(v.phase!=='queued'){id(v.view_id);P.counter(v.state_rev);text(v.name,4096);}
        if(terminal(v)){
            if(v.published!==(v.phase==='succeeded')){fail();}
            if(v.published){if(typeof v.directory_synced!=='boolean'||v.error!==undefined){fail();}}
            else{text(v.error,128);if(v.directory_synced!==undefined){fail();}}
        }else if(v.published!==undefined||v.error!==undefined||v.directory_synced!==undefined){fail();}
        return v;
    }
    function catalog(v,P){
        keys(v,['available','operations','kind','scope','max_bytes','jobs']);
        if(typeof v.available!=='boolean'||v.kind!=='design_default'||v.scope!=='shared_design_default'||v.max_bytes!=='4194304'||v.jobs!==1){fail();}
        const a=v.operations;keys(a,['last_seq','active','history']);P.counter(a.last_seq,true);if(a.active!==null){P.counter(a.active);}
        if(!Array.isArray(a.history)||a.history.length>32){fail();}let prev='0';
        a.history.forEach(function(op){operation(op,P);if(P.compare(op.seq,prev)<=0||P.compare(op.seq,a.last_seq)>0||(!terminal(op)&&op.seq!==a.active)){fail();}prev=op.seq;});
        if(prev!==a.last_seq||(a.active!==null&&(a.active!==prev||terminal(a.history[a.history.length-1])))){fail();}return v;
    }
    function prepared(v,c,P,Q){
        keys(v,['token','view_id','state_rev','name','bytes','replaces_existing','expires_in_ms','scope','affects','mode','levels','title','rows']);
        id(v.token);id(v.view_id);P.counter(v.state_rev);P.counter(v.bytes);P.counter(v.expires_in_ms);
        if(v.view_id!==c.id||v.state_rev!==c.rev||P.compare(v.bytes,'4194304')>0||P.compare(v.expires_in_ms,'30000')>0||
            typeof v.replaces_existing!=='boolean'||v.scope!=='shared_design_default'||v.affects!=='future_opens'||
            !['level','chip','layer'].includes(v.mode)||!Number.isInteger(v.rows)||v.rows<1||v.rows>65536){fail();}
        text(v.name,4096);text(v.title,4096);
        if(v.levels!==null){if(!Array.isArray(v.levels)||!v.levels.length||v.levels.length>4096){fail();}
            const unique=new Set();v.levels.forEach(function(n){Q.i64(n,P);if(unique.has(n)){fail();}unique.add(n);});}
        return v;
    }
    function approval(v,P){keys(v,['seq','view_id','state_rev','token','approve']);P.counter(v.seq);P.counter(v.state_rev);id(v.view_id);id(v.token);if(v.approve!==true){fail();}return v;}
    function label(v){
        if(!v){return 'No design default published in this server session.';}
        let s=phases[v.phase]+' · #'+v.seq+(v.name?' · '+v.name:'');
        if(v.phase==='succeeded'){s+='\nShared default updated for future opens. Existing windows are unchanged.';
            if(!v.directory_synced){s+='\nPublished, but directory sync failed. Durability is unconfirmed; do not publish again just to retry sync.';}}
        if(v.error){s+='\n'+(errors[v.error]||'The server rejected this publication. Check local diagnostics.');}return s;
    }
    function bind(o){
        const P=o.protocol,Q=o.query,el=o.el;
        let enabled=false,stopped=false,model=null,stale=true,draft=null,expiry=null,timer=null;
        let readTask=null,prepareTask=null,writeTask=null,cancelTask=null,revokeTask=null,revokeNext=null,approving=null;
        let pending=null,uncertain=false,note='',pollError='',storageWarning='';
        function abort(t){if(t){t.cancelled=true;if(t.abort){t.abort();}}}
        function context(){try{const c=o.context();if(!c||!c.ready||!c.idle||stopped){return null;}id(c.id);id(c.epoch);P.counter(c.rev);return {id:c.id,epoch:c.epoch,rev:c.rev};}catch(_){return null;}}
        function same(c){const n=context();return !!n&&c.id===n.id&&c.epoch===n.epoch&&c.rev===n.rev;}
        function active(){return model&&model.operations.active;}
        function last(){const a=model&&model.operations.history;return a&&a[a.length-1];}
        function permitted(){return enabled&&!stopped&&!stale&&model&&model.available;}
        function busy(){return !!pending||uncertain||!!active()||!!writeTask||!!approving;}
        function store(request){try{o.savePending(request?JSON.stringify({session_id:id(o.session()),request:request}):null);storageWarning='';}
            catch(_){storageWarning='Browser session storage is unavailable. Keep this page open until the outcome is confirmed; a reload cannot recover an uncertain approval.';}}
        function recover(){el('default-checked').checked=false;try{const raw=o.loadPending();if(raw===null){return;}if(typeof raw!=='string'||raw.length>2048){fail();}
                const saved=JSON.parse(raw);keys(saved,['session_id','request']);id(saved.session_id);
                if(saved.session_id!==o.session()){throw new Error('Previous server session');}
                pending=Object.freeze(approval(saved.request,P));uncertain=true;note='An earlier approval needs confirmation. Resolve sends only that same request; nothing was replayed on reload.';
            }catch(_){uncertain=true;note='The saved approval record is unavailable or belongs to another session. Verify the shared file before clearing the local record.';}}
        async function revoke(token){
            if(stopped||!enabled){return;}if(revokeTask){revokeNext=token;return;}
            const t={cancelled:false,abort:null};revokeTask=t;
            try{await o.http('POST','/api/v1/defaults/revoke',{token:token},false,t);}catch(_){/* Read-only draft also expires server-side. */}
            finally{if(revokeTask===t){revokeTask=null;const next=revokeNext;revokeNext=null;if(next&&!stopped){revoke(next);}}}
        }
        function discard(notify){const d=draft;draft=null;o.clearTimeout(expiry);expiry=null;el('default-consent').checked=false;
            if(d&&notify){revoke(d.value.token);}}
        function changed(){
            if(prepareTask&&!same(prepareTask.context)){abort(prepareTask);prepareTask=null;note='View or connection changed. No publication was approved.';}
            if(draft&&(!same(draft.context)||o.now()>=draft.until)){discard(true);note='Preparation expired or the view changed. Review again.';}
            render();
        }
        function render(){
            el('default-panel').hidden=!enabled;
            el('default-prepare').disabled=!permitted()||!context()||busy()||!!prepareTask;
            el('default-review').hidden=!draft;
            el('default-consent').disabled=!draft||busy()||!permitted();
            el('default-approve').disabled=!draft||!el('default-consent').checked||busy()||!permitted();
            el('default-dismiss').disabled=!draft&&!prepareTask;
            el('default-cancel').hidden=!active();el('default-cancel').disabled=!permitted()||!!cancelTask;
            el('default-refresh').disabled=!enabled||stopped||!!readTask;
            el('default-uncertain').hidden=!uncertain;
            el('default-resolve').disabled=stopped||!pending||!!writeTask||!!cancelTask;
            el('default-forget').disabled=!permitted()||!!writeTask||!!active()||!el('default-checked').checked;
            el('default-status').textContent=label(last());
            el('default-note').textContent=[pollError,note,storageWarning].filter(Boolean).join('\n');
        }
        function schedule(){o.clearTimeout(timer);timer=null;if(enabled&&!stopped){timer=o.setTimeout(refresh,active()?500:2500);}}
        function install(v){
            const old=last(),next=v.operations.history[v.operations.history.length-1];
            if(model&&(P.compare(v.operations.last_seq,model.operations.last_seq)<0||old&&next&&old.seq===next.seq&&terminal(old)&&
                ['phase','view_id','state_rev','name','published','directory_synced','error'].some(function(k){return old[k]!==next[k];}))){throw new Error('Older publication state was ignored.');}
            if(draft&&(v.operations.active||model&&v.operations.last_seq!==model.operations.last_seq)){discard(true);note='Another publication changed the session. Review again.';}
            model=v;stale=false;pollError='';
        }
        function receipt(op){
            if(!model){return;}const a=model.operations;
            if(P.compare(op.seq,a.last_seq)<0){return;}
            const old=last();if(old&&old.seq===op.seq&&terminal(old)){return;}
            a.history=a.history.filter(function(r){return r.seq!==op.seq;}).concat([op]).slice(-32);
            a.last_seq=op.seq;a.active=terminal(op)?null:op.seq;
        }
        async function refresh(){
            if(!enabled||stopped){return;}o.clearTimeout(timer);timer=null;abort(readTask);
            const t={cancelled:false,abort:null};readTask=t;render();
            try{const v=catalog(await o.http('GET','/api/v1/defaults',undefined,false,t),P);
                if(t.cancelled||stopped||readTask!==t){return;}install(v);
            }catch(e){if(!t.cancelled&&readTask===t){stale=true;pollError='Publication state could not be checked. '+e.message;if(e.status===401){stopped=true;}}}
            finally{if(readTask===t){readTask=null;changed();schedule();}}
        }
        async function prepare(){
            changed();const c=context();if(!permitted()||!c||busy()||prepareTask){return;}
            discard(true);const t={context:c,sent:o.now(),cancelled:false,abort:null};prepareTask=t;
            note='Reading current settings and checking the target. No file has been approved.';render();
            try{const d=prepared(await o.http('POST','/api/v1/defaults/prepare',{view_id:c.id,state_rev:c.rev},false,t),c,P,Q);
                if(t.cancelled||prepareTask!==t||!same(c)){return;}
                draft={value:d,context:c,until:Math.min(t.sent+30000,o.now()+Number(d.expires_in_ms))};
                el('default-target').textContent=d.title+' · '+d.mode+' mode\n'+d.name+' · '+d.rows+' rows · '+d.bytes+' bytes\n'+(d.replaces_existing?'REPLACE existing shared file':'Create shared file');
                el('default-levels').textContent=d.levels===null?'All opened levels / layout layers.':('Selected deck levels only: '+d.levels.join(', ')+'. Other rows are NOT merged or preserved.');
                note='Review the shared impact and explicitly approve. This preparation expires after 30 seconds.';
                expiry=o.setTimeout(changed,Math.max(0,draft.until-o.now()));
            }catch(e){if(!t.cancelled&&prepareTask===t){note=errors[e.code]||e.message;}}
            finally{if(prepareTask===t){prepareTask=null;changed();if(draft){el('default-consent').focus();}}}
        }
        async function send(request){
            pending=request;uncertain=false;el('default-checked').checked=false;store(request);abort(readTask);readTask=null;
            const t={cancelled:false,abort:null};writeTask=t;note='Submitting the approved settings. Closing the view does not undo publication.';render();
            try{const op=operation(await o.http('POST','/api/v1/defaults',request,false,t),P);
                if(op.seq!==request.seq||op.view_id!==undefined&&(op.view_id!==request.view_id||op.state_rev!==request.state_rev)){fail();}
                if(t.cancelled||stopped||writeTask!==t){return;}receipt(op);pending=null;uncertain=false;store(null);note='Approval acknowledged. The receipt below is authoritative.';
            }catch(e){if(t.cancelled||stopped||writeTask!==t){return;}
                const rejected=e.status>=400&&e.status<500&&e.status!==408&&e.code!=='operation_expired';
                uncertain=!rejected;el('default-checked').checked=false;if(rejected){pending=null;store(null);note=errors[e.code]||e.message;}
                else{note=errors[e.code]||'Outcome unknown. Resolve replays only the SAME approved request, never a new sequence.';}
                if(e.status===401){stopped=true;}
            }finally{if(writeTask===t){writeTask=null;if(!stopped){await refresh();}else{render();}}}
        }
        async function approve(){
            changed();const d=draft;if(!permitted()||!d||busy()||!el('default-consent').checked){return;}
            const t={};approving=t;render();await refresh();if(approving!==t){return;}approving=null;changed();
            if(!permitted()||draft!==d||!same(d.context)||busy()||!el('default-consent').checked){render();return;}
            try{const request=Object.freeze({seq:P.next(model.operations.last_seq),view_id:d.value.view_id,state_rev:d.value.state_rev,token:d.value.token,approve:true});
                discard(false);await send(request);
            }catch(e){note=e.message;render();}
        }
        async function cancel(){
            const seq=active();if(!seq||!permitted()||cancelTask){return;}const t={cancelled:false,abort:null};cancelTask=t;render();
            try{const op=operation(await o.http('POST','/api/v1/defaults/'+seq+'/cancel',{},false,t),P);if(op.seq!==seq){fail();}
                if(!t.cancelled){receipt(op);note='Cancellation requested. A committed publication stays successful.';}}
            catch(e){if(!t.cancelled){note='Cancellation was not confirmed. Refresh the receipt. '+e.message;}}
            finally{if(cancelTask===t){cancelTask=null;if(!stopped){await refresh();}}}
        }
        function dismiss(){abort(prepareTask);prepareTask=null;discard(true);note='Preparation dismissed. No new publication was approved.';render();el('default-prepare').focus();}
        el('default-prepare').onclick=prepare;el('default-dismiss').onclick=dismiss;
        el('default-consent').onchange=changed;el('default-approve').onclick=approve;
        el('default-review').onkeydown=function(e){if(e.key==='Escape'){e.preventDefault();e.stopPropagation();dismiss();}};
        el('default-refresh').onclick=refresh;el('default-cancel').onclick=cancel;
        el('default-resolve').onclick=function(){if(uncertain&&pending&&!stopped&&!writeTask&&!cancelTask){return send(pending);}};
        el('default-checked').onchange=render;
        el('default-forget').onclick=function(){if(permitted()&&uncertain&&!writeTask&&!active()&&el('default-checked').checked){pending=null;uncertain=false;store(null);el('default-checked').checked=false;note='Local record cleared after your check. No request was sent. A new publication needs a new review and approval.';render();}};
        render();
        return {init:function(value){stopped=false;enabled=value===true;if(enabled){recover();}render();return enabled?refresh():Promise.resolve();},changed:changed,refresh:refresh,
            stop:function(final){stopped=true;stale=true;discard(false);approving=null;if(writeTask){uncertain=true;}
                [readTask,prepareTask,writeTask,cancelTask,revokeTask].forEach(abort);readTask=prepareTask=writeTask=cancelTask=revokeTask=null;revokeNext=null;
                o.clearTimeout(timer);timer=null;if(final){store(null);}render();},resume:function(){stopped=false;stale=true;return refresh();}};
    }
    const api={bind:bind,catalog:catalog,operation:operation,prepared:prepared,approval:approval};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeDefaults=api;}
}(typeof window==='object'?window:this));
