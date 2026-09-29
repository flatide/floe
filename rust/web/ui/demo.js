/* ES2017. Public IDs only. Starting a view is an explicit, non-replayed POST. */
(function(root){
    'use strict';
    function bind(o){
        const doc=o.document,status=doc.getElementById('demo-status'),list=doc.getElementById('demo-samples');
        const buttons=[];let busy=false;
        function request(method,path,body){return new Promise(function(resolve,reject){
            const x=new o.XHR();x.open(method,path,true);x.timeout=10000;
            if(body){x.setRequestHeader('Content-Type','application/json');}
            x.onprogress=function(e){if(e.loaded>16384){x.abort();reject(Error('Reply too large.'));}};
            x.onload=function(){try{if(x.status===429){throw Error('Demo is busy. Please try again later.');}
                if(x.status<200||x.status>=300||x.responseText.length>16384){throw Error('Demo unavailable. Please try again later.');}
                resolve(JSON.parse(x.responseText));}catch(e){reject(e);}};
            x.onerror=x.ontimeout=x.onabort=function(){reject(Error('Connection unavailable. No request was retried.'));};
            x.send(body?JSON.stringify(body):null);
        });}
        async function launch(id){if(busy){return;}busy=true;buttons.forEach(function(b){b.disabled=true;});status.textContent='Starting an independent view…';
            try{const v=await request('POST','/api/v1/demo/launches',{sample_id:id});
                if(!/^[0-9a-f]{64}$/.test(v.launch_id)||!/^[0-9a-f]{64}$/.test(v.bootstrap)||v.viewer_ready!==true){throw Error('Invalid demo response.');}
                // Bootstrap travels only in a same-origin fragment, never in HTTP queries/logs.
                o.location.assign('/server/'+v.launch_id+'#bootstrap='+v.bootstrap);
            }catch(e){status.textContent=e.message+' An interrupted launch expires after 30 seconds.';
                busy=false;buttons.forEach(function(b){b.disabled=false;});}
        }
        async function start(){try{if(o.location.protocol!=='https:'){throw Error('Open this demo using its HTTPS address.');}
            const v=await request('GET','/api/v1/demo/samples');
            if(!Array.isArray(v.samples)||!v.samples.length||v.samples.length>32||new Set(v.samples).size!==v.samples.length||
                v.samples.some(function(id){return typeof id!=='string'||!/^[A-Za-z0-9_-]{1,64}$/.test(id);})){throw Error('Invalid sample list.');}
            v.samples.forEach(function(id){const b=doc.createElement('button');b.type='button';b.textContent=id;b.onclick=function(){launch(id);};list.appendChild(b);buttons.push(b);});
            status.textContent='Select a sample to open the viewer.';
        }catch(e){status.textContent=e.message;}}
        return {start:start};
    }
    if(typeof module==='object'&&module.exports){module.exports={bind:bind};}
    else{bind({document:root.document,location:root.location,XHR:root.XMLHttpRequest}).start();}
})(typeof window==='undefined'?globalThis:window);
