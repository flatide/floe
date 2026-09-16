/* ES2017 owner notes. Reads/typing never publish. Selection is frozen before
 * reading; confirmed edits use per-edit consent or a tab-local opt-in. */
(function(root) {
    'use strict';
    const H=typeof module==='object'&&module.exports?require('./hangul.js'):root.FloeHangul;
    const SaveMode=typeof module==='object'&&module.exports?require('./review-save-mode.js'):root.FloeReviewSaveMode;
    const API='/api/v1/drc/review/notes', LIMIT=65536;
    const phases={queued:'Queued',publishing:'Saving',cancelling:'Cancelling',succeeded:'Saved',failed:'Save failed',cancelled:'Cancelled before saving'};
    const errors={review_changed:'The note file changed or another writer holds its lock. Reload the snapshot before preparing again.',
        review_expired:'The snapshot or preview expired. Reload the snapshot; no automatic save was attempted.',
        drc_context_changed:'The DRC database or view changed. Read the new selection before saving.',
        drc_changed_or_corrupt:'The DRC pack or its review binding changed. Reopen or explicitly import the review; no automatic adoption.',
        review_pack_required:'Notes require an ICE pack. Build a pack explicitly first.',
        review_disabled:'Note editing was not enabled by the launcher.',
        drc_busy:'Another DRC operation is active. Nothing is retried automatically.',
        review_io_error:'The note file could not be accessed. Check permissions and local diagnostics.',
        review_unavailable:'The note operation failed. Check its receipt before trying again.',
        invalid_drc_request:'Invalid note text or target. Registered inputs and private files are protected.',
        drc_read_limit:'The selection or note exceeds the supported limit.',
        operation_conflict:'This sequence belongs to another approved request.',
        operation_sequence:'Another tab used this sequence. Read the notes again before a new approval.',
        operation_expired:'The receipt is no longer available. Check the note file before clearing the local record.'};
    function fail(){throw new Error('Invalid note response');}
    function keys(v,required,optional){if(!v||typeof v!=='object'||Array.isArray(v)||required.some(function(k){return !Object.prototype.hasOwnProperty.call(v,k);})||
        Object.keys(v).some(function(k){return !required.includes(k)&&!(optional||[]).includes(k);})){fail();}}
    function id(v){if(typeof v!=='string'||!/^[a-f0-9]{64}$/.test(v)){fail();}return v;}
    function text(v,max,empty){if(typeof v!=='string'||(!empty&&!v.length)||new TextEncoder().encode(v).length>max){fail();}return v;}
    function context(v){keys(v,['drc_id','revision','view_id']);Object.keys(v).forEach(function(k){id(v[k]);});return v;}
    function equal(a,b){return !!a&&!!b&&['drc_id','revision','view_id'].every(function(k){return a[k]===b[k];});}
    function terminal(v){return !!v&&['succeeded','failed','cancelled'].includes(v.phase);}
    function operation(v,P){
        keys(v,['seq','kind','phase'],['context','elapsed_ms','error','published','outcome_unknown','directory_synced','review_rev']);
        P.counter(v.seq);if(v.kind!=='drc_note'||!Object.prototype.hasOwnProperty.call(phases,v.phase)){fail();}
        if(v.context!==undefined){context(v.context);}if(v.elapsed_ms!==undefined){P.counter(v.elapsed_ms,true);}
        if(v.review_rev!==undefined){P.counter(v.review_rev,true);}
        if(v.phase==='queued'){if(Object.keys(v).length!==3){fail();}return v;}
        if(typeof v.outcome_unknown!=='boolean'||(!v.context&&!v.outcome_unknown)||
            (v.error!==undefined&&v.error!==null&&typeof v.error!=='string')){fail();}
        if(v.error){text(v.error,128);}
        if(v.outcome_unknown){if(v.phase!=='failed'||v.published!==null){fail();}}
        else if(typeof v.published!=='boolean'||v.published!==(v.phase==='succeeded')){fail();}
        if(v.published===true){if(typeof v.directory_synced!=='boolean'||v.error){fail();}}
        else if(v.directory_synced!==undefined&&v.directory_synced!==null){fail();}
        if(terminal(v)){P.counter(v.review_rev,true);}return v;
    }
    function catalog(v,P){
        keys(v,['available','editable','kind','reviewer','review_rev','operations','note_bytes','selection_limit','preparing','autosave'],['detached']);
        if(v.detached!==undefined&&(typeof v.detached!=='boolean'||v.detached&&(v.available||v.editable))){fail();}
        if(v.kind!=='drc_note'||typeof v.editable!=='boolean'||typeof v.available!=='boolean'||typeof v.preparing!=='boolean'||v.autosave!==false||v.note_bytes!==LIMIT||v.selection_limit!==5000){fail();}
        text(v.reviewer,200);P.counter(v.review_rev,true);
        const a=v.operations;keys(a,['last_seq','active','history']);P.counter(a.last_seq,true);if(a.active!==null){P.counter(a.active);}
        if(!Array.isArray(a.history)||a.history.length>32){fail();}let previous='0';
        a.history.forEach(function(r){operation(r,P);if(P.compare(r.seq,previous)<=0||P.compare(r.seq,a.last_seq)>0||(!terminal(r)&&a.active!==r.seq)){fail();}previous=r.seq;});
        if(previous!==a.last_seq||(a.active!==null&&(a.active!==previous||terminal(a.history[a.history.length-1])))){fail();}return v;
    }
    function report(v){keys(v,['skipped_lines','invalid_members','reassigned_members']);Object.keys(v).forEach(function(k){if(!Number.isSafeInteger(v[k])||v[k]<0){fail();}});return v;}
    function preview(v,c,count,P,prepared){
        const common=['kind','phase','name','selected_count','legacy_unverified','import_report','token','context','review_rev','reviewer','expires_in_ms','text'];
        keys(v,common.concat(prepared?['clears','replaces_existing','scope']:['existing_count','mixed','exists']));
        context(v.context);id(v.token);text(v.reviewer,200);text(v.name,4096);P.counter(v.review_rev,true);P.counter(v.selected_count);P.counter(v.expires_in_ms);
        if(!equal(v.context,c)||v.selected_count!==String(count)||typeof v.legacy_unverified!=='boolean'||v.kind!=='drc_note'||
            v.phase!==(prepared?'prepared':'snapshot')||P.compare(v.expires_in_ms,prepared?'30000':'120000')>0){fail();}
        report(v.import_report);
        if(prepared){text(v.text,LIMIT,true);if(v.scope!=='registered_reviewer_notes'||typeof v.replaces_existing!=='boolean'||v.clears!==(v.text==='')){fail();}}
        else{P.counter(v.existing_count,true);if(P.compare(v.existing_count,v.selected_count)>0||typeof v.mixed!=='boolean'||typeof v.exists!=='boolean'||(v.mixed&&v.text!==null)){fail();}
            if(v.text!==null){text(v.text,LIMIT,true);}}
        return v;
    }
    function approval(v,P){keys(v,['seq','context','token','approve','confirm_legacy']);P.counter(v.seq);context(v.context);id(v.token);
        if(v.approve!==true||typeof v.confirm_legacy!=='boolean'){fail();}return v;}
    function refs(v,P){if(!Array.isArray(v)||!v.length||v.length>5000){fail();}const seen=new Set();return v.map(function(r){keys(r,['check','error']);P.counter(r.check,true);P.counter(r.error,true);
        const k=r.check+':'+r.error;if(seen.has(k)){fail();}seen.add(k);return {check:r.check,error:r.error};});}
    function reportText(v){return Object.keys(v).filter(function(k){return v[k]>0;}).map(function(k){return k+': '+v[k];}).join(', ');}
    function statusText(v){
        if(!v){return 'No notes saved in this server session.';}
        let s=(v.outcome_unknown?'Save outcome UNKNOWN':phases[v.phase])+' · #'+v.seq;
        if(v.published===true){s+='\nNote sidecar saved. Layout and waive statuses are unchanged.';if(!v.directory_synced){s+='\nDirectory sync failed: saved, but durability is unconfirmed. Do not repeat the save just to retry sync.';}}
        if(v.outcome_unknown){s+='\nThe worker could have committed. Read back the notes before deciding on another edit.';}
        if(v.error){s+='\n'+(errors[v.error]||'Check local diagnostics and the receipt.');}return s;
    }
    function bind(o){
        const P=o.protocol,el=o.el;
        let enabled=false,stopped=false,model=null,stale=true,editor=null,draft=null,timer=null,expiry=null;
        let io=null,poll=null,write=null,cancelling=null,approving=null,revokeTask=null,revokeNext=null;
        let pending=null,uncertain=false,notice='',storageWarning='',readTurn=0,transferLocked=false;
        const hangul=H.bind({input:el('notes-text'),toggle:el('notes-hangul'),hint:el('notes-hangul-status'),changed:changed});
        const saveMode=SaveMode.bind({toggle:el('notes-autosave'),hint:el('notes-autosave-status'),kind:'note',changed:changed});
        function abort(t){if(t){t.cancelled=true;if(t.abort){t.abort();}}}
        function selection(){try{const c=o.selection();if(stopped||!c){return null;}context(c.context);id(c.epoch);text(c.key,256);text(c.caption,4096);
            if(!Number.isInteger(c.count)||c.count<1||c.count>5000){return null;}return c;}catch(_){return null;}}
        function same(c){const n=selection();return !!n&&equal(n.context,c.context)&&n.epoch===c.epoch&&n.key===c.key&&n.count===c.count;}
        function active(){return model&&model.operations.active;}
        function latest(){const rows=model&&model.operations.history;return rows&&rows[rows.length-1];}
        function displayReady(){return enabled&&!stopped&&!stale&&model&&model.available;}
        function permitted(){return displayReady()&&model.editable;}
        function busy(ignoreTransfer){return (!ignoreTransfer&&transferLocked)||!!pending||uncertain||!!active()||!!write||!!approving;}
        function transferReady(importing){return !!(permitted()&&!busy(true)&&!io&&!model.preparing&&!revokeTask&&(!importing||!editor));}
        async function publishTransfer(value,valid){
            if(!transferReady(true)||!valid()){return false;}const t={};approving=t;render();await refresh();
            if(approving!==t){return false;}approving=null;
            if(!transferReady(true)||!valid()||value.reviewer!==model.reviewer||value.review_rev!==model.review_rev){render();return false;}
            await send({seq:P.next(model.operations.last_seq),context:value.context,token:value.token,approve:true,confirm_legacy:true});return true;
        }
        function store(value){try{o.savePending(value?JSON.stringify({session_id:id(o.session()),request:value}):null);storageWarning='';}
            catch(_){storageWarning='Session storage unavailable. Keep this page open until the save outcome is confirmed.';}}
        function recover(){try{const raw=o.loadPending();if(raw===null){return;}if(typeof raw!=='string'||raw.length>2048){fail();}
            const v=JSON.parse(raw);keys(v,['session_id','request']);id(v.session_id);if(v.session_id!==o.session()){fail();}
            pending=approval(v.request,P);uncertain=true;notice='An earlier approved save needs confirmation. Reload did not repeat it.';
        }catch(_){uncertain=true;notice='An old approval record cannot be recovered. Check the note file before clearing the local record.';}}
        async function revoke(token){
            if(stopped||!enabled){return;}if(revokeTask){revokeNext=token;return;}const t={cancelled:false,abort:null};revokeTask=t;
            try{await o.http('POST',API+'/revoke',{token:token},false,t);}catch(_){/* Server expires unapproved capabilities too. */}
            finally{if(revokeTask===t){revokeTask=null;const next=revokeNext;revokeNext=null;if(next){revoke(next);}}}
        }
        function invalidate(reason){
            if(io){abort(io);io=null;}
            if(editor&&editor.token){revoke(editor.token);editor.token=null;}
            if(draft){revoke(draft.token);draft=null;}
            o.clearTimeout(expiry);expiry=null;el('notes-consent').checked=el('notes-legacy').checked=false;
            notice=reason+' No new save was approved.';
            if(editor){editor.invalid=true;notice+=' Your text is still here.';if(editor.approvedSeq){notice+=' An earlier approved save is not undone; check its receipt.';}}
        }
        function changed(){
            const c=editor?editor.selection:io&&io.selection;
            if(c&&!same(c)){invalidate('Selection, connection or DRC context changed.');}
            else if(editor&&!editor.invalid&&o.now()>=editor.until){invalidate('Snapshot or preview expired.');}
            render();
        }
        function automaticWarning(d){return d.legacy_unverified?'Legacy file confirmation is required':
            reportText(d.import_report)?'Existing note file has parse warnings; explicit review is required':'';}
        function approvalReady(grant){
            return !!draft&&!!editor&&!editor.invalid&&permitted()&&!busy()&&!io&&!model.preparing&&
                (grant?saveMode.valid(grant)&&!automaticWarning(draft):
                    el('notes-consent').checked&&(!draft.legacy_unverified||el('notes-legacy').checked));
        }
        function render(){
            el('notes-panel').hidden=!enabled;
            el('notes-owner').textContent=model?'Reviewer: '+model.reviewer+(model.detached?' · detached; previous receipts only':model.editable?'':' · read only'):'';
            // This container also owns the immutable receipts/recovery UI and
            // unsaved local text. Detachment disables writes, not their display.
            el('notes-authoring').hidden=!model||!model.editable&&!model.detached;
            const c=selection(),ok=permitted()&&!busy()&&!io&&!model.preparing;
            const connection=o.connection?o.connection():null;
            saveMode.sync(enabled&&!stopped&&model&&model.editable?model.reviewer:'',o.session()+'\n'+(connection||''),!!ok&&!!connection);
            el('notes-prepare').textContent=saveMode.on()?'Save note':'Preview save';
            el('notes-local').textContent='Local draft only until confirmed · lost on page reload. Ctrl/Cmd+Enter '+
                (saveMode.on()?'saves the note':'previews')+'; Escape discards. Browser IME is supported.';
            el('notes-selection').textContent=c?c.caption:'Select errors in a ready ICE review to edit notes.';
            el('notes-read').disabled=!ok||!c||!!editor;
            el('notes-editor').hidden=!editor;el('notes-text').disabled=!!io||!!draft||busy()||stopped;
            hangul.sync(!!editor&&!el('notes-text').disabled);
            el('notes-reload').disabled=!ok||!editor||!same(editor.selection);
            el('notes-prepare').disabled=!ok||!editor||editor.invalid||!!draft||!editor.token||new TextEncoder().encode(el('notes-text').value.trim()).length>LIMIT;
            el('notes-discard').disabled=!editor&&!io;
            el('notes-review').hidden=!draft;
            el('notes-consent').disabled=!draft||!ok;el('notes-legacy').disabled=!draft||!ok;
            el('notes-approve').disabled=!approvalReady(null);
            el('notes-cancel').hidden=!active();el('notes-cancel').disabled=!permitted()||!!cancelling;
            el('notes-refresh').disabled=stopped||!enabled||!!poll;
            el('notes-uncertain').hidden=!uncertain;el('notes-resolve').disabled=!pending||stopped||!!write||!!cancelling;
            el('notes-forget').disabled=!(permitted()||model&&model.detached&&!stopped&&!stale)||!!active()||!!write||!el('notes-checked').checked;
            el('notes-status').textContent=statusText(latest());el('notes-message').textContent=[notice,storageWarning].filter(Boolean).join('\n');
            el('notes-bytes').textContent=new TextEncoder().encode(el('notes-text').value).length+' / '+LIMIT+' UTF-8 bytes · empty text clears the selected notes';
            if(o.displayState){o.displayState(!enabled||!model?null:{reviewer:model.reviewer,review_rev:model.review_rev,read_turn:readTurn,
                blocked:!displayReady()?'Saved-note status is not ready.':busy(true)||latest()&&latest().outcome_unknown?'Saved-note publication is pending or unconfirmed.':
                    transferLocked?'Whole-review transfer is in progress; no save is implied.':
                    io||model.preparing?'Note snapshot preparation is in progress.':''});}
        }
        function schedule(){o.clearTimeout(timer);timer=null;if(enabled&&!stopped&&(active()||pending)){timer=o.setTimeout(refresh,active()?500:2500);}}
        function install(v){
            const old=latest(),next=v.operations.history[v.operations.history.length-1];
            if(model&&(v.editable!==model.editable&&!(v.detached&&!v.editable)||model.detached&&!v.detached||v.reviewer!==model.reviewer||P.compare(v.review_rev,model.review_rev)<0||P.compare(v.operations.last_seq,model.operations.last_seq)<0||
                old&&next&&old.seq===next.seq&&terminal(old)&&(['phase','elapsed_ms','error','published','outcome_unknown','directory_synced','review_rev'].some(function(k){return old[k]!==next[k];})||!!old.context!==!!next.context||old.context&&!equal(old.context,next.context)))){throw new Error('Older note state was ignored.');}
            if(editor&&model&&editor.approvedSeq!==v.operations.last_seq&&(v.review_rev!==model.review_rev||v.operations.last_seq!==model.operations.last_seq)){invalidate('Another save changed the note state.');}
            if(v.detached&&(!model||!model.detached)){saveMode.reset();invalidate('DRC replaced; this reviewer is detached.');}
            model=v;stale=false;v.operations.history.forEach(settled);
        }
        function settled(v){if(editor&&editor.accepted&&editor.approvedSeq===v.seq&&v.published===true&&equal(v.context,editor.selection.context)){clearEditor(false);}}
        function receipt(v){if(!model||P.compare(v.seq,model.operations.last_seq)<0){return;}const a=model.operations,old=latest();if(old&&old.seq===v.seq&&terminal(old)){return;}
            a.history=a.history.filter(function(r){return r.seq!==v.seq;}).concat([v]).slice(-32);a.last_seq=v.seq;a.active=terminal(v)?null:v.seq;
            if(v.review_rev!==undefined){model.review_rev=v.review_rev;}settled(v);}
        async function refresh(){
            if(!enabled||stopped){return;}o.clearTimeout(timer);timer=null;abort(poll);const t={cancelled:false,abort:null};poll=t;render();
            try{const v=catalog(await o.http('GET',API,undefined,false,t),P);if(t.cancelled||poll!==t||stopped){return;}install(v);}
            catch(e){if(!t.cancelled&&poll===t){stale=true;notice='Note status unavailable. '+(errors[e.code]||e.message);}}
            finally{if(poll===t){poll=null;changed();schedule();}}
        }
        async function read(keep){
            changed();const c=selection();if(!permitted()||busy()||io||!c||model.preparing||(editor&&!keep)||(keep&&(!editor||!same(editor.selection)))){return;}
            let rows;try{rows=refs(c.references(),P);if(rows.length!==c.count){fail();}}catch(e){notice=e.message;render();return;}
            const saved=keep?el('notes-text').value:null;
            if(editor){invalidate('Reloading snapshot.');}
            const t={selection:c,cancelled:false,abort:null,sent:o.now()};let focus=false;io=t;notice='Reading selected notes. No file is changed.';render();
            try{const v=preview(await o.http('POST',API+'/read',{context:c.context,errors:rows},false,t),c.context,c.count,P,false);
                if(t.cancelled||io!==t||!same(c)){revoke(v.token);return;}
                if(v.reviewer!==model.reviewer){fail();}
                ++readTurn;
                editor={selection:c,token:v.token,until:Math.min(t.sent+120000,o.now()+Number(v.expires_in_ms)),invalid:false};draft=null;
                el('notes-text').value=saved===null?(v.text||''):saved;
                el('notes-target').textContent=c.caption+'\n'+v.name+'\n'+v.existing_count+' selected errors already have notes.';
                notice=(v.mixed?'Selected notes differ. Saving will replace all selected notes with this text.':'Note snapshot loaded.')+
                    (saved!==null?' Local text retained; review against this new snapshot.':'')+(reportText(v.import_report)?'\nExisting file parse report: '+reportText(v.import_report):'');
                expiry=o.setTimeout(changed,Math.max(0,editor.until-o.now()));focus=true;
            }catch(e){if(!t.cancelled&&io===t){notice=errors[e.code]||e.message;}}
            // Hidden/disabled controls cannot receive browser focus. Render after releasing IO first.
            finally{if(io===t){io=null;changed();if(focus&&editor&&!editor.invalid&&!el('notes-text').disabled){el('notes-text').focus();}}}
        }
        async function prepare(grant){
            changed();if(el('notes-prepare').disabled){return;}const e=editor,c=e.selection,token=e.token;
            e.token=null;e.invalid=true;o.clearTimeout(expiry);expiry=null;
            const t={selection:c,cancelled:false,abort:null,sent:o.now()};let prepared=null,focus=false;io=t;notice='Preparing the selected edit. No file is changed yet.';render();
            try{const v=preview(await o.http('POST',API+'/prepare',{context:c.context,token:token,text:el('notes-text').value},false,t),c.context,c.count,P,true);
                if(t.cancelled||io!==t||editor!==e||!same(c)){revoke(v.token);return;}if(v.reviewer!==model.reviewer){fail();}
                draft=prepared=v;e.invalid=false;e.until=Math.min(t.sent+30000,o.now()+Number(v.expires_in_ms));
                el('notes-preview').textContent=v.clears?'(Clear notes on these errors)':v.text;
                el('notes-preview-target').textContent=c.caption+'\n'+v.name+'\n'+(v.replaces_existing?'Replace existing note sidecar':'Create note sidecar')+' · unrelated note groups preserved';
                el('notes-report').textContent=reportText(v.import_report)?'Existing file parse report: '+reportText(v.import_report):'';
                el('notes-legacy-row').hidden=!v.legacy_unverified;el('notes-consent').checked=el('notes-legacy').checked=false;
                notice='Review the exact text above. Approval expires after 30 seconds. Layout and waive files are not changed.';
                expiry=o.setTimeout(changed,Math.max(0,e.until-o.now()));focus=true;
            }catch(error){if(!t.cancelled&&io===t){notice=(errors[error.code]||error.message)+' Your text is retained. Reload the snapshot to prepare again.';}}
            finally{if(io===t){io=null;changed();if(focus&&draft&&!el('notes-consent').disabled&&!(grant&&saveMode.valid(grant)&&!automaticWarning(draft))){el('notes-consent').focus();}}}
            if(grant&&prepared&&draft===prepared){
                if(saveMode.valid(grant)&&!automaticWarning(prepared)){await approve(grant);}
                else{notice=automaticWarning(prepared)?automaticWarning(prepared)+'; automatic save did not approve it.':
                    'Automatic save permission changed. Review and approve this preview explicitly; no save was submitted.';render();}
            }
        }
        function confirm(){changed();return prepare(saveMode.capture());}
        function clearEditor(notify){if(notify){invalidate('Draft discarded.');}else{o.clearTimeout(expiry);expiry=null;}
            editor=draft=null;hangul.close();el('notes-text').value='';el('notes-preview').textContent='';el('notes-consent').checked=el('notes-legacy').checked=false;}
        async function send(request){
            pending=request;uncertain=false;el('notes-checked').checked=false;store(request);abort(poll);poll=null;
            const t={cancelled:false,abort:null};write=t;notice='Submitting the approved note. Closing the view does not undo a committed save.';render();
            try{const v=operation(await o.http('POST',API,request,false,t),P);if(v.seq!==request.seq||(v.context&&!equal(v.context,request.context))){fail();}
                if(t.cancelled||write!==t||stopped){return;}
                if(editor&&editor.approvedSeq===request.seq&&equal(editor.selection.context,request.context)){editor.accepted=true;}
                receipt(v);settled(v);pending=null;uncertain=false;store(null);notice='Approval acknowledged. Check the save receipt below.';
            }catch(e){if(t.cancelled||write!==t||stopped){return;}const rejected=e.status>=400&&e.status<500&&e.status!==408&&e.code!=='operation_expired';
                uncertain=!rejected;if(rejected){pending=null;store(null);notice=(errors[e.code]||e.message)+(editor?' Your local text is retained.':'');}
                else{notice=errors[e.code]||'Outcome unknown. Resolve only resends the SAME approved request, never a new edit.';}}
            finally{if(write===t){write=null;if(!stopped){await refresh();}else{render();}}}
        }
        async function approve(grant){
            changed();if(!approvalReady(grant)){return;}const d=draft,e=editor,t={};approving=t;render();await refresh();
            if(approving!==t){return;}approving=null;changed();if(draft!==d||editor!==e||!approvalReady(grant)){
                if(grant){notice='Automatic save stopped before submission. Your text is retained; check the selection and preview.';}render();return;}
            try{const request={seq:P.next(model.operations.last_seq),context:d.context,token:d.token,approve:true,confirm_legacy:grant?false:el('notes-legacy').checked};
                o.clearTimeout(expiry);expiry=null;e.token=null;e.invalid=true;e.approvedSeq=request.seq;e.accepted=false;draft=null;
                el('notes-consent').checked=el('notes-legacy').checked=false;await send(request);
            }catch(error){notice=error.message;render();}
        }
        async function cancel(){const seq=active();if(!seq||!permitted()||cancelling){return;}const t={cancelled:false,abort:null};cancelling=t;render();
            try{const v=operation(await o.http('POST',API+'/'+seq+'/cancel',{},false,t),P);if(v.seq!==seq){fail();}if(!t.cancelled){receipt(v);notice='Cancellation requested. A committed save remains saved.';}}
            catch(e){if(!t.cancelled){notice='Cancellation unconfirmed. Refresh the receipt. '+e.message;}}
            finally{if(cancelling===t){cancelling=null;if(!stopped){await refresh();}}}
        }
        function discard(){clearEditor(true);notice='Local draft discarded. Previously approved saves are not undone.';render();el('notes-read').focus();}
        el('notes-read').onclick=function(){return read(false);};el('notes-reload').onclick=function(){return read(true);};
        el('notes-prepare').onclick=confirm;el('notes-discard').onclick=discard;
        el('notes-consent').onchange=el('notes-legacy').onchange=changed;el('notes-approve').onclick=function(){return approve(null);};
        el('notes-editor').onkeydown=function(e){if(hangul.composing(e)){return;}if(e.key==='Escape'){e.preventDefault();e.stopPropagation();discard();}
            else if(e.key==='Enter'&&(e.ctrlKey||e.metaKey)){e.preventDefault();e.stopPropagation();if(!draft){confirm();}}};
        el('notes-refresh').onclick=refresh;el('notes-cancel').onclick=cancel;
        el('notes-resolve').onclick=function(){if(model&&(model.editable||model.detached)&&pending&&uncertain&&!stopped&&!write&&!cancelling){return send(pending);}};
        el('notes-checked').onchange=render;el('notes-forget').onclick=function(){if(!el('notes-forget').disabled){pending=null;uncertain=false;store(null);el('notes-checked').checked=false;notice='Local recovery record cleared after your check. No request was sent.';render();}};
        render();
        return {attach:function(value){if(!value){if(!enabled){render();}return;}try{const v=catalog(value,P);if(!enabled){enabled=true;if(v.editable||v.detached){recover();}}install(v);changed();schedule();}
                catch(e){stale=true;notice=e.message;render();}},changed:changed,refresh:refresh,
            open:function(){if(!permitted()){return false;}changed();if(editor){el('notes-text').focus();}else{read(false);}return true;},
            transferReady:transferReady,transferLock:function(value){transferLocked=value===true;render();},publishTransfer:publishTransfer,
            stop:function(final){stopped=true;stale=true;saveMode.reset();approving=null;clearEditor(false);if(write){uncertain=true;}
                [io,poll,write,cancelling,revokeTask].forEach(abort);io=poll=write=cancelling=revokeTask=null;revokeNext=null;o.clearTimeout(timer);timer=null;
                if(final&&model&&model.editable){store(null);}render();},resume:function(){stopped=false;return enabled?refresh():Promise.resolve();}};
    }
    const api={bind:bind,catalog:catalog,preview:preview,operation:operation,approval:approval,refs:refs,statusText:statusText};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeDRCNotes=api;}
}(typeof window==='object'?window:this));
