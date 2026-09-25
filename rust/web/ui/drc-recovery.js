/* ES2017. Explicit owner repair, separate from save/autosave. Only a user
 * action may POST an approval or a read-only reconciliation. GET never repairs. */
(function(root){
    'use strict';
    const BASE='/api/v1/drc/review/';
    function fail(){throw new Error('Invalid review recovery response');}
    function keys(v,required,optional){if(!v||typeof v!=='object'||Array.isArray(v)||required.some(function(k){return !Object.prototype.hasOwnProperty.call(v,k);})||Object.keys(v).some(function(k){return !required.includes(k)&&!(optional||[]).includes(k);})){fail();}}
    function id(v){if(typeof v!=='string'||!/^[a-f0-9]{64}$/.test(v)){fail();}return v;}
    function text(v,n){if(typeof v!=='string'||new TextEncoder().encode(v).length>n){fail();}return v;}
    function kind(v){if(!['notes','waives'].includes(v)){fail();}return v;}
    function context(v){keys(v,['drc_id','revision','view_id']);Object.keys(v).forEach(function(k){id(v[k]);});return v;}
    function equal(a,b){return !!a&&!!b&&['drc_id','revision','view_id'].every(function(k){return a[k]===b[k];});}
    function wireKind(k){return k==='notes'?'drc_note':'drc_waive';}
    function operation(v,P){
        const short=['queued','reconciling'].includes(v&&v.phase);
        keys(v,short?['kind','seq','phase']:['kind','seq','phase','context','reader_revision','review_rev','recovered','directory_synced','error','reopen_required'],['scope_id']);
        if(v.kind!=='review_recovery'){fail();}P.counter(v.seq);if(v.scope_id!==undefined){id(v.scope_id);}if(short){return v;}
        if(!['uncertain','succeeded','failed'].includes(v.phase)){fail();}context(v.context);id(v.reader_revision);P.counter(v.review_rev,true);
        if(v.recovered!==(v.phase==='uncertain'?null:v.phase==='succeeded')||![null,true,false].includes(v.directory_synced)||typeof v.reopen_required!=='boolean'||v.recovered!==false&&!v.reopen_required){fail();}
        if(v.error!==null){text(v.error,128);}return v;
    }
    function catalog(v,k,P){
        keys(v,['kind','review_kind','available','reviewer','binding_id','operations']);
        if(v.kind!=='review_recovery'||v.review_kind!==wireKind(k)||typeof v.available!=='boolean'){fail();}text(v.reviewer,200);id(v.binding_id);
        const ops=v.operations;keys(ops,['last_seq','active','history']);P.counter(ops.last_seq,true);
        if(!Array.isArray(ops.history)||ops.history.length>32){fail();}let last='0';
        ops.history.forEach(function(r){operation(r,P);if(P.compare(r.seq,last)<=0||P.compare(r.seq,ops.last_seq)>0){fail();}last=r.seq;});
        if(ops.active!==null){P.counter(ops.active);if(ops.active!==ops.last_seq||!ops.history.some(function(r){return r.seq===ops.active&&['queued','reconciling','uncertain'].includes(r.phase);})){fail();}}
        if(ops.history.some(function(r){return ['queued','reconciling','uncertain'].includes(r.phase)&&r.seq!==ops.active;})){fail();}return v;
    }
    function preview(v,k,P){
        keys(v,['kind','phase','review_kind','name','bytes','scope','token','context','review_rev','reviewer','expires_in_ms']);
        if(v.kind!=='review_recovery'||v.phase!=='prepared'||v.review_kind!==wireKind(k)||v.scope!=='exact_staging_link_only'){fail();}
        text(v.name,4096);if(!v.name||/[\x00/\\]/.test(v.name)||['.','..'].includes(v.name)){fail();}P.counter(v.bytes,true);id(v.token);context(v.context);P.counter(v.review_rev,true);text(v.reviewer,200);
        P.counter(v.expires_in_ms);if(P.compare(v.expires_in_ms,'30000')>0){fail();}return v;
    }
    function approval(v,P){keys(v,['seq','context','token','approve_recovery']);P.counter(v.seq);context(v.context);id(v.token);if(v.approve_recovery!==true){fail();}return v;}
    function bind(o){
        const el=o.el,P=o.protocol;
        let stopped=false,grant=null,model=null,draft=null,until=0,io=null,timer=null,expiry=null,turn=0;
        let pending=null,unknown=false,loaded=false,k='notes',message='',storageWarning='',stamp='',loadedSession='',grantStamp='';
        function session(){return o.session();}
        function current(){try{const c=o.context();return !stopped&&c?context(c):null;}catch(_){return null;}}
        function permitted(){return !stopped&&grant&&grant.available&&grant[k+'_editable'];}
        function active(){return model&&model.operations.active;}
        function latest(){const a=model&&model.operations.history;return a&&a[a.length-1];}
        function abort(){if(io){io.cancelled=true;if(io.abort){io.abort();}io=null;}++turn;}
        function clearDraft(){draft=null;el('recovery-consent').checked=false;o.clearTimeout(expiry);expiry=null;}
        function persist(req){
            try{o.savePending(req?JSON.stringify({session_id:id(session()),kind:k,request:req}):null);storageWarning='';return true;}
            catch(_){storageWarning='Cannot store the recovery receipt in this tab. Do not close it until the result is checked.';return false;}
        }
        function restore(){
            if(loaded||!session()){return;}loaded=true;loadedSession=session();
            try{const raw=o.loadPending();if(raw===null||raw===undefined){return;}if(typeof raw!=='string'||raw.length>2048){fail();}
                const saved=JSON.parse(raw);keys(saved,['session_id','kind','request']);id(saved.session_id);kind(saved.kind);approval(saved.request,P);
                unknown=true;if(saved.session_id!==session()){message='A receipt belongs to an earlier session. Check the file explicitly before clearing this tab record.';return;}
                k=saved.kind;pending=saved.request;message='An approval may have been sent. Check its receipt; no retry runs automatically.';
            }catch(_){unknown=true;message='The saved recovery receipt cannot be read. No automatic repair will run.';}
        }
        function paint(){
            const busy=!!io||!!active()||unknown||!!pending,editable=!!permitted();
            el('recovery-panel').hidden=!(grant&&grant.available)&&!unknown&&!pending;
            if(unknown||pending||active()){el('recovery-panel').open=true;}
            el('recovery-waives-choice').disabled=!(grant&&grant.waives_editable);
            el('recovery-kind').value=k;el('recovery-kind').disabled=busy||!!draft;
            el('recovery-prepare').disabled=!editable||!current()||busy||!!draft;
            el('recovery-preview').hidden=!draft;
            el('recovery-target').textContent=draft?draft.name+' · '+draft.bytes+' bytes · reviewer '+draft.reviewer:'';
            el('recovery-approve').disabled=!editable||busy||!draft||!equal(draft.context,current())||o.now()>=until||!el('recovery-consent').checked;
            el('recovery-discard').disabled=!!io;
            el('recovery-refresh').disabled=!!io||!editable;
            el('recovery-resolve').hidden=!pending;el('recovery-resolve').disabled=!!io||!editable;
            const row=latest();el('recovery-check').hidden=!(row&&row.phase==='uncertain');el('recovery-check').disabled=!!io||!editable||!!pending;
            const c=current(),earlier=!!(row&&row.context&&c&&row.context.drc_id!==c.drc_id);
            el('recovery-forget').hidden=!unknown;el('recovery-checked-label').hidden=!unknown;
            el('recovery-forget').disabled=!!io||!!active()||!el('recovery-checked').checked;
            el('recovery-status').textContent=row?(earlier?'Earlier recovery #':'Recovery #')+row.seq+': '+row.phase+(row.recovered===true?
                ' — payload unchanged.'+(earlier?' The DRC registration has changed since this receipt.':' Reopen the DRC with Open DRC to replace the old reader.')+(row.directory_synced!==true?' Durability was not confirmed; do not repeat the repair.':''):
                row.phase==='uncertain'?' — result unknown. Only an explicit read-only check is available.':row.phase==='failed'?' — not recovered; check the file before a new preview.'+(row.reopen_required?(earlier?' The DRC registration has changed since this receipt.':' Reopen with Open DRC; the old reader is retired.'):''):''):'';
            el('recovery-message').textContent=[message,storageWarning].filter(Boolean).join(' ');
        }
        function schedule(){o.clearTimeout(timer);timer=null;const row=latest();if(!stopped&&active()&&row&&row.phase!=='uncertain'){timer=o.setTimeout(refresh,350);}}
        async function request(method,path,body,accept){
            if(io||stopped){return false;}const t={method:method},n=turn;io=t;paint();
            try{const v=await o.http(method,BASE+k+path,body,false,t);if(stopped||t.cancelled||turn!==n){return false;}accept(v);return true;}
            catch(e){if(!stopped&&!t.cancelled&&turn===n){message=e&&e.message||'Recovery request failed.';}return false;}
            finally{if(io===t){io=null;paint();schedule();}}
        }
        function install(v){model=catalog(v,k,P);if(draft&&(!model.available||model.reviewer!==draft.reviewer)){clearDraft();}}
        function refresh(){if(!permitted()||io){return Promise.resolve(false);}return request('GET','/recovery',undefined,install);}
        async function prepare(){
            if(!permitted()||!current()||io||active()||unknown||pending||draft){return;}
            const editor=o.editors&&o.editors[k];if(!editor||!editor.recoveryReady||!editor.recoveryReady()){message='Finish or discard the current review edit before previewing file recovery.';paint();return;}
            const c=current();await request('POST','/recovery/prepare',{context:c},function(v){
                v=preview(v,k,P);if(!equal(c,current())||!equal(c,v.context)){throw Error('The recovery context changed. Request a new preview.');}
                draft=v;until=o.now()+Number(v.expires_in_ms);el('recovery-consent').checked=false;
                message='Preview only: the payload and its target name remain unchanged. Approval removes exactly the verified extra staging link.';
                expiry=o.setTimeout(function(){clearDraft();message='Recovery preview expired; nothing was approved.';paint();},v.expires_in_ms);
            });
        }
        async function send(req){
            const ok=await request('POST','/recovery',req,function(v){operation(v,P);if(v.seq!==req.seq||v.context&&!equal(v.context,req.context)){fail();}
                pending=null;unknown=false;persist(null);message='Approval acknowledged. File recovery does not reload the DRC reader.';
            });
            if(!ok&&pending){unknown=true;}paint();if(ok){await refresh();}
        }
        async function approve(){
            if(el('recovery-approve').disabled||io){return;}const d=draft,n=turn;
            // Reserve the single request slot across status -> approval. A
            // repeated click cannot select another sequence or approval token.
            if(!await refresh()||turn!==n||draft!==d||!equal(d.context,current())||o.now()>=until||!el('recovery-consent').checked||active()||unknown||pending){paint();return;}
            let req;try{req=approval({seq:P.next(model.operations.last_seq),context:d.context,token:d.token,approve_recovery:true},P);}
            catch(e){message=e.message;paint();return;}
            if(!persist(req)){message='Approval was not sent because its retry receipt could not be retained.';paint();return;}
            pending=req;clearDraft();await send(req);
        }
        async function check(){
            const row=latest();if(!permitted()||pending||io||!row||row.phase!=='uncertain'){return;}
            if(await request('POST','/recovery/reconcile',{seq:row.seq},function(v){operation(v,P);if(v.seq!==row.seq){fail();}})){await refresh();}
        }
        function changed(){
            if(loaded&&loadedSession!==session()&&pending){pending=null;unknown=true;message='The approval belongs to another session; it cannot be replayed here.';}
            restore();const c=current(),s=session()+':'+JSON.stringify(c);
            if(stamp!==s){stamp=s;abort();clearDraft();model=null;}
            paint();
        }
        el('recovery-kind').onchange=function(){const next=kind(el('recovery-kind').value);if(el('recovery-kind').disabled||!grant||!grant[next+'_editable']){paint();return;}k=next;abort();model=null;message='';paint();refresh();};
        el('recovery-prepare').onclick=prepare;el('recovery-approve').onclick=approve;
        el('recovery-consent').onchange=paint;el('recovery-checked').onchange=paint;
        el('recovery-refresh').onclick=refresh;
        el('recovery-resolve').onclick=function(){if(permitted()&&pending&&!io){return send(pending);}};
        el('recovery-check').onclick=check;
        el('recovery-discard').onclick=async function(){if(io||!draft){return;}const d=draft;clearDraft();paint();await request('POST','/revoke',{token:d.token},function(){});};
        el('recovery-forget').onclick=function(){if(el('recovery-forget').disabled){return;}if(persist(null)){pending=null;unknown=false;el('recovery-checked').checked=false;message='Tab receipt cleared only. No file was changed and no operation was cancelled.';}paint();};
        paint();
        return {attach:function(v){
                // review_grant.available means RECONNECT is possible, not
                // that the currently attached reviewer can write.
                const n=v.notes,w=v.waives;
                grant=n&&n.kind==='drc_note'&&n.available===true&&n.editable===true&&n.detached===false?{
                    available:v.replacing!==true,reviewer:n.reviewer,notes_editable:true,
                    waives_editable:!!(w&&w.kind==='drc_waive'&&w.available===true&&w.detached===false&&w.reviewer===n.reviewer),
                    notes_binding:n.binding_id,waives_binding:w&&w.binding_id}:null;
                const g=JSON.stringify(grant);if(grantStamp!==g){grantStamp=g;abort();clearDraft();model=null;}
                changed();if(permitted()&&!model&&!io){refresh();}},changed:changed,
            busy:function(){return !!(draft||pending||unknown||active()||io&&io.method!=='GET');},
            stop:function(){stopped=true;abort();clearDraft();o.clearTimeout(timer);timer=null;paint();},
            resume:function(){stopped=false;changed();refresh();}};
    }
    const api={bind:bind,operation:operation,catalog:catalog,preview:preview,approval:approval};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeDRCRecovery=api;}
}(typeof window==='object'?window:this));
