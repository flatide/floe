/* ES2017. Independent diagnostics; same one-use local authentication, no view/WS. */
(function(root){
    'use strict';
    function bind(o){
        const win=o.window,doc=o.document,el=function(id){return doc.getElementById(id);};
        const bundle=doc.querySelector('meta[name="floe-bundle"]').content;
        const key='floe-display-session:'+o.location.origin,pending=new Set();
        let auth=null,stopped=false,unloaded=false,started=false;
        function status(s){if(!unloaded){el('display-page-status').textContent=s;}}
        function remove(){try{win.sessionStorage.removeItem(key);}catch(_){/* No persistent fallback. */}}
        function valid(a){return a&&a.protocol===1&&a.bundle===bundle&&typeof a.csrf==='string'&&/^[0-9a-f]{64}$/.test(a.csrf)&&typeof a.session_id==='string'&&/^[0-9a-f]{64}$/.test(a.session_id);}
        function request(method,path,body){
            return new Promise(function(resolve,reject){
                const x=new o.XHR();pending.add(x);let done=false;
                function finish(error,value){if(done){return;}done=true;pending.delete(x);if(error){reject(error);}else{resolve(value);}}
                x.open(method,path,true);x.timeout=8000;
                if(auth){x.setRequestHeader('X-Floe-CSRF',auth.csrf);}
                if(body!==undefined){x.setRequestHeader('Content-Type','application/json');}
                x.onprogress=function(e){if(e.loaded>65536||e.lengthComputable&&e.total>65536){finish(Error('Diagnostic session reply exceeds its limit'));x.abort();}};
                x.onload=function(){
                    if(x.status<200||x.status>=300){finish(Error('Diagnostic session unavailable (HTTP '+x.status+'). Start a new session; no request was replayed.'));return;}
                    try{if(x.responseText.length>65536){throw Error('Diagnostic session reply exceeds its limit');}finish(null,x.responseText?JSON.parse(x.responseText):null);}
                    catch(_){finish(Error('Invalid diagnostic session reply'));}
                };
                x.onerror=x.ontimeout=function(){finish(Error('Diagnostic session read failed or timed out; no request was replayed'));};
                x.onabort=function(){finish(Error('Diagnostic session request cancelled'));};
                x.send(body===undefined?null:JSON.stringify(body));
            });
        }
        const display=win.FloeDisplayTest.bind({el:el,document:doc,window:win,XHR:o.XHR,bundle:bundle,csrf:function(){return auth?auth.csrf:'';},
            decode:function(h,data,callback){return win.FloeImageDecode.create({Image:win.Image,ImageData:win.ImageData,Blob:win.Blob,URL:win.URL,setTimeout:win.setTimeout.bind(win),clearTimeout:win.clearTimeout.bind(win)},h,data,callback);}});
        function stop(){stopped=true;display.close();exit.stop();pending.forEach(function(x){x.abort();});}
        const exit=win.FloeSessionExit.bind({el:el,document:doc,confirm:async function(){
            if(stopped){return;}stop();remove();
            try{await request('DELETE','/api/v1/session');status('Diagnostic session closed.');}
            catch(_){status('Session exit was not confirmed. Close the isolated Firefox or use Ctrl+C in its terminal. No retry was sent.');}
            finally{auth=null;}
        }});
        async function start(){
            if(started||stopped){return;}started=true;
            const fragment=o.location.hash;
            try{
                if(fragment){o.history.replaceState(null,'',o.location.pathname);}
                if(typeof win.ImageData!=='function'||!win.URL||typeof win.URL.createObjectURL!=='function'||!doc.createElement('canvas').getContext('2d')){throw Error('Canvas 2D and image APIs are required. Use a supported Firefox.');}
                let storageMissing=false;
                if(/^#bootstrap=[0-9a-f]{64}$/.test(fragment)){
                    // A supplied new token wins over any stale tab state; never replay it.
                    remove();const result=await request('POST','/api/v1/session/exchange',{bootstrap:fragment.slice(11),protocol:1,bundle:bundle});
                    if(stopped){return;}if(!valid(result)){throw Error('Diagnostic session version or credentials are invalid');}auth=result;
                    try{win.sessionStorage.setItem(key,JSON.stringify(auth));}catch(_){storageMissing=true;}
                }else{
                    if(fragment){throw Error('Invalid private session link. Start a new displaytest session.');}
                    try{auth=JSON.parse(win.sessionStorage.getItem(key));}catch(_){auth=null;}
                    if(!valid(auth)){auth=null;throw Error('Launch floe2-web displaytest and open its private session link.');}
                }
                const caps=await request('GET','/api/v1/capabilities');
                if(stopped){return;}
                if(!caps||caps.protocol!==1||caps.bundle!==bundle||caps.display_only!==true||caps.render!==false||caps.catalog!==false){throw Error('This is not a matching standalone diagnostic session.');}
                display.open();exit.init();status('Ready. Run the display test explicitly.'+(storageMissing?' Tab storage is unavailable; reloading needs a new session.':''));
            }catch(error){if(!stopped){stop();remove();auth=null;status(error.message||'Diagnostic session failed');}}
        }
        win.addEventListener('pagehide',function(){unloaded=true;stop();auth=null;});
        win.addEventListener('pageshow',function(e){if(e.persisted){unloaded=false;status('Diagnostic page was restored in a stopped state. Reload to reconnect; no test or write was replayed.');}});
        return {start:start,stop:stop};
    }
    const api={bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeDisplayPage=api;api.bind({window:root,document:document,location:location,history:history,XHR:XMLHttpRequest}).start();}
}(typeof window==='object'?window:this));
