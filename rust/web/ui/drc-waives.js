/* ES2017 owner waives. Frozen selection + one-use preview + confirmed edit.
 * Publication and reader refresh are separate receipts, never optimistic edits. */
(function(root) {
    'use strict';
    const SaveMode=typeof module==='object'&&module.exports?require('./review-save-mode.js'):root.FloeReviewSaveMode;
    const API='/api/v1/drc/review/waives';
    const phases={queued:'Queued',publishing:'Saving',cancelling:'Cancelling',refreshing_reader:'Saved · refreshing review',succeeded:'Save completed',failed:'Save failed',cancelled:'Cancelled before saving'};
    const errors={review_changed:'The waive file changed or its lock is held. Read a new snapshot before preparing again.',
        review_expired:'The snapshot or preview expired. Nothing was saved automatically.',
        drc_context_changed:'The DRC selection or read revision changed. Select errors again before a new approval.',
        drc_changed_or_corrupt:'The DRC pack or sidecar changed externally. Reopen the review; no external file is adopted automatically.',
        review_pack_required:'Waive editing requires an ICE pack. Build the pack explicitly first.',
        review_disabled:'Waive editing was not enabled by the launcher.',
        drc_busy:'Another DRC operation is active. No write is retried automatically.',
        review_io_error:'The waive file could not be accessed. Check permissions and local diagnostics.',
        review_unavailable:'Check the save receipt before deciding whether to try again.',
        drc_apply_unknown:'Reader acknowledgement was lost or timed out. Reopen the review to verify the saved file.',
        invalid_drc_request:'Invalid waive selection or action. Registered inputs and private files are protected.',
        drc_read_limit:'Select at most 5000 errors.',
        operation_conflict:'This sequence belongs to another approved request.',
        operation_sequence:'Another tab used this sequence. Read a new snapshot before approving.',
        operation_expired:'The receipt expired. Check the waive file before clearing the local recovery record.'};
    function fail(){throw new Error('Invalid waive response');}
    function keys(v,required,optional){if(!v||typeof v!=='object'||Array.isArray(v)||required.some(function(k){return !Object.prototype.hasOwnProperty.call(v,k);})||
        Object.keys(v).some(function(k){return !required.includes(k)&&!(optional||[]).includes(k);})){fail();}}
    function id(v){if(typeof v!=='string'||!/^[a-f0-9]{64}$/.test(v)){fail();}return v;}
    function text(v,max){if(typeof v!=='string'||!v.length||new TextEncoder().encode(v).length>max){fail();}return v;}
    function context(v){keys(v,['drc_id','revision','view_id']);Object.keys(v).forEach(function(k){id(v[k]);});return v;}
    function equal(a,b){return !!a&&!!b&&['drc_id','revision','view_id'].every(function(k){return a[k]===b[k];});}
    function terminal(v){return !!v&&['succeeded','failed','cancelled'].includes(v.phase);}
    function operation(v,P){
        keys(v,['seq','kind','phase'],['context','elapsed_ms','error','published','outcome_unknown','directory_synced','review_rev','reader_applied','reader_error','reader_revision','scope_id']);
        if(v.scope_id!==undefined){id(v.scope_id);}
        P.counter(v.seq);if(v.kind!=='drc_waive'||!Object.prototype.hasOwnProperty.call(phases,v.phase)){fail();}
        if(v.phase==='queued'){if(Object.keys(v).length!==(v.scope_id===undefined?3:4)){fail();}return v;}
        context(v.context);if(v.elapsed_ms!==undefined){P.counter(v.elapsed_ms,true);}if(v.review_rev!==undefined){P.counter(v.review_rev,true);}
        if(typeof v.outcome_unknown!=='boolean'){fail();}if(v.error!==undefined&&v.error!==null){text(v.error,128);}
        if(v.reader_error!==undefined){text(v.reader_error,128);}if(v.reader_revision!==undefined){id(v.reader_revision);}
        if(v.outcome_unknown){if(v.phase!=='failed'||v.published!==null){fail();}}
        else if(typeof v.published!=='boolean'||v.published!==['succeeded','refreshing_reader'].includes(v.phase)){fail();}
        if(v.published===true){if(typeof v.directory_synced!=='boolean'||v.error){fail();}}
        else if(v.directory_synced!==undefined&&v.directory_synced!==null){fail();}
        if(v.reader_applied!==undefined&&v.reader_applied!==null&&typeof v.reader_applied!=='boolean'){fail();}
        if(v.reader_applied===true&&(v.phase!=='succeeded'||v.published!==true||!v.reader_revision||v.reader_error)){fail();}
        if(terminal(v)){P.counter(v.review_rev,true);if(v.reader_applied===undefined){fail();}}
        return v;
    }
    function catalog(v,P){
        keys(v,['available','kind','reviewer','review_rev','operations','note_bytes','selection_limit','preparing','autosave'],['detached','binding_id']);
        if(v.binding_id!==undefined){id(v.binding_id);}
        if(v.detached!==undefined&&(typeof v.detached!=='boolean'||v.detached&&v.available)){fail();}
        if(v.kind!=='drc_waive'||typeof v.available!=='boolean'||typeof v.preparing!=='boolean'||v.autosave!==false||v.note_bytes!==65536||v.selection_limit!==5000){fail();}
        text(v.reviewer,200);P.counter(v.review_rev,true);const a=v.operations;keys(a,['last_seq','active','history']);P.counter(a.last_seq,true);
        if(a.active!==null){P.counter(a.active);}if(!Array.isArray(a.history)||a.history.length>32){fail();}let previous='0';
        a.history.forEach(function(r){operation(r,P);if(P.compare(r.seq,previous)<=0||P.compare(r.seq,a.last_seq)>0||(!terminal(r)&&a.active!==r.seq)){fail();}previous=r.seq;});
        if(previous!==a.last_seq||(a.active!==null&&(a.active!==previous||terminal(a.history[a.history.length-1])))){fail();}return v;
    }
    function preview(v,c,count,P,prepared){
        keys(v,['kind','phase','name','selected_count','reserved_count','legacy_unverified','token','context','review_rev','reviewer','expires_in_ms'].concat(
            prepared?['waived','changed_count','replaces_existing','scope']:['waived_count','exists']));
        context(v.context);id(v.token);text(v.reviewer,200);text(v.name,4096);P.counter(v.review_rev,true);P.counter(v.selected_count);P.counter(v.expires_in_ms);
        if(!equal(v.context,c)||v.selected_count!==String(count)||typeof v.legacy_unverified!=='boolean'||v.kind!=='drc_waive'||
            v.phase!==(prepared?'prepared':'snapshot')||P.compare(v.expires_in_ms,prepared?'30000':'120000')>0){fail();}
        P.counter(v.reserved_count,true);if(P.compare(v.reserved_count,v.selected_count)>0){fail();}
        const n=prepared?v.changed_count:v.waived_count;P.counter(n,true);if(P.compare(n,v.selected_count)>0){fail();}
        if(prepared){if(v.scope!=='registered_reviewer_waives'||typeof v.replaces_existing!=='boolean'||typeof v.waived!=='boolean'){fail();}}
        else if(typeof v.exists!=='boolean'||Number(v.waived_count)+Number(v.reserved_count)>count){fail();}
        return v;
    }
    function approval(v,P){keys(v,['seq','context','token','approve','confirm_legacy']);P.counter(v.seq);context(v.context);id(v.token);
        if(v.approve!==true||typeof v.confirm_legacy!=='boolean'){fail();}return v;}
    function refs(v,P){if(!Array.isArray(v)||!v.length||v.length>5000){fail();}const seen=new Set();return v.map(function(r){keys(r,['check','error']);P.counter(r.check,true);P.counter(r.error,true);
        const k=r.check+':'+r.error;if(seen.has(k)){fail();}seen.add(k);return {check:r.check,error:r.error};});}
    function statusText(v){
        if(!v){return 'No waive save in this server session.';}
        let s=(v.outcome_unknown?'Save outcome UNKNOWN':phases[v.phase])+' · #'+v.seq;
        if(v.published===true){s+='\nWaive file saved.';if(!v.directory_synced){s+=' Directory sync failed: durability is unconfirmed. Do not repeat the save to retry sync.';}
            if(v.phase==='refreshing_reader'){s+='\nWaiting for the review reader.';}
            else if(v.reader_applied===true){s+='\nReader updated; waiting for matching review metadata before resuming.';}
            else{s+='\nReader '+(v.reader_applied===null?'outcome UNKNOWN':'was not updated')+'. Reopen the review before relying on its statuses. The saved file is not undone.';}}
        if(v.outcome_unknown){s+='\nThe worker may have committed. Check the file and reopen the review; no automatic retry.';}
        if(v.error){s+='\n'+(errors[v.error]||'Check local diagnostics and this receipt.');}
        if(v.reader_error){s+='\n'+(errors[v.reader_error]||'Reader refresh failed. Check local diagnostics.');}return s;
    }
    function bind(o){
        const P=o.protocol,el=o.el;
        let enabled=false,stopped=false,stale=true,model=null,reader=null,editor=null,draft=null,expiry=null,timer=null;
        let io=null,poll=null,write=null,approving=null,cancelling=null,revokeTask=null,revokeNext=null;
        let pending=null,uncertain=false,notice='',storageWarning='',notified=false,refreshKey='',transferLocked=false;
        const saveMode=SaveMode.bind({toggle:el('waives-autosave'),hint:el('waives-autosave-status'),kind:'waive',changed:changed});
        function abort(t){if(t){t.cancelled=true;if(t.abort){t.abort();}}}
        function active(){return model&&model.operations.active;}
        function latest(){const a=model&&model.operations.history;return a&&a[a.length-1];}
        function affects(c){return !reader||!c||reader.id===c.drc_id;}
        function suspended(){
            if(!enabled||stopped){return false;}
            if(stale){return true;}
            if((pending||write||uncertain)&&affects(pending&&pending.context)){return true;}
            const v=latest();if(!v||!affects(v.context)){return false;}
            if(active()||!terminal(v)){return true;}
            if(v.published===null||(v.published===true&&v.reader_applied!==true)){return true;}
            return !reader||reader.phase!=='ready'||(v.reader_revision!==undefined&&v.reader_revision!==reader.revision);
        }
        function notify(){const paused=suspended();if(paused!==notified){notified=paused;if(o.changed){o.changed();}}}
        function selection(){try{const c=o.selection();if(stopped||!c){return null;}context(c.context);id(c.epoch);text(c.key,256);text(c.caption,4096);
            if(!Number.isInteger(c.count)||c.count<1||c.count>5000){return null;}return c;}catch(_){return null;}}
        function same(c){const n=selection();return !!n&&equal(n.context,c.context)&&n.epoch===c.epoch&&n.key===c.key&&n.count===c.count;}
        function permitted(){return enabled&&!stopped&&!stale&&model&&model.available;}
        function busy(ignoreTransfer){return (!ignoreTransfer&&transferLocked)||!!pending||uncertain||!!active()||!!write||!!approving||suspended();}
        function transferReady(importing){return !!(permitted()&&!busy(true)&&!io&&!model.preparing&&!revokeTask&&(!importing||!editor));}
        async function publishTransfer(value,valid){
            if(!transferReady(true)||!valid()){return false;}const t={};approving=t;render();await refresh();
            if(approving!==t){return false;}approving=null;
            if(!transferReady(true)||!valid()||value.reviewer!==model.reviewer||value.review_rev!==model.review_rev){render();return false;}
            await send({seq:P.next(model.operations.last_seq),context:value.context,token:value.token,approve:true,confirm_legacy:true});return true;
        }
        function store(value){try{o.savePending(value?JSON.stringify({session_id:id(o.session()),request:value}):null);storageWarning='';}
            catch(_){storageWarning='Session storage unavailable. Keep this page open until the outcome is confirmed.';}}
        function recover(){try{const raw=o.loadPending();if(raw===null){return;}if(typeof raw!=='string'||raw.length>2048){fail();}const v=JSON.parse(raw);
            keys(v,['session_id','request']);id(v.session_id);if(v.session_id!==o.session()){fail();}pending=approval(v.request,P);uncertain=true;
            notice='An earlier approved waive save needs confirmation. Reload did not repeat it.';
        }catch(_){uncertain=true;notice='An old approval record cannot be recovered. Check the waive file before clearing this local record.';}}
        async function revoke(token){if(stopped||!enabled){return;}if(revokeTask){revokeNext=token;return;}const t={cancelled:false,abort:null};revokeTask=t;
            try{await o.http('POST',API+'/revoke',{token:token},false,t);}catch(_){/* Server expires unused previews too. */}
            finally{if(revokeTask===t){revokeTask=null;const next=revokeNext;revokeNext=null;if(next){revoke(next);}}}}
        function invalidate(reason){
            if(io){abort(io);io=null;}if(editor&&editor.token){revoke(editor.token);editor.token=null;}if(draft){revoke(draft.token);draft=null;}
            o.clearTimeout(expiry);expiry=null;el('waives-consent').checked=el('waives-legacy').checked=false;
            notice=reason+' No new save was approved.';if(editor){editor.invalid=true;if(editor.approvedSeq){notice+=' An earlier approved save is not undone; check its receipt.';}}
        }
        function changed(){const c=editor?editor.selection:io&&io.selection;
            if(c&&!same(c)){invalidate('Selection, connection or review context changed.');}
            else if(editor&&!editor.invalid&&o.now()>=editor.until){invalidate('Snapshot or preview expired.');}render();}
        function action(){return el('waives-action').value==='waive'?true:el('waives-action').value==='clear'?false:null;}
        function approvalReady(grant){return !!draft&&!!editor&&!editor.invalid&&permitted()&&!busy()&&!io&&!model.preparing&&
            (grant?saveMode.valid(grant)&&!draft.legacy_unverified:el('waives-consent').checked&&(!draft.legacy_unverified||el('waives-legacy').checked));}
        function render(){
            notify();el('waives-panel').hidden=!enabled;el('waives-owner').textContent=model?'Reviewer: '+model.reviewer+(model.detached?' · detached; previous receipts only':''):'';
            const c=selection(),ok=permitted()&&!busy()&&!io&&!model.preparing;
            const connection=o.connection?o.connection():null;
            saveMode.sync(enabled&&!stopped&&model&&!model.detached?model.reviewer:'',o.session()+'\n'+(connection||''),!!ok&&!!connection);
            el('waives-prepare').textContent=saveMode.on()?'Save status':'Preview save';
            el('waives-local').textContent=saveMode.on()?'Choosing an action saves it. The w key opens a new toggle-and-save action; an existing choice stays unchanged. Escape discards an unsent choice.':
                'No automatic save. Ctrl/Cmd+Enter previews; Escape discards the local choice.';
            el('waives-selection').textContent=c?c.caption:'Select errors in a ready ICE review to edit waives.';
            el('waives-read').disabled=!ok||!c||!!editor;el('waives-editor').hidden=!editor;
            el('waives-action').disabled=!!io||!!draft||busy()||stopped;
            el('waives-reload').disabled=!ok||!editor||!same(editor.selection);
            el('waives-prepare').disabled=!ok||!editor||editor.invalid||!!draft||!editor.token||action()===null;
            el('waives-discard').disabled=!editor&&!io;el('waives-review').hidden=!draft;
            el('waives-consent').disabled=el('waives-legacy').disabled=!draft||!ok;
            el('waives-approve').disabled=!approvalReady(null);
            el('waives-cancel').hidden=!active();el('waives-cancel').disabled=!permitted()||!!cancelling;
            el('waives-refresh').disabled=stopped||!enabled||!!poll;el('waives-uncertain').hidden=!uncertain;
            el('waives-resolve').disabled=!pending||stopped||!!write||!!cancelling;
            el('waives-forget').disabled=!(permitted()||model&&model.detached&&!stopped&&!stale)||!!active()||!!write||!el('waives-checked').checked;
            el('waives-status').textContent=(latest()&&latest().scope_id&&model.binding_id!==latest().scope_id?'Earlier review registration — receipt only\n':'')+statusText(latest());el('waives-message').textContent=[notice,storageWarning].filter(Boolean).join('\n');
            el('waives-paused').hidden=!suspended();
        }
        function schedule(){o.clearTimeout(timer);timer=null;if(enabled&&!stopped&&(active()||pending)){timer=o.setTimeout(refresh,active()?500:2500);}}
        function refreshReview(){const v=latest();if(!v||!terminal(v)||stopped){return;}const key=v.seq+':'+v.review_rev;
            if(key!==refreshKey&&o.refreshReview){refreshKey=key;Promise.resolve().then(function(){if(!stopped){return o.refreshReview();}}).catch(function(){/* Explicit refresh is still available. */});}}
        function settled(v){if(editor&&editor.accepted&&editor.approvedSeq===v.seq&&terminal(v)&&v.published===true&&equal(v.context,editor.selection.context)){clearEditor(false);}}
        function install(v){const old=latest(),next=v.operations.history[v.operations.history.length-1];
            const rebound=model&&v.binding_id&&model.binding_id&&v.binding_id!==model.binding_id&&P.compare(v.review_rev,model.review_rev)>0;
            if(model&&(!rebound&&(model.detached&&!v.detached||v.binding_id!==model.binding_id)||v.reviewer!==model.reviewer||P.compare(v.review_rev,model.review_rev)<0||P.compare(v.operations.last_seq,model.operations.last_seq)<0||
                old&&next&&old.seq===next.seq&&terminal(old)&&(['phase','elapsed_ms','error','published','outcome_unknown','directory_synced','review_rev','reader_applied','reader_error','reader_revision','scope_id'].some(function(k){return old[k]!==next[k];})||!equal(old.context,next.context)))){throw new Error('Older waive state was ignored.');}
            if(rebound){saveMode.reset();invalidate('Launcher reviewer reconnected; read a new selection.');}
            if(editor&&model&&editor.approvedSeq!==v.operations.last_seq&&(v.review_rev!==model.review_rev||v.operations.last_seq!==model.operations.last_seq)){invalidate('Another save changed the waive state.');}
            if(v.detached&&(!model||!model.detached)){saveMode.reset();invalidate('DRC replaced; this reviewer is detached.');}
            model=v;stale=false;v.operations.history.forEach(settled);
        }
        function receipt(v){if(!model||P.compare(v.seq,model.operations.last_seq)<0){return;}const a=model.operations,old=latest();if(old&&old.seq===v.seq&&terminal(old)){return;}
            a.history=a.history.filter(function(r){return r.seq!==v.seq;}).concat([v]).slice(-32);a.last_seq=v.seq;a.active=terminal(v)?null:v.seq;
            if(v.review_rev!==undefined){model.review_rev=v.review_rev;}settled(v);}
        async function refresh(){if(!enabled||stopped){return;}o.clearTimeout(timer);timer=null;abort(poll);const t={cancelled:false,abort:null};poll=t;render();
            try{const v=catalog(await o.http('GET',API,undefined,false,t),P);if(t.cancelled||poll!==t||stopped){return;}install(v);}
            catch(e){if(!t.cancelled&&poll===t){stale=true;notice='Waive status unavailable. '+(errors[e.code]||e.message);}}
            finally{if(poll===t){poll=null;changed();schedule();refreshReview();}}}
        async function read(keep,toggle,grant){changed();const c=selection();if(!permitted()||busy()||io||!c||model.preparing||(editor&&!keep)||(keep&&(!editor||!same(editor.selection)))){return;}
            let rows;try{rows=refs(c.references(),P);if(rows.length!==c.count){fail();}}catch(e){notice=e.message;render();return;}
            const saved=keep?el('waives-action').value:'';if(editor){invalidate('Reloading snapshot.');}
            const t={selection:c,cancelled:false,abort:null,sent:o.now()};let focus=false;io=t;notice='Reading selected statuses. No file is changed.';render();
            try{const v=preview(await o.http('POST',API+'/read',{context:c.context,errors:rows},false,t),c.context,c.count,P,false);
                if(t.cancelled||io!==t||!same(c)){revoke(v.token);return;}if(v.reviewer!==model.reviewer){fail();}
                editor={selection:c,token:v.token,until:Math.min(t.sent+120000,o.now()+Number(v.expires_in_ms)),invalid:false};draft=null;el('waives-action').value=toggle?(v.waived_count===v.selected_count?'clear':'waive'):saved;
                el('waives-target').textContent=c.caption+'\n'+v.name+'\n'+v.waived_count+' already waived · '+v.reserved_count+' reserved statuses';
                notice=toggle?'Toggle selected from current statuses. Preparing is separate from saving.':'Snapshot loaded. Choose Waive or Clear waive.';expiry=o.setTimeout(changed,Math.max(0,editor.until-o.now()));focus=true;
            }catch(e){if(!t.cancelled&&io===t){notice=errors[e.code]||e.message;}}
            // Hidden/disabled controls cannot receive browser focus. Render after releasing IO first.
            finally{if(io===t){io=null;changed();if(focus&&editor&&!editor.invalid&&!el('waives-action').disabled&&!(grant&&saveMode.valid(grant))){el('waives-action').focus();}}}
            if(focus&&grant&&editor&&!editor.invalid&&same(c)){
                if(saveMode.valid(grant)){await prepare(grant);}else{notice='Automatic save permission changed. The local choice was not saved.';render();}
            }
        }
        async function prepare(grant){changed();if(el('waives-prepare').disabled){return;}const e=editor,c=e.selection,token=e.token,waived=action();
            e.token=null;e.invalid=true;o.clearTimeout(expiry);expiry=null;const t={selection:c,cancelled:false,abort:null,sent:o.now()};let prepared=null,focus=false;io=t;notice='Preparing this selection. No file is changed yet.';render();
            try{const v=preview(await o.http('POST',API+'/prepare',{context:c.context,token:token,waived:waived},false,t),c.context,c.count,P,true);
                if(t.cancelled||io!==t||editor!==e||!same(c)){revoke(v.token);return;}if(v.reviewer!==model.reviewer||v.waived!==waived){fail();}
                draft=prepared=v;e.invalid=false;e.until=Math.min(t.sent+30000,o.now()+Number(v.expires_in_ms));
                el('waives-preview').textContent=(v.waived?'Waive':'Clear waive on')+' '+v.selected_count+' selected errors · '+v.changed_count+' statuses change';
                el('waives-preview-target').textContent=c.caption+'\n'+v.name+'\n'+(v.replaces_existing?'Replace existing waive sidecar':'Create waive sidecar')+' · unselected statuses preserved';
                el('waives-reserved').textContent=v.reserved_count==='0'?'':v.reserved_count+' selected reserved statuses will be replaced with '+(v.waived?'1 (waived).':'0 (not waived).');
                el('waives-legacy-row').hidden=!v.legacy_unverified;el('waives-consent').checked=el('waives-legacy').checked=false;
                notice='Approve only the action shown above. Preview expires after 30 seconds. The geometry pack is not changed.';
                expiry=o.setTimeout(changed,Math.max(0,e.until-o.now()));focus=true;
            }catch(error){if(!t.cancelled&&io===t){notice=(errors[error.code]||error.message)+' Read a new snapshot before preparing again.';}}
            finally{if(io===t){io=null;changed();if(focus&&draft&&!el('waives-consent').disabled&&!(grant&&saveMode.valid(grant)&&!draft.legacy_unverified)){el('waives-consent').focus();}}}
            if(grant&&prepared&&draft===prepared){
                if(saveMode.valid(grant)&&!prepared.legacy_unverified){await approve(grant);}else{
                    notice=prepared.legacy_unverified?'Legacy file confirmation is required; automatic save did not approve it.':
                        'Automatic save permission changed. Review and approve this preview explicitly; no save was submitted.';render();}
            }
        }
        function confirm(){changed();return prepare(saveMode.capture());}
        function clearEditor(discard){if(discard){invalidate('Draft discarded.');}else{o.clearTimeout(expiry);expiry=null;}
            editor=draft=null;el('waives-action').value='';el('waives-preview').textContent='';el('waives-consent').checked=el('waives-legacy').checked=false;}
        async function send(request){pending=request;uncertain=false;el('waives-checked').checked=false;store(request);abort(poll);poll=null;
            const t={cancelled:false,abort:null};write=t;notice='Submitting the approved save. Review reads are paused; a committed file is not undone by closing.';render();
            try{const v=operation(await o.http('POST',API,request,false,t),P);if(v.seq!==request.seq||(v.context&&!equal(v.context,request.context))){fail();}
                if(t.cancelled||write!==t||stopped){return;}if(editor&&editor.approvedSeq===request.seq&&equal(editor.selection.context,request.context)){editor.accepted=true;}
                receipt(v);settled(v);pending=null;uncertain=false;store(null);notice='Approval acknowledged. File and reader outcomes are shown separately.';
            }catch(e){if(t.cancelled||write!==t||stopped){return;}const rejected=e.status>=400&&e.status<500&&e.status!==408&&e.code!=='operation_expired';
                uncertain=!rejected;if(rejected){pending=null;store(null);notice=errors[e.code]||e.message;}
                else{notice=errors[e.code]||'Outcome unknown. Resolve resends only the SAME approved request, never a new edit.';}}
            finally{if(write===t){write=null;if(!stopped){await refresh();}else{render();}}}}
        async function approve(grant){changed();if(!approvalReady(grant)){return;}const d=draft,e=editor,t={};approving=t;render();await refresh();
            if(approving!==t){return;}approving=null;changed();if(draft!==d||editor!==e||!approvalReady(grant)){
                if(grant){notice='Automatic save stopped before submission. Check the selection and preview.';}render();return;}
            try{const request={seq:P.next(model.operations.last_seq),context:d.context,token:d.token,approve:true,confirm_legacy:grant?false:el('waives-legacy').checked};
                o.clearTimeout(expiry);expiry=null;e.token=null;e.invalid=true;e.approvedSeq=request.seq;e.accepted=false;draft=null;
                el('waives-consent').checked=el('waives-legacy').checked=false;await send(request);
            }catch(error){notice=error.message;render();}}
        async function cancel(){const seq=active();if(!seq||!permitted()||cancelling){return;}const t={cancelled:false,abort:null};cancelling=t;render();
            try{const v=operation(await o.http('POST',API+'/'+seq+'/cancel',{},false,t),P);if(v.seq!==seq){fail();}if(!t.cancelled){receipt(v);notice='Cancellation requested. Check both outcomes; cancellation is not undo.';}}
            catch(e){if(!t.cancelled){notice='Cancellation unconfirmed. Refresh the receipt. '+e.message;}}
            finally{if(cancelling===t){cancelling=null;if(!stopped){await refresh();}}}}
        function discard(){clearEditor(true);notice='Local choice discarded. Previously approved saves are not undone.';render();el('waives-read').focus();}
        el('waives-read').onclick=function(){return read(false);};el('waives-reload').onclick=function(){return read(true);};
        el('waives-action').onchange=function(){changed();const grant=saveMode.capture();if(grant){return prepare(grant);}};
        el('waives-prepare').onclick=confirm;el('waives-discard').onclick=discard;
        el('waives-consent').onchange=el('waives-legacy').onchange=changed;el('waives-approve').onclick=function(){return approve(null);};
        el('waives-editor').onkeydown=function(e){if(e.isComposing||e.keyCode===229){return;}if(e.key==='Escape'){e.preventDefault();e.stopPropagation();discard();}
            else if(e.key==='Enter'&&(e.ctrlKey||e.metaKey)){e.preventDefault();e.stopPropagation();if(!draft){confirm();}}};
        el('waives-refresh').onclick=function(){refreshKey='';return refresh();};el('waives-cancel').onclick=cancel;
        el('waives-resolve').onclick=function(){if(pending&&uncertain&&!stopped&&!write&&!cancelling){return send(pending);}};
        el('waives-checked').onchange=render;el('waives-forget').onclick=function(){if(!el('waives-forget').disabled){pending=null;uncertain=false;store(null);el('waives-checked').checked=false;notice='Local recovery record cleared after your check. No request was sent.';render();}};
        render();return {attach:function(value,currentReader){reader=currentReader||null;if(!value){if(enabled){stale=true;notice='Waive registration unavailable. Refresh before relying on review statuses.';}render();return;}
                try{if(!enabled){enabled=true;recover();}const v=catalog(value,P);install(v);changed();schedule();}
                catch(e){stale=true;notice=e.message;render();}},changed:changed,refresh:refresh,suspended:suspended,
            open:function(){if(!enabled||stopped){return false;}changed();if(editor){el('waives-action').focus();}else{read(false,true,saveMode.capture());}return true;},
            transferReady:transferReady,transferLock:function(value){transferLocked=value===true;render();},publishTransfer:publishTransfer,
            stop:function(final){stopped=true;stale=true;saveMode.reset();approving=null;clearEditor(false);if(write){uncertain=true;}
                [io,poll,write,cancelling,revokeTask].forEach(abort);io=poll=write=cancelling=revokeTask=null;revokeNext=null;o.clearTimeout(timer);timer=null;
                if(final){store(null);}notice=final?'Session ended. Earlier committed saves are not undone.':'Review disconnected. Check the receipt after reconnecting.';render();},resume:function(){stopped=false;return enabled?refresh():Promise.resolve();}};
    }
    const api={bind:bind,catalog:catalog,preview:preview,operation:operation,approval:approval,refs:refs,statusText:statusText};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeDRCWaives=api;}
}(typeof window==='object'?window:this));
