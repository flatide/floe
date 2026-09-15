/* Owner-only CLI proposal consumer. No paths, indexing or fresh-seq retries. */
(function(root){'use strict';
    function bind(env){
        const el=env.el;let enabled=false,paused=true,revision=null,pending=null,prepared='',attempt=null,busy=false;
        let polling=null,timer=null,autoTimer=null,error='',readError='',preparing=false;
        const retired=[];
        function paint(){
            el('launch-panel').hidden=!pending&&!attempt;
            el('launch-status').textContent=error||readError||(attempt?'Request outcome needs confirmation. Check the same request; do not launch it again.':
                pending&&pending.phase==='preparing'?'CLI request queued · reading source headers…':
                pending&&pending.phase==='failed'?'CLI preparation failed: '+pending.error:
                pending&&pending.confirm_levels?'Choose the requested jobdeck levels, then Open requested layout.':
                pending?'CLI request ready · waiting for pending view inputs.':'');
            el('launch-open').hidden=!pending||pending.phase!=='ready'||!pending.request||!!attempt||prepared!==pending.id;
            el('launch-open').disabled=busy||!env.ready();
            el('launch-check').hidden=!attempt;el('launch-check').disabled=busy;
            el('launch-dismiss').disabled=busy||!!attempt;
        }
        function save(value){env.savePending(value===null?null:JSON.stringify(value));attempt=value;}
        async function finish(result){
            const id=attempt?attempt.id:pending&&pending.id;
            if(id){retired.push(id);if(retired.length>32){retired.shift();}}
            save(null);if(pending&&pending.id===id){pending=null;}if(prepared===id){prepared='';}
            error='';paint();env.changed();
            if(result.phase==='submitted'){await env.completed();}
        }
        async function recover(){
            if(!attempt){return false;}
            const status=await env.http('GET','/api/v1/launch/'+attempt.id,undefined,true);
            if(status&&status.receipt){await finish(status.receipt);return true;}
            if(!status){throw new Error('Launch receipt expired. Check operation history; no request was replayed.');}
            if(status.phase==='dismissed'){await finish({phase:'dismissed'});return true;}
            return false;
        }
        async function send(id,input){
            save({id:id,input:input}); // Persist BEFORE the mutation, also on reload/BFCache.
            const result=await env.http('POST','/api/v1/launch/'+id,input);
            await finish(result);
        }
        async function perform(action){
            if(busy||paused||!enabled){return;}busy=true;error='';paint();
            try{
                if(action==='retry'){
                    if(!await recover()){
                        const original=attempt;
                        // Explicit button only; never allocate another operation seq.
                        await send(original.id,original.input);
                    }
                }else if(action==='dismiss'){
                    if(attempt){throw new Error('Check the original request before dismissing; its outcome is unknown.');}
                    const id=attempt?attempt.id:pending&&pending.id;
                    if(id){await send(id,{action:'dismiss'});}
                }else if(pending&&pending.phase==='ready'){
                    const item=pending;
                    if(!item.request){env.present();await send(item.id,{action:'present'});}
                    else{
                        if(!env.ready()){return;}
                        await env.open(item, function(input){return send(item.id,input);});
                    }
                }
            }catch(e){
                error=e.message||String(e);
                if(attempt){try{await recover();}catch(readError){error=readError.message||String(readError);}}
            }finally{busy=false;paint();laterAuto();}
        }
        async function laterAuto(){
            env.clearTimeout(autoTimer);autoTimer=null;
            if(paused||busy||preparing||attempt||error||!pending||pending.phase!=='ready'){return;}
            if(pending.request&&!env.ready()){autoTimer=env.setTimeout(laterAuto,100);return;}
            const item=pending;
            if(item.id!==prepared){
                preparing=true;
                try{
                    if(item.request){await env.prepare(item);}
                    if(paused||!pending||pending.id!==item.id){return;}
                    prepared=item.id;env.changed();
                }catch(e){error=e.message||String(e);paint();return;}
                finally{preparing=false;}
            }
            if(!item.confirm_levels){perform('open');}
            paint();
        }
        async function accept(value){
            env.protocol.counter(value.revision,true);
            if(revision!==null&&env.protocol.compare(value.revision,revision)<0){return;}
            revision=value.revision;readError='';
            pending=value.pending&&!retired.includes(value.pending.id)?value.pending:null;
            paint();env.changed();laterAuto();
        }
        async function poll(){
            if(paused||!enabled||polling){return;}
            const token={cancelled:false};polling=token;
            try{
                const value=await env.http('GET',revision===null?'/api/v1/launch':'/api/v1/launch/poll/'+revision,undefined,false,token);
                if(!paused&&!token.cancelled){await accept(value);}
            }catch(e){if(!paused&&!token.cancelled){readError=e.message||String(e);if(e.status===401){stop();}paint();}}
            finally{if(polling===token){polling=null;}if(!paused&&enabled){timer=env.setTimeout(poll,readError?1000:0);}}
        }
        function stop(){paused=true;env.clearTimeout(timer);env.clearTimeout(autoTimer);if(polling){polling.cancelled=true;if(polling.abort){polling.abort();}}}
        async function resume(){
            if(!enabled){return;}paused=false;
            if(attempt){try{await recover();}catch(e){error=e.message;}}
            paint();poll();
        }
        async function init(supported){
            enabled=!!supported;
            if(enabled){
                const saved=env.loadPending();
                if(saved){
                    try{const a=JSON.parse(saved);if(saved.length>32768||!a||!/^[0-9a-f]{64}$/.test(a.id)||!a.input||!['open','present','dismiss'].includes(a.input.action)){throw new Error('Invalid saved launch receipt');}attempt=a;}
                    catch(e){error='Saved launch request is invalid. Check operation history before launching again.';enabled=false;el('launch-panel').hidden=false;el('launch-status').textContent=error;return;}
                }
                await resume();
            }
        }
        el('launch-open').onclick=function(){perform('open');};
        el('launch-check').onclick=function(){perform('retry');};
        el('launch-dismiss').onclick=function(){perform('dismiss');};
        return {init:init,stop:stop,resume:resume,changed:paint,blocked:function(){return !!prepared||!!attempt;}};
    }
    root.FloeLauncher={bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=root.FloeLauncher;}
}(typeof window==='object'?window:globalThis));
