/* ES2017: opt-in browser-local display diagnostics. Two bounded bitmaps,
 * one uncancellable PNG encoder and one retry blob; no HTTP or storage. */
(function(root) {
    'use strict';
    const MAX_BYTES=80*1024*1024;
    function bind(o) {
        const el=id=>o.document.getElementById(id);
        let available=false,enabled=false,stopped=false,epoch=0,serial=0,raf=null;
        let received=null,display=null,encoding=null,encoded=null,note='';
        function allowed(){return available&&enabled&&!stopped;}
        function release(entry){if(entry){entry.canvas.width=entry.canvas.height=1;}}
        function discardEncoded(){if(encoded&&encoded.url){o.window.URL.revokeObjectURL(encoded.url);}encoded=null;}
        function render(){
            el('dump-panel').hidden=!available;el('dump-enabled').disabled=!available||stopped;el('dump-enabled').checked=enabled;
            el('dump-received').disabled=!allowed()||!received||!!encoding;
            el('dump-display').disabled=!allowed()||!display||!!encoding;
            el('dump-refresh').disabled=el('dump-clear').disabled=!allowed();
            el('dump-received-info').textContent=received?received.info:'No received frame retained.';
            el('dump-display-info').textContent=display?display.info:'No composed display retained.';
            el('dump-retry').hidden=!encoded;el('dump-retry').disabled=!allowed()||!!encoding;
            el('dump-retry').textContent=encoded?'Download '+encoded.name+' again':'Download encoded PNG again';
            el('dump-status').textContent=encoding?(encoding.epoch===epoch?'Encoding retained pixels; one encoder at a time.':'Waiting for cancelled PNG encoding to finish.'):note||(enabled?'Collecting recent pixels only. PNG encoding starts on download.':'Collection is off.');
        }
        function reset(){
            ++epoch;if(raf!==null){o.window.cancelAnimationFrame(raf);raf=null;}
            release(received);release(display);received=display=null;discardEncoded();
            // Cancelling delivery cannot cancel browser toBlob work. Keep its
            // single encoder credit through reset/disable/resume until callback.
            note=encoding?'Images cleared; waiting for the cancelled encoder.':'Images cleared.';render();
        }
        function fail(){note='Could not retain these pixels. Previous capture of this kind was cleared.';render();}
        function receivedFrame(h,canvas){
            if(!allowed()||o.document.hidden){return;}
            release(received);received=null;
            try {
                o.protocol.counter(h.generation);
                if(!['foreground','margin'].includes(h.purpose)||!['raw','png'].includes(h.format)||canvas.width!==h.width||canvas.height!==h.height){throw new Error('Invalid received frame');}
                const copy=o.compose(o.document,o.protocol,{pixels:[h.width,h.height],layers:[{canvas:canvas,offset:[0,0]}]});
                const n=++serial;
                received={canvas:copy,serial:n,info:'Received #'+n+' · '+h.purpose+' · '+h.format+' · generation '+h.generation+' · '+h.width+' × '+h.height+' px'+(h.complete?'':' · incomplete')};
                note='';render();
            }catch(_){fail();}
        }
        function captureDisplay(){
            if(!allowed()||o.document.hidden){return;}
            release(display);display=null;
            try {
                const scene=o.scene();
                if(!scene){note='No displayed composition is available.';render();return;}
                const copy=o.compose(o.document,o.protocol,scene),n=++serial;
                display={canvas:copy,serial:n,info:'Display #'+n+' · '+copy.width+' × '+copy.height+' px · '+scene.layers.length+' canvas layers · latest received '+(received?'#'+received.serial:'none')};
                note='';render();
            }catch(_){fail();}
        }
        function changed(){
            if(!allowed()||o.document.hidden||raf!==null){return;}
            raf=o.window.requestAnimationFrame(function(){raf=null;captureDisplay();});
        }
        function download(){
            if(!allowed()||!encoded){return;}
            if(!encoded.url){encoded.url=o.window.URL.createObjectURL(encoded.blob);}
            const a=o.document.createElement('a');a.href=encoded.url;a.download=encoded.name;a.target='_blank';a.rel='noopener noreferrer';a.hidden=true;
            o.document.body.appendChild(a);try{a.click();}finally{o.document.body.removeChild(a);}
            note='PNG download requested: '+encoded.name+'. Check browser downloads; this is not a remote-screen screenshot.';render();
        }
        function request(kind){
            if(!['received','display'].includes(kind)){return false;}
            const entry=kind==='received'?received:display;
            if(!allowed()||!entry||encoding){return false;}
            discardEncoded();
            const t={epoch:epoch,name:'floe-dump-'+kind+'-'+entry.serial+'-'+entry.canvas.width+'x'+entry.canvas.height+'.png'};
            encoding=t;note='Encoding retained '+kind+' pixels…';render();
            try {
                entry.canvas.toBlob(function(blob){
                    if(encoding!==t){return;}encoding=null;
                    if(t.epoch!==epoch||!allowed()){note=allowed()?'Cancelled encoding finished; new downloads are available.':'';render();return;}
                    if(!blob||blob.type!=='image/png'||!Number.isInteger(blob.size)||blob.size<1||blob.size>MAX_BYTES){note='PNG encoding failed or exceeded 80 MiB.';render();return;}
                    encoded={blob:blob,url:null,name:t.name};
                    try{download();}catch(_){note='Download was refused. The encoded PNG is retained for an explicit retry.';render();}
                },'image/png');
            }catch(_){if(encoding===t){encoding=null;}note='This browser could not encode PNG.';render();}
            return true;
        }
        el('dump-enabled').onchange=function(){enabled=available&&!stopped&&el('dump-enabled').checked;reset();note='';render();if(enabled){changed();}};
        el('dump-received').onclick=()=>request('received');el('dump-display').onclick=()=>request('display');
        el('dump-refresh').onclick=function(){if(raf!==null){o.window.cancelAnimationFrame(raf);raf=null;}captureDisplay();};
        el('dump-clear').onclick=reset;
        el('dump-retry').onclick=function(){if(encoding){return;}try{download();}catch(_){note='Download refused; pixels remain in this tab.';render();}};
        render();return {received:receivedFrame,changed:changed,reset:reset,request:request,
            init:function(v,start){available=v===true;enabled=available&&start===true;note='';render();if(enabled){changed();}},
            stop:function(){stopped=true;enabled=false;reset();},resume:function(){stopped=false;note='Collection is off after page restore; enable it explicitly.';render();}};
    }
    const api={bind:bind};if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeDisplayDump=api;}
}(typeof window==='object'?window:this));
