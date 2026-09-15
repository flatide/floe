/* ES2017 whole-review transfer. Uploaded bytes are never an approval. The
 * existing note/waive controller owns publication, recovery and reader fences. */
(function(root){
    'use strict';
    const CHUNK=1048576,MAX=536870912,BASE='/api/v1/drc/review/';
    function fail(){throw new Error('Invalid review transfer response');}
    function keys(v,required,optional){if(!v||typeof v!=='object'||Array.isArray(v)||required.some(function(k){return !Object.prototype.hasOwnProperty.call(v,k);})||Object.keys(v).some(function(k){return !required.includes(k)&&!(optional||[]).includes(k);})){fail();}}
    function id(v){if(typeof v!=='string'||!/^[a-f0-9]{64}$/.test(v)){fail();}return v;}
    function kind(v){if(v!=='notes'&&v!=='waives'){fail();}return v;}
    function text(v,max){if(typeof v!=='string'||new TextEncoder().encode(v).length>max){fail();}return v;}
    function context(v){keys(v,['drc_id','revision','view_id']);Object.keys(v).forEach(function(k){id(v[k]);});return v;}
    function equal(a,b){return !!a&&!!b&&['drc_id','revision','view_id'].every(function(k){return a[k]===b[k];});}
    function bound(v,max,P){P.counter(v,true);if(P.compare(v,String(max))>0){fail();}return v;}
    function report(v){keys(v,['skipped_lines','invalid_members','reassigned_members']);Object.keys(v).forEach(function(k){if(!Number.isSafeInteger(v[k])||v[k]<0){fail();}});return v;}
    function name(v,k,n){if(v!=='floe-'+k+'-'+n+(k==='notes'?'.fe':'.waive')){fail();}}
    function artifact(v,k,P,summary){
        keys(v,summary?['id','name','bytes','review_rev','contents','legacy_unverified','import_report']:['id','name','bytes','review_rev','context','expires_in_ms']);
        P.counter(v.id);bound(v.bytes,MAX,P);P.counter(v.review_rev,true);name(v.name,k,v.id);
        if(summary){if(typeof v.legacy_unverified!=='boolean'){fail();}report(v.import_report);counts(v.contents,k,P);}
        else{context(v.context);bound(v.expires_in_ms,600000,P);}return v;
    }
    function counts(v,k,P){keys(v,k==='notes'?['groups','members']:['waived_count']);Object.keys(v).forEach(function(n){P.counter(v[n],true);});}
    function preview(v,k,P){
        keys(v,['kind','phase','action','name','replaces_existing','legacy_unverified','scope','bytes','token','context','review_rev','reviewer','expires_in_ms'].concat(k==='notes'?['groups','members','clears','import_report']:['waived_count']));
        if(v.kind!==(k==='notes'?'drc_note':'drc_waive')||v.phase!=='prepared'||v.action!=='replace_all'||v.scope!=='registered_reviewer_entire_review'||v.legacy_unverified!==true||typeof v.replaces_existing!=='boolean'){fail();}
        id(v.token);context(v.context);P.counter(v.review_rev,true);text(v.name,4096);text(v.reviewer,200);bound(v.bytes,MAX,P);bound(v.expires_in_ms,30000,P);
        if(k==='notes'){P.counter(v.groups,true);P.counter(v.members,true);report(v.import_report);if(v.clears!==(v.members==='0')){fail();}}
        else{P.counter(v.waived_count,true);}return v;
    }
    function terminal(v){return ['succeeded','failed','cancelled'].includes(v.phase);}
    function operation(v,k,P){
        keys(v,['kind','seq','phase'],['context','action','upload','preview','artifact','error']);P.counter(v.seq);
        if(v.kind!=='drc_review_transfer'){fail();}if(v.phase==='queued'){if(Object.keys(v).length!==3){fail();}return v;}
        if(!terminal(v)||!['import','chunk','prepare','export'].includes(v.action)){fail();}context(v.context);
        if(v.phase!=='succeeded'){text(v.error,128);if(v.upload||v.preview||v.artifact){fail();}return v;}
        if(v.error!==undefined){fail();}
        const field=v.action==='export'?'artifact':v.action==='prepare'?'preview':'upload';
        if(['upload','preview','artifact'].some(function(f){return (v[f]!==undefined)!==(f===field);})){fail();}
        if(field==='artifact'){artifact(v.artifact,k,P,true);}else if(field==='preview'){preview(v.preview,k,P);if(!equal(v.preview.context,v.context)){fail();}}
        else{const u=v.upload;keys(u,['token','bytes','received','expires_in_ms']);id(u.token);bound(u.bytes,MAX,P);bound(u.received,MAX,P);bound(u.expires_in_ms,600000,P);if(u.bytes==='0'||P.compare(u.received,u.bytes)>0){fail();}}
        return v;
    }
    function catalog(v,k,P){
        keys(v,['kind','available','operations','upload','artifacts','limits','usage']);if(v.kind!=='drc_review_transfer'||typeof v.available!=='boolean'){fail();}
        keys(v.limits,['chunk_bytes','file_bytes','entries','readers','ttl_seconds']);
        if(v.limits.chunk_bytes!==CHUNK||v.limits.file_bytes!==String(MAX)||v.limits.entries!==2||v.limits.readers!==1||v.limits.ttl_seconds!==600){fail();}
        keys(v.usage,['entries','bytes','pending','readers']);bound(v.usage.bytes,2*MAX,P);
        ['entries','pending','readers'].forEach(function(n){if(!Number.isInteger(v.usage[n])||v.usage[n]<0||v.usage[n]>(n==='readers'?1:2)){fail();}});if(v.usage.pending>v.usage.entries){fail();}
        if(v.upload!==null){keys(v.upload,['token','context','bytes','received']);id(v.upload.token);context(v.upload.context);bound(v.upload.bytes,MAX,P);bound(v.upload.received,MAX,P);if(v.upload.bytes==='0'||P.compare(v.upload.received,v.upload.bytes)>0){fail();}}
        if(!Array.isArray(v.artifacts)||v.artifacts.length>2){fail();}const ids=new Set();v.artifacts.forEach(function(a){artifact(a,k,P,false);if(ids.has(a.id)){fail();}ids.add(a.id);});
        const a=v.operations;keys(a,['last_seq','active','history']);P.counter(a.last_seq,true);if(a.active!==null){P.counter(a.active);}if(!Array.isArray(a.history)||a.history.length>32){fail();}
        let last='0';a.history.forEach(function(r){operation(r,k,P);if(P.compare(r.seq,last)<=0||P.compare(r.seq,a.last_seq)>0||(!terminal(r)&&a.active!==r.seq)){fail();}last=r.seq;});
        if(last!==a.last_seq||(a.active!==null&&(a.active!==last||terminal(a.history[a.history.length-1])))){fail();}return v;
    }
    function download(doc,csrf,k,n,P){id(csrf);kind(k);P.counter(n);const form=doc.createElement('form'),input=doc.createElement('input');
        form.method='POST';form.action=BASE+k+'/artifacts/'+n+'/download';form.enctype='application/x-www-form-urlencoded';form.target='_blank';form.rel='noopener noreferrer';form.hidden=true;
        input.type='hidden';input.name='csrf';input.value=csrf;form.appendChild(input);doc.body.appendChild(form);try{form.submit();}finally{input.value='';doc.body.removeChild(form);}}
    function bind(o){
        const P=o.protocol,el=o.el;
        let selected='notes',enabled=false,stopped=false,available={},model=null,stale=true,job=null,pending=null,draft=null,publishing=null;
        let io=null,poll=null,timer=null,expiry=null,running=false,cleaning=false,notice='',lastLock=false;
        let fileKey=null;const fileRows=new Map();
        function api(){return BASE+selected;}
        function editor(){return o.editors[selected];}
        function scope(){try{const c=o.context();if(!c||stopped){return null;}context(c.context);id(c.epoch);return c;}catch(_){return null;}}
        function same(s){const c=scope();return c&&s&&c.epoch===s.epoch&&equal(c.context,s.context);}
        function busy(){return cleaning||!!job||!!pending||!!draft||!!publishing||!!(model&&model.operations.active);}
        function abort(t){if(t){t.cancelled=true;if(t.abort){t.abort();}if(t.wake){t.wake();}}}
        function lock(){const value=busy();if(value===lastLock){return;}lastLock=value;Object.keys(o.editors).forEach(function(k){const e=o.editors[k];if(e){e.transferLock(value);}});}
        function clearDraft(){o.clearTimeout(expiry);expiry=null;draft=null;el('transfer-consent').checked=el('transfer-run').checked=false;}
        function currentFile(){const files=el('transfer-file').files;return files&&files.length===1?files[0]:null;}
        function ready(importing){return enabled&&!stopped&&!stale&&model&&model.available&&scope()&&editor()&&editor().transferReady(importing);}
        function render(){
            lock();const occupied=busy(),can=ready(false);
            el('transfer-panel').hidden=!enabled;el('transfer-kind').disabled=occupied||stopped;
            el('transfer-waives-option').disabled=!available.waives;
            el('transfer-file').disabled=occupied||!can;
            el('transfer-import').disabled=occupied||!ready(true)||!currentFile()||!!(model&&model.upload);
            el('transfer-export').disabled=occupied||!can||!!(model&&model.upload);
            el('transfer-refresh').disabled=stopped||!enabled||!!poll;
            el('transfer-cancel').disabled=stopped||(!job&&!(model&&model.operations.active));
            el('transfer-discard').disabled=stopped||cleaning||!!publishing||(!draft&&!job&&!pending&&!(model&&model.upload));
            el('transfer-resolve').hidden=!pending||running;el('transfer-resolve').disabled=stopped||!job||!same(job.scope)||job.cancelled;
            el('transfer-review').hidden=!draft;el('transfer-approve').disabled=!draft||!ready(true)||!!publishing||!el('transfer-consent').checked||!el('transfer-run').checked;
            el('transfer-message').textContent=notice;
            const u=model&&model.upload;el('transfer-progress').textContent=job&&job.file?'Upload '+job.offset+' / '+job.file.size+' bytes':u?'Server upload '+u.received+' / '+u.bytes+' bytes. Reload never resumes a file upload automatically.':'';
            el('transfer-usage').textContent=model?model.usage.entries+'/2 temporary file slots · '+(Number(model.usage.bytes)/CHUNK).toFixed(1)+' MiB reserved/retained · '+model.usage.readers+'/1 downloads':'';
            const list=el('transfer-files'),files=model?model.artifacts:[],key=selected+':'+files.map(function(a){return a.id;}).join(',');
            if(key!==fileKey){fileKey=key;list.textContent='';fileRows.clear();
            files.forEach(function(a){
                const row=o.document.createElement('div'),label=o.document.createElement('p'),save=o.document.createElement('button'),release=o.document.createElement('button');
                label.textContent=a.name+' · '+a.bytes+' bytes · expires in '+Math.ceil(Number(a.expires_in_ms)/1000)+' s';
                save.type=release.type='button';save.textContent='Download';release.textContent='Release';
                save.disabled=stopped||stale||!scope()||!equal(a.context,scope().context);release.disabled=stopped||stale;
                save.onclick=function(){if(save.disabled){return;}try{o.download(selected,a.id);notice='Download requested. Check browser downloads; this does not save or import a review.';}catch(e){notice=e.message;}render();};
                release.onclick=async function(){if(release.disabled){return;}try{await o.http('DELETE',api()+'/artifacts/'+a.id);notice='Temporary export released; downloaded copies are unchanged.';}catch(e){notice=e.message;}await refresh();};
                row.appendChild(label);row.appendChild(save);row.appendChild(release);list.appendChild(row);fileRows.set(a.id,{label:label,save:save,release:release});
            });if(!files.length){list.textContent='No prepared downloads.';}}
            files.forEach(function(a){const row=fileRows.get(a.id);row.label.textContent=a.name+' · '+a.bytes+' bytes · expires in '+Math.ceil(Number(a.expires_in_ms)/1000)+' s';
                row.save.disabled=stopped||stale||!scope()||!equal(a.context,scope().context);row.release.disabled=stopped||stale;});
        }
        function schedule(){o.clearTimeout(timer);timer=null;if(enabled&&!stopped&&model&&(model.upload||model.artifacts.length||model.operations.active)){timer=o.setTimeout(refresh,model.operations.active?500:2500);}}
        async function refresh(){
            if(!enabled||stopped){return;}abort(poll);const t={cancelled:false,abort:null,kind:selected};poll=t;
            try{const v=catalog(await o.http('GET',BASE+t.kind+'/transfer',undefined,false,t),t.kind,P);if(t.cancelled||poll!==t||stopped||selected!==t.kind){return;}
                if(model&&P.compare(v.operations.last_seq,model.operations.last_seq)<0){throw new Error('Older transfer state ignored');}model=v;stale=false;
            }catch(e){if(!t.cancelled&&poll===t){stale=true;notice='Transfer state unavailable. '+e.message;}}
            finally{if(poll===t){poll=null;changed();schedule();}}
        }
        function remember(v){const a=model.operations;a.last_seq=v.seq;a.active=terminal(v)?null:v.seq;a.history=a.history.filter(function(r){return r.seq!==v.seq;}).concat([v]).slice(-32);}
        function pause(t){return new Promise(function(resolve){let n;const wake=function(){o.clearTimeout(n);t.wake=null;resolve();};t.wake=wake;n=o.setTimeout(wake,500);});}
        async function transact(j,action,fields,blob){
            if(!pending){pending={request:Object.assign({seq:P.next(model.operations.last_seq),context:j.scope.context,action:action},fields),blob:blob,offset:j.offset,sent:o.now()};}
            const p=pending,t={cancelled:false,abort:null,wake:null};io=t;render();
            try{let v=operation(await (p.blob?o.chunk(selected,p.request,p.offset,p.blob,t):o.http('POST',api()+'/transfer',p.request,false,t)),selected,P);
                while(true){if(t.cancelled||j.cancelled||stopped||job!==j||!same(j.scope)){throw new Error('Transfer interrupted; refresh and discard its temporary upload.');}
                    if(v.seq!==p.request.seq||(v.context&&!equal(v.context,p.request.context))||(v.action&&v.action!==p.request.action)){fail();}remember(v);
                    if(terminal(v)){pending=null;if(v.phase!=='succeeded'){throw new Error(v.error||'Transfer failed');}return {value:v,sent:p.sent};}
                    await pause(t);if(t.cancelled||j.cancelled){throw new Error('Transfer interrupted');}
                    v=operation(await o.http('GET',api()+'/transfer/'+p.request.seq,undefined,false,t),selected,P);
                }
            }catch(e){if(e.status>=400&&e.status<500&&e.status!==408){pending=null;}throw e;}
            finally{if(io===t){io=null;}}
        }
        async function drive(){
            if(running||!job||job.cancelled||!same(job.scope)||stopped){return;}running=true;const j=job;render();
            try{while(job===j&&!j.cancelled&&same(j.scope)&&!stopped){
                if(j.stage==='export'){await transact(j,'export',{},null);job=null;notice='Export prepared. Download it below; the review was not changed.';break;}
                if(j.stage==='import'){const result=await transact(j,'import',{bytes:String(j.file.size)},null);j.token=result.value.upload.token;j.stage='chunk';}
                else if(j.stage==='chunk'){
                    if(j.offset===j.file.size){j.stage='prepare';continue;}
                    const blob=pending?pending.blob:j.file.slice(j.offset,Math.min(j.file.size,j.offset+CHUNK));
                    const result=await transact(j,'chunk',{token:j.token},blob),u=result.value.upload;
                    if(u.token!==j.token||u.bytes!==String(j.file.size)||u.received!==String(Math.min(j.file.size,j.offset+CHUNK))){fail();}j.offset=Number(u.received);render();
                }else{const result=await transact(j,'prepare',{token:j.token},null),v=result.value.preview;
                    if(!equal(v.context,j.scope.context)){fail();}
                    draft={value:v,scope:j.scope,until:Math.min(result.sent+30000,o.now()+Number(v.expires_in_ms))};job=null;
                    el('transfer-preview').textContent=(v.replaces_existing?'Replace':'Create')+' '+v.name+'\nReviewer: '+v.reviewer+'\n'+
                        (selected==='notes'?v.groups+' groups · '+v.members+' annotated errors'+(v.clears?' · CLEAR ALL NOTES':'')+'\nParse report: '+JSON.stringify(v.import_report):v.waived_count+' waived errors · reserved statuses preserved from the imported file')+
                        '\nThis replaces the ENTIRE review, not the selection. Entries absent from this file are not merged with existing data.';
                    notice='Upload verified; nothing saved. Verify the DRC run and approve the whole replacement. Preview expires after 30 seconds.';
                    expiry=o.setTimeout(changed,Math.max(0,draft.until-o.now()));el('transfer-consent').checked=el('transfer-run').checked=false;break;
                }
            }}catch(e){if(!stopped&&job===j){notice=e.message+(pending?' Resolve retries only the identical request; it does not approve a save.':' No review was saved. Discard temporary state before restarting.');}}
            finally{running=false;if(!stopped){await editor().refresh();await refresh();}else{render();}}
        }
        async function start(importing){changed();if((importing?el('transfer-import'):el('transfer-export')).disabled){return;}const file=importing?currentFile():null;
            if(file&&(!Number.isSafeInteger(file.size)||file.size<1||file.size>(selected==='notes'?16*CHUNK:MAX))){notice='File size must be 1 byte to '+(selected==='notes'?'16':'512')+' MiB. The server validates its contents and exact waive length.';render();return;}
            job={scope:scope(),file:file,offset:0,token:null,stage:importing?'import':'export',cancelled:false};notice=importing?'Uploading a private temporary copy; no review save is approved.':'Preparing a read-only review export.';await drive();
        }
        async function discard(){
            if(stopped||publishing||cleaning){return;}cleaning=true;const root=api(),j=job,p=pending,token=draft?draft.value.token:j&&j.token||model&&model.upload&&model.upload.token;
            if(j){j.cancelled=true;}abort(io);clearDraft();pending=null;job=null;
            render();try{const seq=model&&model.operations.active||p&&p.request.seq;if(seq){await o.http('POST',root+'/transfer/'+seq+'/cancel',{});}
                if(token){await o.http('POST',root+'/revoke',{token:token});}notice='Temporary transfer discarded. Earlier approved saves and downloaded files are not undone.';
            }catch(e){notice='Discard/cancellation unconfirmed. Refresh, then discard any remaining server upload. '+e.message;}
            finally{cleaning=false;}await refresh();
        }
        function changed(){
            if(job&&!same(job.scope)){job.cancelled=true;abort(io);notice='Review or connection changed. Refresh and discard temporary transfer state; no new save was approved.';}
            if(draft&&(!same(draft.scope)||o.now()>=draft.until)){const token=draft.value.token;clearDraft();notice='Whole-review preview expired or context changed. Import again to prepare a new approval.';
                if(!stopped){o.http('POST',api()+'/revoke',{token:token}).catch(function(){});}}
            render();
        }
        async function approve(){changed();if(el('transfer-approve').disabled){return;}const d=draft,t={};publishing=t;clearDraft();render();
            const valid=function(){return publishing===t&&!stopped&&same(d.scope)&&o.now()<d.until;};
            try{const submitted=await editor().publishTransfer(d.value,valid);notice=submitted?'Approval handed to the '+selected+' save panel. Check its save/reader receipt and recovery controls.':'Review state changed before approval; no replacement was submitted.';}
            catch(e){notice='Check the '+selected+' save panel for the approval outcome. '+e.message;}
            finally{if(publishing===t){publishing=null;}if(!stopped){await refresh();}else{render();}}
        }
        el('transfer-import').onclick=function(){return start(true);};el('transfer-export').onclick=function(){return start(false);};
        el('transfer-refresh').onclick=refresh;el('transfer-resolve').onclick=drive;el('transfer-cancel').onclick=discard;el('transfer-discard').onclick=discard;el('transfer-approve').onclick=approve;
        el('transfer-consent').onchange=el('transfer-run').onchange=changed;el('transfer-file').onchange=changed;
        el('transfer-kind').onchange=function(){if(busy()){el('transfer-kind').value=selected;return;}selected=kind(el('transfer-kind').value);if(!available[selected]){selected='notes';el('transfer-kind').value=selected;}
            abort(poll);poll=null;model=null;stale=true;el('transfer-file').value='';notice='';return refresh();};
        render();return {attach:function(v){available={notes:!!v.notes&&v.notes.editable===true,waives:!!v.waives};const first=!enabled;enabled=available.notes||available.waives;if(first&&enabled){return refresh();}changed();},changed:changed,refresh:refresh,
            stop:function(final){stopped=true;abort(io);abort(poll);poll=null;if(job){job.cancelled=true;job.file=null;}if(pending){pending.blob=null;}el('transfer-file').value='';clearDraft();publishing=null;if(final){job=pending=model=null;notice='Session ended. Temporary transfers are no longer available; earlier committed saves are not undone.';}o.clearTimeout(timer);timer=null;render();},
            resume:function(){stopped=false;notice='Refresh shows remaining temporary uploads and exports. Reload/reconnect never approves a replacement.';return refresh();}};
    }
    const api={bind:bind,catalog:catalog,operation:operation,preview:preview,artifact:artifact,download:download};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeDRCTransfer=api;}
}(typeof window==='object'?window:this));
