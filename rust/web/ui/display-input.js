/* ES2017. One immutable CLI-selected PNG; no browser path or upload. */
(function(root){
    'use strict';
    const P=typeof module==='object'&&module.exports?require('./protocol.js'):root.FloeProtocol;
    const W=360,H=160,MAX=80*1024*1024;
    function metadata(m){
        if(m==null){return null;}P.pixels(m.width,m.height);
        if(!Number.isInteger(m.bytes)||m.bytes<45||m.bytes>MAX){throw Error('Invalid input PNG size');}
        return {width:m.width,height:m.height,bytes:m.bytes};
    }
    function bind(o){
        const el=o.el,canvas=el('display-input-canvas');
        let info=null,ready=false,task=null,report=null;
        function status(s){el('display-input-status').textContent=s;}
        function controls(){el('display-input-run').disabled=!ready||!info||!!task;el('display-input-cancel').disabled=!task;}
        function reset(){canvas.width=canvas.height=1;canvas.hidden=true;report=null;el('display-input-report').textContent='';el('display-input-observed').value='unverified';}
        function publish(){if(report){const value=el('display-input-observed').value;report.screen_observation=['visible','display_problem'].includes(value)?value:'unverified';el('display-input-report').textContent=JSON.stringify(report,null,2);}}
        function cancel(){if(task){const t=task;task=null;t.cancelled=true;if(t.xhr){t.xhr.abort();}if(t.decode){t.decode.cancel();}}controls();}
        function read(t){return new Promise(function(resolve,reject){
            const x=new o.XHR(),m=t.info;t.xhr=x;let done=false;
            function finish(error){if(done){return;}done=true;t.xhr=null;if(error){reject(error);}else{resolve(new Uint8Array(x.response));}}
            x.open('GET','/api/v1/display-test/input',true);x.responseType='arraybuffer';x.timeout=5000;x.setRequestHeader('X-Floe-CSRF',o.csrf());
            x.onprogress=function(e){if(e.loaded>m.bytes||e.lengthComputable&&e.total>m.bytes){finish(Error('Input PNG reply exceeds its declared size'));x.abort();}};
            x.onload=function(){
                if(t.cancelled){finish(Error('Input PNG cancelled'));return;}
                if(x.status!==200||x.getResponseHeader('Content-Type')!=='image/png'||!(x.response instanceof ArrayBuffer)||x.response.byteLength!==m.bytes){finish(Error('Input PNG is unavailable or invalid'));return;}
                finish();
            };
            x.onerror=x.ontimeout=function(){finish(Error('Input PNG read failed or timed out'));};
            x.onabort=function(){finish(Error('Input PNG cancelled'));};x.send(null);
        });}
        function decode(data,t){return new Promise(function(resolve,reject){
            if(t.cancelled||task!==t||!ready){reject(Error('Input PNG cancelled'));return;}
            const m=t.info;
            t.decode=o.decode({format:'png',width:m.width,height:m.height},data,function(draw,error){
                t.decode=null;if(t.cancelled||!draw){reject(error||Error('Input PNG cancelled'));return;}
                try{
                    canvas.width=W;canvas.height=H;
                    const ctx=canvas.getContext('2d');if(!ctx){throw Error('Canvas 2D is unavailable');}
                    ctx.save();try{ctx.clearRect(0,0,W,H);ctx.imageSmoothingEnabled=true;ctx.scale(W/m.width,H/m.height);draw(ctx);}finally{ctx.restore();}
                    const pixels=ctx.getImageData(0,0,W,H).data;if(pixels.length!==W*H*4){throw Error('Input PNG readback length mismatch');}
                    let nontransparent=0;for(let at=3;at<pixels.length;at+=4){if(pixels[at]!==0){nontransparent++;}}
                    const dpr=Number.isFinite(o.window.devicePixelRatio)&&o.window.devicePixelRatio>0?o.window.devicePixelRatio:1;
                    canvas.style.width=(W/dpr)+'px';canvas.style.height=(H/dpr)+'px';canvas.hidden=false;
                    resolve({version:1,bundle:o.bundle,source_pixels:[m.width,m.height],encoded_bytes:m.bytes,display_pixels:[W,H],nontransparent_pixels:nontransparent,
                        device_pixel_ratio:dpr,screen_observation:'unverified',desktop_acceptance:'unverified',scope:'Browser PNG decode and smoothed Canvas stretch; not GTK interpolation parity or remote-screen acceptance'});
                }catch(e){reject(e);}
            });t.decode.start();
        });}
        async function run(){
            if(!ready||!info||task){return;}reset();const t={cancelled:false,xhr:null,decode:null,info:info};task=t;controls();status('Reading the frozen input PNG…');
            try{const result=await decode(await read(t),t);if(!t.cancelled&&task===t&&ready){report=result;publish();status('PNG decoded. Inspect the image; this is not a pixel-parity or remote-screen pass.');}}
            catch(e){if(!t.cancelled&&task===t){reset();status(e.message||'Input PNG failed');}}
            finally{if(task===t){task=null;}controls();}
        }
        el('display-input-run').onclick=run;
        el('display-input-cancel').onclick=function(){cancel();reset();status('Input PNG cancelled; no automatic retry.');};
        el('display-input-observed').onchange=publish;controls();
        return {init:function(m){cancel();reset();info=metadata(m);ready=true;el('display-input').hidden=!info;if(info){status('CLI-selected PNG: '+info.width+' × '+info.height+' pixels, '+info.bytes+' bytes. Show explicitly; the server will not reread the file.');}controls();},
            close:function(){ready=false;cancel();reset();},run:run};
    }
    const api={metadata:metadata,bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeDisplayInput=api;}
}(typeof window==='object'?window:this));
