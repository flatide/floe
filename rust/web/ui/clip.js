/* ES2017 owner exact clip. Preparing a receipt is not approval. Accepted jobs
 * outlive pan, view close and this controller, but never the owner session. */
(function (root) {
    'use strict';
    const phases = {queued:'Queued',preparing:'Preparing',opening:'Opening exact geometry',clipping:'Clipping',
        finishing:'Finishing',cancelling:'Cancelling',ready:'Clip ready',failed:'Clip failed',cancelled:'Clip cancelled'};
    const errors = {stale_frame:'The view changed. Review the current view again.',frame_not_displayed:'Wait for a displayed frame, then review again.',
        export_draft_expired:'The preparation expired or was replaced. Review the clip again.',
        busy:'CPU, memory or file capacity is busy. Try fewer jobs, finish other work or release old files; nothing is retried automatically.',
        unsupported:'Jobdeck clip is not supported. Open a source OASIS layout.',
        worker_failed:'The native export failed. No complete clip was published.',io_error:'The export could not read or write a file.',
        incomplete:'The export exceeded a limit or its data changed. No partial file was accepted.',
        index_unavailable:'The index changed or is unavailable. Reopen a valid index.',
        operation_conflict:'This sequence belongs to a different request. No new export was submitted.',
        operation_expired:'This request left the retained history. No new export was submitted.',
        operation_sequence:'Another request changed the operation sequence. Review before submitting again.',
        artifact_unavailable:'The file expired or was released.',invalid_request:'The clip options are invalid.'};
    function fail() { throw new Error('Invalid clip response'); }
    function keys(v, names, optional) {
        if (!v || typeof v !== 'object' || Array.isArray(v) ||
            Object.keys(v).some(function (k) { return !names.includes(k) && !(optional || []).includes(k); }) ||
            names.some(function (k) { return !Object.prototype.hasOwnProperty.call(v,k); })) { fail(); }
    }
    function identity(v) { if (typeof v !== 'string' || !/^[0-9a-f]{64}$/.test(v)) { fail(); } return v; }
    function bounded(v, limit, P) { P.counter(v,true); if (P.compare(v,String(limit))>0) { fail(); } return v; }
    function bounds(v,P,Q) {
        if (!Array.isArray(v) || v.length!==4) { fail(); } v.forEach(function (s) { Q.i64(s,P); });
        if (Q.cmp(v[0],v[2],P)>=0 || Q.cmp(v[1],v[3],P)>=0) { fail(); } return v;
    }
    function file(v,P) {
        keys(v,['id','bytes','expires_in_ms','name']);P.counter(v.id);bounded(v.bytes,536870912,P);bounded(v.expires_in_ms,600000,P);
        if (v.name!=='floe-clip-'+v.id+'.oas') { fail(); } return v;
    }
    function terminal(op) { return !!op && ['ready','failed','cancelled'].includes(op.phase); }
    function operation(op,P,Q) {
        keys(op,['seq','kind','phase'],['view_id','dataset_revision','elapsed_ms','error','artifact']);P.counter(op.seq);
        if (op.kind!=='exact_clip' || !Object.prototype.hasOwnProperty.call(phases,op.phase)) { fail(); }
        if (op.view_id!==undefined) { identity(op.view_id); } if(op.dataset_revision!==undefined) { P.counter(op.dataset_revision); }
        if(op.elapsed_ms!==undefined) { P.counter(op.elapsed_ms,true); }
        if (op.error!==undefined && op.error!==null && (typeof op.error!=='string' || op.error.length>128)) { fail(); }
        if (op.phase==='ready') {
            const a=op.artifact;
            keys(a,['id','bytes','records','bbox_dbu','source_stale','name','available','expires_in_ms']);
            P.counter(a.id);bounded(a.bytes,536870912,P);P.counter(a.records,true);bounds(a.bbox_dbu,P,Q);
            if (typeof a.source_stale!=='boolean' || typeof a.available!=='boolean' || a.name!=='floe-clip-'+a.id+'.oas' ||
                (!a.available && a.expires_in_ms!==null)) { fail(); }
            if(a.available) {bounded(a.expires_in_ms,600000,P);}
        } else if (op.artifact!==undefined && op.artifact!==null) { fail(); }
        return op;
    }
    function catalog(v,P,Q) {
        keys(v,['operations','available','kind','jobs_default','jobs_min','jobs_max','limits','usage','artifacts']);
        if (typeof v.available!=='boolean' || v.kind!=='exact_clip' || v.jobs_default!==4 || v.jobs_min!==1 || v.jobs_max!==16) { fail(); }
        keys(v.limits,['artifacts','artifact_bytes','total_bytes','readers','ttl_seconds']);
        if(v.limits.artifacts!==4 || v.limits.artifact_bytes!=='536870912' || v.limits.total_bytes!=='2147483648' || v.limits.readers!==2 || v.limits.ttl_seconds!==600) { fail(); }
        keys(v.usage,['entries','pending','bytes','readers']);bounded(v.usage.bytes,2147483648,P);
        ['entries','pending','readers'].forEach(function (k) { if(!Number.isInteger(v.usage[k]) || v.usage[k]<0 || v.usage[k]>(k==='readers'?2:4)) {fail();} });
        if(v.usage.pending>v.usage.entries) {fail();}
        const a=v.operations;keys(a,['last_seq','active','history']);P.counter(a.last_seq,true);if(a.active!==null) {P.counter(a.active);}
        if (!Array.isArray(a.history) || a.history.length>32) {fail();} let prev='0';
        a.history.forEach(function (op) {operation(op,P,Q);if(P.compare(op.seq,prev)<=0 || P.compare(op.seq,a.last_seq)>0 || (!terminal(op)&&op.seq!==a.active)) {fail();} prev=op.seq;});
        if(prev!==a.last_seq || (a.active!==null && (a.active!==prev || terminal(a.history[a.history.length-1])))) {fail();}
        if(!Array.isArray(v.artifacts) || v.artifacts.length>4) {fail();} const ids=new Set();
        v.artifacts.forEach(function (f) {file(f,P);if(ids.has(f.id)) {fail();}ids.add(f.id);});return v;
    }
    function selection(layers) {return layers.mode==='only'?{mode:'only',count:layers.pairs.length}:{mode:layers.mode};}
    function prepared(m,t,P,Q) {
        keys(m,['type','seq','view_id','connection_epoch','source_stale','draft']);
        if(m.type!=='clip.prepared' || m.seq!==t.seq || m.view_id!==t.view || m.connection_epoch!==t.epoch || m.source_stale!==t.source_stale) {fail();}
        const d=m.draft;keys(d,['token','dataset_revision','bbox_dbu','layers','jobs','cell_name','expires_in_ms']);identity(d.token);P.counter(d.dataset_revision);
        bounds(d.bbox_dbu,P,Q);bounded(d.expires_in_ms,30000,P);
        keys(d.layers,t.layers.mode==='only'?['mode','count']:['mode']);
        if(d.dataset_revision!==t.anchor.dataset_revision || d.jobs!==t.jobs || d.cell_name!==t.cell_name || d.layers.mode!==t.layers.mode || d.layers.count!==t.layers.count) {fail();}
        return d;
    }
    function download(document,csrf,id,P) {
        identity(csrf);P.counter(id);
        const form=document.createElement('form'),input=document.createElement('input');
        form.method='POST';form.action='/api/v1/artifacts/'+id+'/download';form.enctype='application/x-www-form-urlencoded';
        // An expired-file error must not navigate away from the layout. The
        // new download context gets no opener and no secret in its URL.
        // Do not use noreferrer: it nulls the POST Origin required by the
        // gateway. Its same-origin referrer policy still protects external URLs.
        form.target='_blank';form.rel='noopener';form.hidden=true;
        input.type='hidden';input.name='csrf';input.value=csrf;form.appendChild(input);document.body.appendChild(form);
        try {form.submit();}finally{input.value='';document.body.removeChild(form);}
    }
    function label(op) {
        if(!op) {return 'No clip requested.';} let text=phases[op.phase]+' · #'+op.seq;
        if(op.elapsed_ms!==undefined) {text+=' · '+(Number(op.elapsed_ms)/1000).toFixed(1)+' s';}
        if(op.artifact) {text+=' · '+op.artifact.records+' records';}
        if(op.error) {text+='\n'+(errors[op.error]||op.error);}return text;
    }
    function bind(o) {
        const P=o.protocol,Q=o.query,el=function(id){return o.document.getElementById(id);};
        let enabled=false,stopped=false,model=null,stale=true,timer=null,getTask=null,writeTask=null,cancelTask=null;
        let editor='',draft=null,preparing=null,expiryTimer=null,pending=null,approvalTask=null,uncertain=false,waiting=false,note='',pollError='';
        const releasing=new Set();let filesKey='',fileRows=new Map();
        function abort(t) {if(t) {t.cancelled=true;if(t.abort) {t.abort();}}}
        function scope() {try {const c=o.context();return enabled&&!stopped&&c&&c.state.capabilities.clip?Q.scope(c,P,true):null;}catch(_) {return null;}}
        function active() {return model&&model.operations.active;}
        function last() {const a=model&&model.operations.history;return a&&a[a.length-1];}
        function permitted() {return enabled&&!stopped&&!stale&&model&&model.available;}
        function busy() {return waiting||uncertain||!!pending||!!active();}
        function cancelPreparation() {if(preparing) {o.clearTimeout(preparing.timeout);preparing=null;}o.clearTimeout(expiryTimer);expiryTimer=null;draft=null;}
        function dismiss(focus) {
            if(!editor) {return false;}editor='';cancelPreparation();el('clip-form').hidden=true;el('clip-open').setAttribute('aria-expanded','false');
            if(focus) {el('clip-open').focus();}return true;
        }
        function renderFiles() {
            const files=model?model.artifacts:[], key=files.map(function(f){return f.id;}).join(','),list=el('clip-files');
            if(key!==filesKey || !list.children.length) {
                const focused=o.document.activeElement,restore=focused&&focused.dataset&&focused.dataset.artifact?{id:focused.dataset.artifact,kind:focused.dataset.kind}:null;
                filesKey=key;list.textContent='';fileRows=new Map();
                files.forEach(function(f){
                const row=o.document.createElement('div');row.className='clip-file';
                const title=o.document.createElement('p');row.appendChild(title);
                const buttons=o.document.createElement('div');buttons.className='button-row';
                const save=o.document.createElement('button');save.type='button';save.textContent='Download OASIS';save.dataset.artifact=f.id;save.dataset.kind='save';
                save.onclick=function(){if(!permitted()||releasing.has(f.id)||!model.artifacts.some(function(x){return x.id===f.id;})) {return;}
                    try{o.download(f.id);note='Download requested. Check your browser’s downloads; the file stays available until expiry or release.';}catch(e){note=e.message;}render();};
                const release=o.document.createElement('button');release.type='button';release.textContent='Release';release.dataset.artifact=f.id;release.dataset.kind='release';
                release.onclick=function(){return remove(f.id);};buttons.appendChild(save);buttons.appendChild(release);row.appendChild(buttons);list.appendChild(row);
                fileRows.set(f.id,{title:title,save:save,release:release});
                });
                if(restore) {const r=fileRows.get(restore.id);(r&&r[restore.kind]||el('clip-refresh')).focus();}
            }
            files.forEach(function(f){
                const r=fileRows.get(f.id);r.title.textContent=f.name+' · '+(Number(f.bytes)/1048576).toFixed(2)+' MiB · expires in '+Math.ceil(Number(f.expires_in_ms)/1000)+' s';
                r.save.disabled=r.release.disabled=!permitted()||releasing.has(f.id);
            });
            if(!files.length) {list.textContent='No ready files.';}
        }
        function render() {
            const s=scope();
            if(editor&&(!s||s.key!==editor)) {dismiss(false);note='View or connection changed. Review the current view again.';}
            if(draft&&o.now()>=draft.until) {cancelPreparation();note='Preparation expired. Review the clip again.';}
            el('clip-panel').hidden=!enabled;
            el('clip-open').disabled=!permitted()||!s||busy();
            el('clip-prepare').disabled=!permitted()||!editor||!!preparing||busy();
            ['clip-layers','clip-jobs','clip-cell-name'].forEach(function(id){el(id).disabled=waiting||!!preparing;});
            el('clip-review').hidden=!draft;el('clip-approve').disabled=!permitted()||!draft||busy();
            el('clip-cancel').hidden=!active();el('clip-cancel').disabled=!permitted()||!!cancelTask||last()&&last().phase==='cancelling';
            el('clip-resolve').hidden=!uncertain;el('clip-resolve').disabled=stopped||waiting;
            el('clip-refresh').disabled=stopped||!enabled||!!getTask;
            el('clip-status').textContent=label(last());
            el('clip-note').textContent=pollError||note||(!s?'Open a connected layout and wait for a displayed frame. Jobdeck clip is unsupported.':'');
            el('clip-usage').textContent=model?model.usage.entries+'/4 artifact slots · '+(Number(model.usage.bytes)/1048576).toFixed(1)+' MiB reserved/retained · '+model.usage.readers+'/2 downloads':'';
            renderFiles();
        }
        function schedule() {o.clearTimeout(timer);timer=null;if(enabled&&!stopped) {timer=o.setTimeout(refresh,active()||waiting?500:2500);}}
        async function refresh() {
            if(!enabled||stopped) {return;}o.clearTimeout(timer);timer=null;abort(getTask);
            const t={cancelled:false,abort:null};getTask=t;render();
            try {
                const v=catalog(await o.http('GET','/api/v1/exports',undefined,false,t),P,Q);
                if(t.cancelled||stopped||getTask!==t) {return;}
                if(model&&P.compare(v.operations.last_seq,model.operations.last_seq)<0) {throw new Error('Older clip state was ignored');}
                model=v;stale=false;pollError='';
            } catch(e) {if(!t.cancelled&&!stopped&&getTask===t) {stale=true;pollError='Clip state could not be checked. '+e.message;if(e.status===401) {stopped=true;dismiss(false);}}}
            finally {if(getTask===t) {getTask=null;render();schedule();}}
        }
        function prepare() {
            render();const s=scope(),c=o.context();if(!permitted()||!editor||!s||busy()||preparing) {return;}
            const jobs=el('clip-jobs').value,name=el('clip-cell-name').value,layers=el('clip-layers').value;
            if(!/^(?:[1-9]|1[0-6])$/.test(jobs)||!['visible','all','none'].includes(layers)||!name||new TextEncoder().encode(name).length>4096||/[\u0000-\u001f\u007f-\u009f]/.test(name)) {note='Choose 1–16 jobs and a nonempty cell name without control characters (4096 UTF-8 bytes maximum).';render();return;}
            cancelPreparation();note='Preparing the current viewport; no export has been approved.';
            const t={view:c.id,epoch:c.state.connection_epoch,stamp:s.key,anchor:s.anchor,source_stale:c.state.source_stale,
                jobs:Number(jobs),cell_name:name,layers:layers==='visible'?selection(c.state.layers):{mode:layers},sent:o.now(),seq:null,timeout:null};
            preparing=t;
            try {t.seq=o.send({type:'view.clip.prepare',view_id:t.view,connection_epoch:t.epoch,body:{anchor:t.anchor,bounds:{kind:'viewport'},layers:layers,jobs:t.jobs,cell_name:name}});
                t.timeout=o.setTimeout(function(){if(preparing===t){cancelPreparation();note='Preparation timed out. No export was submitted.';render();}},5000);
            }catch(e){cancelPreparation();note=e.message;}render();
        }
        function receive(m) {
            const t=preparing;if(!t||m.seq!==t.seq) {return false;}o.clearTimeout(t.timeout);preparing=null;
            const s=scope();if(!s||s.key!==t.stamp||stopped||!editor) {render();return true;}
            try {if(m.type==='error') {throw new Error(errors[m.code]||m.code||'Preparation failed');}
                const d=prepared(m,t,P,Q);draft={value:d,until:Math.min(t.sent+30000,o.now()+Number(d.expires_in_ms)),stamp:t.stamp,view:t.view};
                el('clip-bounds').textContent=d.bbox_dbu.join(', ')+' DBU\n1 DBU = '+o.context().state.dbu_um+' µm';
                el('clip-selection').textContent=d.layers.mode==='only'?d.layers.count+' visible layer pairs':d.layers.mode==='none'?'No layers — the OASIS will be empty.':'All layers';
                el('clip-options').textContent=d.jobs+' jobs · cell '+d.cell_name;
                el('clip-stale').hidden=!m.source_stale;note='Review these frozen options, then approve. Preparation expires after 30 seconds.';
                expiryTimer=o.setTimeout(render,Math.max(0,draft.until-o.now()));
            }catch(e){cancelPreparation();note=e.message;}render();return true;
        }
        async function send(request) {
            waiting=true;uncertain=false;pending=request;abort(getTask);getTask=null;dismiss(false);note='';render();
            const t={cancelled:false,abort:null};writeTask=t;
            try {const op=operation(await o.http('POST','/api/v1/exports',request,false,t),P,Q);
                if(op.seq!==request.seq || op.view_id!==undefined&&op.view_id!==request.view_id) {throw new Error('Clip receipt mismatch');}
                if(t.cancelled||stopped||writeTask!==t) {return;}pending=null;
            }catch(e){if(t.cancelled||stopped||writeTask!==t) {return;}
                uncertain=!(e.status>=400&&e.status<500);if(!uncertain){pending=null;}
                note=uncertain?'Outcome unknown. Resolve request resends only the SAME approved request, never a new export.':(errors[e.code]||e.message);
                if(e.status===401){stopped=true;}
            }finally{if(writeTask===t){writeTask=null;waiting=false;if(!stopped){await refresh();}else{render();}}}
        }
        async function approve() {
            render();const d=draft;if(!permitted()||!d||busy()) {return;}const t={};approvalTask=t;waiting=true;render();
            await refresh();if(approvalTask!==t) {return;}approvalTask=null;waiting=false;render();const s=scope();
            if(!permitted()||draft!==d||!s||s.key!==d.stamp||o.now()>=d.until||active()) {render();return;}
            try {await send(Object.freeze({seq:P.next(model.operations.last_seq),view_id:d.view,token:d.value.token,approve:true}));}
            catch(e){note=e.message;render();}
        }
        async function cancel() {
            const seq=active();if(!seq||!permitted()||cancelTask) {return;}const t={cancelled:false,abort:null};cancelTask=t;render();
            try{const op=operation(await o.http('POST','/api/v1/exports/'+seq+'/cancel',{},false,t),P,Q);if(op.seq!==seq){fail();}if(!t.cancelled){note='Cancellation requested. A file already published stays successful.';}}
            catch(e){if(!t.cancelled){note='Cancellation was not confirmed. Refresh its state. '+e.message;}}
            finally{if(cancelTask===t){cancelTask=null;if(!stopped){await refresh();}}}
        }
        async function remove(id) {
            if(!permitted()||releasing.has(id)||!model.artifacts.some(function(f){return f.id===id;})) {return;}
            releasing.add(id);render();
            try {await o.http('DELETE','/api/v1/artifacts/'+id);note='Server file released. Any copy already downloaded is unchanged.';}
            catch(e){note='Release was not confirmed. Refresh the file list. '+e.message;}
            finally {releasing.delete(id);if(!stopped){await refresh();}}
        }
        el('clip-open').onclick=function(){render();const s=scope();if(!permitted()||!s||busy()){return;}editor=s.key;cancelPreparation();note='';
            el('clip-layers').value='visible';el('clip-jobs').value='4';el('clip-cell-name').value='FLOE_CLIP';el('clip-form').hidden=false;el('clip-open').setAttribute('aria-expanded','true');render();el('clip-layers').focus();};
        el('clip-form').onsubmit=function(e){e.preventDefault();prepare();};
        el('clip-approve').onclick=approve;el('clip-dismiss').onclick=function(){dismiss(true);render();};
        el('clip-form').onkeydown=function(e){if(e.key==='Escape'){e.preventDefault();e.stopPropagation();dismiss(true);render();}};
        ['clip-layers','clip-jobs','clip-cell-name'].forEach(function(id){el(id).oninput=function(){cancelPreparation();note='Options changed. Review before approving.';render();};el(id).onchange=el(id).oninput;});
        el('clip-cancel').onclick=cancel;el('clip-refresh').onclick=refresh;el('clip-resolve').onclick=function(){if(uncertain&&pending&&!waiting&&!stopped){return send(pending);}};
        render();
        return {init:function(available){stopped=false;enabled=available===true;render();return enabled?refresh():Promise.resolve();},changed:render,receive:receive,
            refresh:refresh,escape:function(){const closed=dismiss(true);render();return closed;},
            stop:function(){stopped=true;stale=true;dismiss(false);o.clearTimeout(timer);timer=null;if(writeTask){uncertain=true;}approvalTask=null;waiting=false;
                [getTask,writeTask,cancelTask].forEach(abort);getTask=writeTask=cancelTask=null;render();},
            resume:function(){stopped=false;stale=true;return refresh();}};
    }
    const api={bind:bind,catalog:catalog,operation:operation,prepared:prepared,file:file,download:download};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeClip=api;}
}(typeof window==='object'?window:this));
