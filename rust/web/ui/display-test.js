/* ES2017. On-demand synthetic display checks; no layout, upload or saved report. */
(function(root){
    'use strict';
    const W=360,H=160,MAX=16+W*H*4;
    function expected(x,y){
        const i=Math.floor((x-20)/85);
        return y>=30&&y<130&&i>=0&&i<4&&x-(20+i*85)<70?
            [[255,51,51,255],[51,255,51,255],[51,51,255,255],[255,255,51,255]][i]:[0,0,0,255];
    }
    function compare(data,width,height,pixel){
        if(!data||data.length!==width*height*4){throw Error('Pixel readback length mismatch');}
        let different=0;
        for(let y=0;y<height;y++){for(let x=0;x<width;x++){const c=pixel(x,y),at=(y*width+x)*4;if(c.some(function(n,k){return data[at+k]!==n;})){different++;}}}
        return {pixels:width*height,different_pixels:different};
    }
    function bind(o){
        const el=o.el,ids=['display-test-png','display-test-raw','display-test-base','display-test-overlay'];
        let opened=false,task=null,report=null;
        function controls(){el('display-test-run').disabled=!opened||!!task;el('display-test-cancel').disabled=!task;}
        function status(s){el('display-test-status').textContent=s;}
        function reset(){ids.forEach(function(id){const c=el(id);c.width=c.height=1;});el('display-test-results').hidden=true;el('display-test-report').textContent='';report=null;el('display-test-observed').value='unverified';}
        function publish(){if(report){const v=el('display-test-observed').value;report.screen_observation=['all_visible','display_problem'].includes(v)?v:'unverified';el('display-test-report').textContent=JSON.stringify(report,null,2);}}
        function cancel(){if(task){const t=task;task=null;t.cancelled=true;if(t.xhr){t.xhr.abort();}if(t.decode){t.decode.cancel();}}controls();}
        function read(format,t){
            return new Promise(function(resolve,reject){
                const x=new o.XHR();t.xhr=x;let done=false;
                function finish(error){if(done){return;}done=true;t.xhr=null;if(error){reject(error);}else{resolve(new Uint8Array(x.response));}}
                x.open('GET','/api/v1/display-test/'+format,true);x.responseType='arraybuffer';x.timeout=5000;x.setRequestHeader('X-Floe-CSRF',o.csrf());
                x.onprogress=function(e){if(e.loaded>MAX||e.lengthComputable&&e.total>MAX){finish(Error('Display fixture exceeds its fixed limit'));x.abort();}};
                x.onload=function(){
                    if(t.cancelled){finish(Error('Display test cancelled'));return;}
                    const mime=format==='raw'?'application/octet-stream':'image/png';
                    if(x.status!==200||x.getResponseHeader('Content-Type')!==mime||!(x.response instanceof ArrayBuffer)||x.response.byteLength>MAX){finish(Error('Display fixture is unavailable or invalid'));return;}
                    finish();
                };
                x.onerror=x.ontimeout=function(){finish(Error('Display fixture read failed or timed out'));};
                x.onabort=function(){finish(Error('Display test cancelled'));};
                x.send(null);
            });
        }
        function decode(format,data,t){
            return new Promise(function(resolve,reject){
                if(t.cancelled||task!==t||!opened){reject(Error('Display test cancelled'));return;}
                const c=el('display-test-'+format);c.width=W;c.height=H;
                t.decode=o.decode({format:format,width:W,height:H},data,function(draw,error){
                    t.decode=null;
                    if(t.cancelled||!draw){reject(error||Error('Display test cancelled'));return;}
                    try{const ctx=c.getContext('2d',{alpha:false});if(!ctx){throw Error('Canvas 2D is unavailable');}ctx.imageSmoothingEnabled=false;draw(ctx);
                        resolve(compare(ctx.getImageData(0,0,W,H).data,W,H,expected));
                    }catch(e){reject(e);}
                });t.decode.start();
            });
        }
        function compose(){
            const base=el('display-test-base'),overlay=el('display-test-overlay'),crop=o.document.createElement('canvas');
            base.width=W;base.height=H;overlay.width=320;overlay.height=128;crop.width=320;crop.height=128;
            try{
                const a=base.getContext('2d',{alpha:false}),b=overlay.getContext('2d'),c=crop.getContext('2d',{alpha:false});
                if(!a||!b||!c){throw Error('Canvas composition is unavailable');}
                a.imageSmoothingEnabled=c.imageSmoothingEnabled=false;a.drawImage(el('display-test-raw'),0,0);
                b.clearRect(0,0,320,128);b.fillStyle='#ffffff';b.fillRect(152,63,16,2);b.fillRect(159,56,2,16);
                c.drawImage(base,-20,-16);c.drawImage(overlay,0,0);
                return compare(c.getImageData(0,0,320,128).data,320,128,function(x,y){return (x>=152&&x<168&&y>=63&&y<65)||(x>=159&&x<161&&y>=56&&y<72)?[255,255,255,255]:expected(x+20,y+16);});
            }finally{crop.width=crop.height=1;}
        }
        function layout(){
            const dpr=Number.isFinite(o.window.devicePixelRatio)&&o.window.devicePixelRatio>0?o.window.devicePixelRatio:1;
            ids.forEach(function(id){const c=el(id);c.style.width=(c.width/dpr)+'px';c.style.height=(c.height/dpr)+'px';});
            el('display-test-base').style.left=(-20/dpr)+'px';el('display-test-base').style.top=(-16/dpr)+'px';
            el('display-test-stack').style.width=(320/dpr)+'px';el('display-test-stack').style.height=(128/dpr)+'px';return dpr;
        }
        async function run(){
            if(!opened||task){return;}reset();const t={cancelled:false,xhr:null,decode:null};task=t;controls();status('Checking fixed PNG, raw RGBA and cropped overlay…');
            try{
                const png=await decode('png',await read('png',t),t);if(t.cancelled){return;}
                const raw=await decode('raw',await read('raw',t),t);if(t.cancelled){return;}
                const composition=compose(),dpr=layout();
                report={version:1,bundle:o.bundle,fixture:'gtk-four-bars-v1',pixel_checks:{png:png,raw:raw,crop_overlay:composition},device_pixel_ratio:dpr,
                    desktop_acceptance:'unverified',screen_observation:'unverified',scope:'Canvas readback only; not WebSocket/native-renderer or compositor/ETX acceptance'};
                el('display-test-results').hidden=false;publish();
                status([png,raw,composition].every(function(r){return r.different_pixels===0;})?'Canvas pixels match. Visually inspect all three panels; the remote screen is not measured.':'Pixel differences found. Copy the report and describe which panels are wrong.');
            }catch(error){if(!t.cancelled&&task===t){reset();status(error.message||'Display test failed');}}
            finally{if(task===t){task=null;}controls();}
        }
        el('display-test-run').onclick=run;el('display-test-cancel').onclick=function(){cancel();reset();status('Display test cancelled; it will not restart automatically.');};
        el('display-test-observed').onchange=publish;controls();
        return {open:function(){opened=true;controls();},close:function(){opened=false;cancel();reset();status('Run explicitly; no layout or renderer is accessed.');},run:run};
    }
    const api={expected:expected,compare:compare,bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeDisplayTest=api;}
}(typeof window==='object'?window:this));
