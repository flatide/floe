/* ES2017. Server file handles, not browser upload or arbitrary path input. */
(function(root){'use strict';
    function stable(v){if(v&&typeof v==='object'&&!Array.isArray(v)){const out={};Object.keys(v).sort().forEach(function(k){out[k]=v[k];});return JSON.stringify(out);}return JSON.stringify(v);}
    function id(v){return typeof v==='string'&&/^[0-9a-f]{64}$/.test(v);}
    function page(v){
        if(!v||!id(v.snapshot)||!id(v.directory)||!Number.isSafeInteger(v.total)||v.total<0||v.total>100000||
            !Number.isSafeInteger(v.start)||v.start<0||v.start%128!==0||v.start>v.total||
            !Array.isArray(v.rows)||v.rows.length>128||v.rows.length!==Math.min(128,v.total-v.start)||
            v.next!==(v.start+v.rows.length<v.total?v.start+v.rows.length:null)||
            !Array.isArray(v.breadcrumbs)||!v.breadcrumbs.length||v.breadcrumbs.length>65||
            v.breadcrumbs.some(function(c){return !id(c.handle)||typeof c.name!=='string'||c.name.length>512;})||
            v.rows.some(function(r){return r.handle!==null&&!id(r.handle)||!['file','directory'].includes(r.kind)||typeof r.name!=='string'||r.name.length>512;})||
            !Number.isSafeInteger(v.skipped_names)||v.skipped_names<0||!Number.isSafeInteger(v.skipped_links)||v.skipped_links<0){throw Error('Invalid file catalogue response');}
        return v;
    }
    const errors={browse_changed:'An entry changed or its handle expired. Refresh the directory.',
        browse_busy_or_limit:'Catalogue or owner is busy, or this directory exceeds the scan/match limit. Wait, narrow the name filter, or choose a smaller approved root.',
        browse_cancelled:'File operation cancelled. No open request was published.',
        browse_invalid_selection:'Select a supported, unchanged OASIS or jobdeck file within the approved roots. GDS/gzip are not supported yet.',
        browse_read_error:'The directory or file could not be read. Refresh or choose another approved root.'};
    function bind(o){
        const el=o.el,doc=o.document,panel=el('browse-dialog'),button=el('browse-open');
        let enabled=false,paused=true,opened=false,prior=null,hidden=[],roots=[],current=null,selected=null;
        let pending=null,busy=false,invalid=false,task=null,timer=null,error='';
        function paint(){
            button.disabled=!enabled||!o.available();button.hidden=!enabled;
            const locked=!!pending||busy||invalid;
            ['browse-root','browse-filter','browse-query','browse-refresh'].forEach(function(k){el(k).disabled=locked;});
            el('browse-select').disabled=locked||!selected;
            el('browse-prev').disabled=locked||!current||current.start===0;
            el('browse-next').disabled=locked||!current||current.next===null;
            el('browse-check').hidden=!pending;el('browse-check').disabled=busy||invalid;
            el('browse-cancel').hidden=!pending;el('browse-cancel').disabled=busy||invalid;
            el('browse-close').disabled=!!pending||invalid;
            el('browse-status').textContent=error||(pending?'Reading server catalogue… (cancellation does not interrupt a blocked filesystem call)':
                current?(current.total?String(current.start+1)+'–'+String(current.start+current.rows.length)+' of '+current.total:'No matching entries')+
                ' · '+current.skipped_links+' links/special files and '+current.skipped_names+' unreadable names skipped.':'Choose an approved server folder.');
            panel.setAttribute('aria-busy',pending?'true':'false');
            el('browse-selection').textContent=selected?selected.name:'No file selected';
            Array.from(el('browse-entries').querySelectorAll('button')).forEach(function(b){b.disabled=locked||b.getAttribute('data-unavailable')==='true';b.setAttribute('aria-pressed',selected&&selected.handle===b.getAttribute('data-handle')?'true':'false');});
            Array.from(el('browse-crumbs').querySelectorAll('button')).forEach(function(b){b.disabled=locked;});
        }
        function save(value){o.savePending(value===null?null:JSON.stringify(value));pending=value;}
        function show(){
            if(opened){return;}opened=true;prior=doc.activeElement;panel.hidden=false;button.setAttribute('aria-expanded','true');
            ['app-header','app-workspace'].forEach(function(k){const n=el(k);hidden.push([n,n.getAttribute('aria-hidden')]);n.setAttribute('aria-hidden','true');});
            el('browse-query').focus();o.changed();paint();
        }
        function close(restore){
            if(!opened){return;}opened=false;panel.hidden=true;button.setAttribute('aria-expanded','false');
            hidden.forEach(function(p){if(p[1]===null){p[0].removeAttribute('aria-hidden');}else{p[0].setAttribute('aria-hidden',p[1]);}});hidden=[];
            if(restore){(prior&&doc.contains(prior)?prior:button).focus();}prior=null;o.changed();
        }
        function draw(value){
            current=page(value);selected=null;el('browse-entries').textContent='';el('browse-crumbs').textContent='';
            current.breadcrumbs.forEach(function(c){const b=doc.createElement('button');b.type='button';b.textContent=c.name;b.onclick=function(){load(c.handle);};el('browse-crumbs').appendChild(b);});
            el('browse-root').value=current.breadcrumbs[0].handle;
            current.rows.forEach(function(row){const li=doc.createElement('li'),b=doc.createElement('button');b.type='button';
                b.setAttribute('data-handle',row.handle||'');b.setAttribute('data-unavailable',row.handle===null?'true':'false');
                b.textContent=(row.kind==='directory'?'▸ ':'')+row.name+(row.kind==='file'&&typeof row.bytes==='string'?'  ·  '+row.bytes+' B':'');
                if(row.handle===null){b.title='Entry exceeds the depth/path limit; start with a narrower approved root.';}
                b.onclick=function(){if(pending||busy||!row.handle){return;}if(row.kind==='directory'){load(row.handle);}else{selected=row;paint();}};
                b.ondblclick=function(){if(row.kind==='file'&&row.handle&&!pending&&!busy){selected=row;choose();}};
                li.appendChild(b);el('browse-entries').appendChild(li);
            });paint();
        }
        async function catalogue(){
            const v=await o.http('GET','/api/v1/browse');o.protocol.counter(v.last_seq,true);
            if(v.active!==null){o.protocol.counter(v.active);}
            if(!Array.isArray(v.roots)||!v.roots.length||v.roots.length>32||v.roots.some(function(r){return !id(r.handle)||typeof r.name!=='string'||r.name.length>512;})){throw Error('Invalid approved roots');}
            roots=v.roots;const was=el('browse-root').value;el('browse-root').textContent='';
            roots.forEach(function(r){const option=doc.createElement('option');option.value=r.handle;option.textContent=r.name;el('browse-root').appendChild(option);});
            el('browse-root').value=roots.some(function(r){return r.handle===was;})?was:roots[0].handle;return v;
        }
        function later(){o.clearTimeout(timer);timer=null;if(!paused&&pending&&!busy&&!error){timer=o.setTimeout(function(){check(false);},200);}}
        async function receive(value){
            if(!pending||!value){throw Error('Request receipt is not available. Check / retry the same request; no new selection was sent.');}
            if(value.seq!==pending.request.seq||stable(value.request)!==stable(pending.request)){throw Error('A different request owns this sequence. No result was applied; restart the workspace to clear this recovery record.');}
            if(['queued','running','cancelling'].includes(value.phase)){error='';return;}
            if(!['succeeded','failed','cancelled'].includes(value.phase)){throw Error('Invalid file operation state');}
            const kind=pending.request.kind;
            if(value.phase==='succeeded'){
                if(kind==='select'){
                    if(!value.result||!id(value.result.launch_id)||!id(value.result.source_id)){throw Error('Invalid selection receipt');}
                    save(null);close(true);o.selected();
                }else{const result=page(value.result&&value.result.page);save(null);if(opened){draw(result);}}
                error='';
            }else{save(null);error=errors[value.error]||'File operation failed; no open request was published.';}
            o.changed();
        }
        async function check(retry){
            if(paused||busy||!pending||invalid){return;}busy=true;error='';paint();
            const token={cancelled:false};task=token;
            try{
                let value=await o.http('GET','/api/v1/browse/'+pending.request.seq,undefined,true,token);
                if(token.cancelled||paused){return;}
                if(!value&&retry){value=await o.http('POST','/api/v1/browse',pending.request,false,token);}
                if(token.cancelled||paused){return;}await receive(value);
            }catch(e){if(!token.cancelled&&!paused){error=e.message||String(e);}}
            finally{if(task===token){task=null;}busy=false;paint();later();}
        }
        async function submit(input){
            if(paused||busy||pending||invalid){return;}busy=true;error='';paint();const token={cancelled:false};task=token;
            try{
                const cursor=await catalogue();if(token.cancelled||paused){return;}
                if(cursor.active!==null){throw Error('Another catalogue request is active. Wait for it to finish, then refresh.');}
                input.seq=o.protocol.next(cursor.last_seq);
                save({request:input}); // BEFORE any mutation: selection can publish an open proposal.
                const value=await o.http('POST','/api/v1/browse',input,false,token);
                if(!token.cancelled&&!paused){await receive(value);}
            }catch(e){if(!token.cancelled&&!paused){
                // This is a fresh admission (not recovery). A complete 4xx
                // response means this first send was rejected before enqueue.
                if(pending&&e.status>=400&&e.status<500&&e.status!==401){save(null);}
                error=e.message||String(e);
            }}
            finally{if(task===token){task=null;}busy=false;paint();o.changed();later();}
        }
        function load(directory){selected=null;submit({kind:'list',directory:directory,filter:el('browse-filter').value,query:el('browse-query').value});}
        function choose(){if(selected){submit({kind:'select',handle:selected.handle});}}
        async function cancel(){
            if(!pending||busy||paused){return;}busy=true;error='';paint();const token={cancelled:false};task=token;
            try{const value=await o.http('POST','/api/v1/browse/'+pending.request.seq+'/cancel',{},false,token);if(!token.cancelled&&!paused){await receive(value);}}
            catch(e){if(!token.cancelled&&!paused){error=e.message||String(e);}}
            finally{if(task===token){task=null;}busy=false;paint();later();}
        }
        async function open(){if(paused||!enabled||!o.available()||opened){return;}show();if(pending){check(false);}else if(!current){load(el('browse-root').value||roots[0].handle);} }
        button.onclick=open;el('browse-close').onclick=function(){if(!pending&&!invalid){close(true);}};
        el('browse-select').onclick=choose;el('browse-check').onclick=function(){check(true);};el('browse-cancel').onclick=cancel;
        el('browse-root').onchange=function(){load(el('browse-root').value);};
        el('browse-refresh').onclick=function(){load(current?current.directory:el('browse-root').value);};
        el('browse-filter').onchange=el('browse-refresh').onclick;
        el('browse-query').onkeydown=function(e){if(e.key==='Enter'){e.preventDefault();el('browse-refresh').onclick();}};
        el('browse-prev').onclick=function(){if(current){submit({kind:'page',snapshot:current.snapshot,start:current.start-128});}};
        el('browse-next').onclick=function(){if(current&&current.next!==null){submit({kind:'page',snapshot:current.snapshot,start:current.next});}};
        doc.addEventListener('keydown',function(e){
            if(!opened){return;}e.stopPropagation();
            if(e.key==='Enter'&&e.target===el('browse-query')){e.preventDefault();if(!pending&&!busy&&!invalid){el('browse-refresh').onclick();}return;}
            if(e.key==='Escape'){e.preventDefault();if(pending){cancel();}else if(!invalid){close(true);}return;}
            if(e.key==='Tab'){
                const items=Array.from(panel.querySelectorAll('button,input,select,[tabindex="0"]')).filter(function(n){return !n.disabled&&n.getClientRects().length;});
                if(!items.length){e.preventDefault();return;}const index=items.indexOf(doc.activeElement);
                if(index<0||e.shiftKey&&index===0||!e.shiftKey&&index===items.length-1){e.preventDefault();items[e.shiftKey?items.length-1:0].focus();}
            }
        },true);
        doc.addEventListener('focusin',function(e){if(opened&&!panel.contains(e.target)){panel.focus();}},true);
        function stop(){paused=true;o.clearTimeout(timer);if(task){task.cancelled=true;if(task.abort){task.abort();}}close(false);}
        async function resume(){if(!enabled){return;}paused=false;try{await catalogue();if(pending||invalid){show();if(!invalid){check(false);}}}catch(e){error=e.message;show();}paint();}
        async function init(supported,empty){
            enabled=!!supported;paint();if(!enabled){return;}
            try{const saved=o.loadPending();if(saved){const v=JSON.parse(saved);if(saved.length>2048||!v||!v.request||!['list','page','select'].includes(v.request.kind)){throw Error('Invalid saved file request');}o.protocol.counter(v.request.seq);pending=v;}}
            catch(e){invalid=true;error='Saved file request is invalid. Restart the workspace; no selection was retried.';}
            await resume();if(empty&&!pending&&!invalid){open();}
        }
        return {init:init,stop:stop,resume:resume,changed:paint,blocked:function(){return opened||!!pending||invalid;}};
    }
    const api={bind:bind,page:page};if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeBrowse=api;}
}(typeof window==='object'?window:globalThis));
