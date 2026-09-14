/* Explicit owner settings I/O, ES2017. Never reads an unchosen file/path. */
(function(root,factory){const api=factory();if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeSettings=api;}}(typeof window==='object'?window:this,function(){
    'use strict';
    const MAX=4*1024*1024;
    function bind(o){
        const el=o.el,w=o.window;let enabled=false,job=null,choice=null,dead=false;const urls=new Map();
        function context(){const c=o.context();return c&&c.ready&&/^[a-f0-9]{64}$/.test(c.id)&&/^[a-f0-9]{64}$/.test(c.epoch)&&/^[1-9][0-9]*$/.test(c.rev)?c:null;}
        function same(c,revision){const n=context();return n&&n.id===c.id&&n.epoch===c.epoch&&(!revision||n.rev===c.rev);}
        function status(s){el('settings-status').textContent=s||'';}
        function controls(){const c=context(),ready=enabled&&!dead&&c&&c.idle&&!job;el('settings-panel').hidden=!enabled;el('settings-load').disabled=!ready||!w.FileReader;el('settings-save').disabled=!ready;el('settings-cancel').disabled=!job;}
        function finish(j,text){if(job!==j){return;}job=null;status(text);controls();}
        function cancel(message){const j=job;job=null;choice=null;el('settings-file').value='';if(j){if(j.reader){j.reader.abort();}if(j.xhr){j.xhr.abort();}if(j.cancel){j.cancel();}}if(message){status(message);}controls();}
        function changed(){if(job&&!same(job.context,job.phase!=='apply')){cancel('View changed; settings were not replayed.');}controls();}
        function current(j){if(job!==j){return false;}if(!same(j.context,true)){finish(j,'View changed; settings were not replayed.');return false;}return true;}
        function request(j,method,format,text){return new Promise(function(resolve,reject){
            const xhr=new o.XHR();j.xhr=xhr;xhr.open(method,'/api/v1/views/'+j.context.id+'/settings/'+j.context.rev+'/'+format);xhr.timeout=8000;xhr.setRequestHeader('X-Floe-CSRF',o.csrf());
            if(method==='POST'){xhr.setRequestHeader('Content-Type','text/plain; charset=utf-8');}
            function failure(message){j.xhr=null;reject(new Error(message));}
            xhr.onprogress=function(e){if(e.loaded>MAX+16384){xhr.abort();}};
            xhr.onerror=function(){failure('Settings service unavailable. Nothing was retried.');};xhr.onabort=function(){failure('Settings request cancelled.');};xhr.ontimeout=function(){failure('Settings request timed out. Nothing was retried.');};
            xhr.onload=function(){j.xhr=null;try{
                if(xhr.responseText.length>MAX||new o.Encoder().encode(xhr.responseText).length>MAX){throw new Error('Settings reply exceeds 4 MiB.');}
                if(xhr.status<200||xhr.status>=300){let message='Settings request failed.';try{const p=JSON.parse(xhr.responseText);message=p.error==='unsupported'?'Custom bitmaps or inherited-width overrides cannot be saved as Calibre. Choose Native JSON.':o.message(p.error);}catch(_){}throw new Error(message);}
                if(xhr.getResponseHeader('Content-Type')!==(method==='GET'?'text/plain; charset=utf-8':'application/json')){throw new Error('Invalid settings reply.');}resolve(xhr.responseText);
            }catch(e){reject(e);}};xhr.send(text===undefined?null:text);
        });}
        async function load(file,c){
            if(!enabled||dead||job||!c||!c.idle||!same(c,true)){status('Choose a file for the current connected view.');return;}
            if(!file||!Number.isSafeInteger(file.size)||file.size<0||file.size>MAX){status('Settings file must be at most 4 MiB.');return;}
            const j={context:c,phase:'read'};job=j;controls();status('Reading selected settings…');
            try{const bytes=await new Promise(function(resolve,reject){const reader=new w.FileReader();j.reader=reader;reader.onload=function(){j.reader=null;resolve(reader.result);};reader.onerror=reader.onabort=function(){j.reader=null;reject(new Error('Selected file could not be read.'));};reader.readAsArrayBuffer(file);});
                if(!current(j)){return;}if(!(bytes instanceof ArrayBuffer)||bytes.byteLength>MAX){throw new Error('Settings file exceeds 4 MiB.');}
                const text=new o.Decoder('utf-8',{fatal:true}).decode(bytes),format=text.trim().startsWith('{')?'native':'calibre';j.phase='prepare';status('Validating settings…');
                const reply=await request(j,'POST',format,text);if(!current(j)){return;}const p=JSON.parse(reply);
                if(p.view_id!==c.id||p.state_rev!==c.rev||!/^[a-f0-9]{64}$/.test(p.prepared_token)||!Number.isSafeInteger(p.rows)||p.rows<0||p.rows>65536||!Number.isSafeInteger(p.malformed)||p.malformed<0||p.malformed>MAX){throw new Error('Invalid prepared settings reply.');}
                j.phase='apply';status('Applying settings…');j.cancel=o.edit({prepared_token:p.prepared_token},function(error){finish(j,error?o.message(error):'Settings applied · '+p.rows+' rows read (unknown layers unchanged)'+(p.malformed?' · '+p.malformed+' malformed rows ignored':'')+'.');});
            }catch(e){finish(j,e.message||'Settings load failed.');}
        }
        function release(url){const timer=urls.get(url);if(timer!==undefined){o.clearTimeout(timer);urls.delete(url);w.URL.revokeObjectURL(url);}}
        async function save(){const c=context();if(!enabled||dead||job||!c||!c.idle){return;}if(urls.size>=4){status('Wait for prior downloads to be released.');return;}
            const format=el('settings-format').value;if(format!=='native'&&format!=='calibre'){return;}const j={context:c,phase:'save'};job=j;controls();status('Preparing settings download…');
            try{const text=await request(j,'GET',format);if(!current(j)){return;}const blob=new o.Blob([text],{type:'text/plain;charset=utf-8'}),url=w.URL.createObjectURL(blob);urls.set(url,o.setTimeout(function(){release(url);},60000));
                const link=o.document.createElement('a');link.href=url;link.download=format==='native'?'floe-layers.json':'floe-layers.layerprops';o.document.body.appendChild(link);try{link.click();}finally{o.document.body.removeChild(link);}finish(j,'Download requested. Check the browser download list.');
            }catch(e){finish(j,e.message||'Settings save failed.');}
        }
        el('settings-load').onclick=function(){choice=context();if(!job&&enabled&&!dead&&choice&&choice.idle){el('settings-file').value='';el('settings-file').click();}};
        el('settings-file').onchange=function(){const c=choice;choice=null;const files=el('settings-file').files,file=files&&files.length===1?files[0]:null;el('settings-file').value='';if(file){load(file,c);}};
        el('settings-save').onclick=save;el('settings-cancel').onclick=function(){const sent=job&&job.phase==='apply';cancel(sent?'Settings application may already have committed. Current view is authoritative.':'Settings cancelled.');};
        return {changed:changed,capabilities:function(v){enabled=v===true;changed();},stop:function(){dead=true;cancel();Array.from(urls.keys()).forEach(release);},resume:function(){dead=false;changed();}};
    }
    return {bind:bind};
}));
