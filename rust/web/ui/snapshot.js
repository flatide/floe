/* ES2017, owner-only visible-pixel snapshots. No source reads, geometry
 * rerender, server upload or clipboard read. One encoder + one retained PNG. */
(function(root) {
    'use strict';
    const MAX_BYTES=80*1024*1024;
    function compose(document,P,scene) {
        if(!scene||!Array.isArray(scene.pixels)||scene.pixels.length!==2||!Array.isArray(scene.layers)||!scene.layers.length||scene.layers.length>5) {
            throw new Error('No displayed canvas to capture.');
        }
        P.pixels(scene.pixels[0],scene.pixels[1]);
        scene.layers.forEach(function(l){
            if(!l||!l.canvas||!Array.isArray(l.offset)||l.offset.length!==2||!l.offset.every(function(v){return Number.isInteger(v)&&Math.abs(v)<=2147483647;})) {
                throw new Error('Invalid displayed canvas placement.');
            }
            P.pixels(l.canvas.width,l.canvas.height);
        });
        const canvas=document.createElement('canvas');canvas.width=scene.pixels[0];canvas.height=scene.pixels[1];
        try {
            const ctx=canvas.getContext('2d',{alpha:false});if(!ctx||typeof canvas.toBlob!=='function'){throw new Error('This browser cannot encode canvas PNGs.');}
            ctx.imageSmoothingEnabled=false;ctx.fillStyle='#000000';ctx.fillRect(0,0,canvas.width,canvas.height);
            scene.layers.forEach(function(l){ctx.drawImage(l.canvas,l.offset[0],l.offset[1]);});return canvas;
        }catch(e){canvas.width=canvas.height=1;throw e;}
    }
    function clipboard(window) {
        try {return window.isSecureContext===true && window.navigator && window.navigator.clipboard &&
            typeof window.navigator.clipboard.write==='function' && typeof window.ClipboardItem==='function' &&
            (typeof window.ClipboardItem.supports!=='function'||window.ClipboardItem.supports('image/png'));}catch(_){return false;}
    }
    function outcome(p) {return Promise.resolve(p).then(function(v){return {ok:true,value:v};},function(e){return {ok:false,error:e};});}
    function bind(o) {
        const el=function(id){return o.document.getElementById(id);};
        let enabled=false,stopped=false,active=null,encoding=null,retained=null,note='',timer=null;
        function allowed(){return enabled&&!stopped;}
        function busy(){return !!active||!!encoding;}
        function discard(){if(retained&&retained.url){o.window.URL.revokeObjectURL(retained.url);}retained=null;}
        function render(){
            el('snapshot-panel').hidden=!enabled;
            const ready=allowed()&&o.ready()&&!busy();
            el('snapshot-copy').disabled=!ready||!clipboard(o.window);el('snapshot-save').disabled=!ready;
            el('snapshot-retry').hidden=!retained;el('snapshot-retry').disabled=!allowed()||busy();
            el('snapshot-status').textContent=note||(!clipboard(o.window)?'Image clipboard unavailable. Use Save view PNG.':'Ctrl/Cmd+C on the canvas copies its visible pixels.');
        }
        function download(){
            if(!allowed()||!retained){return;}
            if(!retained.url){retained.url=o.window.URL.createObjectURL(retained.blob);}
            const a=o.document.createElement('a');a.href=retained.url;a.download=retained.name;a.target='_blank';a.rel='noopener noreferrer';a.hidden=true;
            o.document.body.appendChild(a);try{a.click();}finally{o.document.body.removeChild(a);}
            note='PNG download requested · '+retained.width+' × '+retained.height+' device pixels. Check your browser’s downloads.';render();
        }
        function request(kind){
            if(!['copy','save'].includes(kind)){throw new Error('Invalid snapshot action');}
            if(!allowed()||!o.ready()||busy()){return false;}
            if(kind==='copy'&&!clipboard(o.window)){note='Image clipboard unavailable. Use Save view PNG.';render();return true;}
            let canvas;try{canvas=compose(o.document,o.protocol,o.scene());}catch(e){note=e.message;render();return true;}
            discard();const t={canvas:canvas,cancelled:false,reject:null,width:canvas.width,height:canvas.height};active=encoding=t;
            note='Capturing the visible pixels…';render();
            // toBlob owns a bitmap snapshot. Cancelling its delivery does not
            // cancel the browser encoder: keep that single credit until callback.
            const encoded=new Promise(function(resolve,reject){
                t.reject=reject;
                try {canvas.toBlob(function(blob){
                    if(encoding===t){encoding=null;}canvas.width=canvas.height=1;
                    if(t.cancelled||!allowed()){reject(new Error('Snapshot delivery cancelled.'));}
                    else if(!blob||blob.type!=='image/png'||!Number.isInteger(blob.size)||blob.size<1||blob.size>MAX_BYTES){reject(new Error('PNG encoding failed or exceeded 80 MiB.'));}
                    else {retained={blob:blob,url:null,name:'floe-view-'+t.width+'x'+t.height+'.png',width:t.width,height:t.height};resolve(blob);}
                    render();
                },'image/png');}
                catch(e){if(encoding===t){encoding=null;}canvas.width=canvas.height=1;reject(e);}
            });
            let delivery;
            try {
                // Start the permission-sensitive call inside the original click,
                // not after async encoding has lost transient user activation.
                delivery=kind==='copy'?o.window.navigator.clipboard.write([new o.window.ClipboardItem({'image/png':encoded})]):encoded.then(function(){
                    if(t.cancelled||!allowed()){throw new Error('Snapshot delivery cancelled.');}download();
                });
            }catch(e){delivery=Promise.reject(e);}
            o.clearTimeout(timer);timer=o.setTimeout(function(){
                if(active===t&&!t.cancelled){note='The browser is still encoding or waiting for clipboard permission. Another capture will not start until it finishes.';render();}
            },15000);
            // A prompt refusal must not free the encoder early. Both branches
            // are observed, including platforms that reject ClipboardItem first.
            Promise.all([outcome(encoded),outcome(delivery)]).then(function(results){
                if(active!==t){return;}active=null;o.clearTimeout(timer);timer=null;
                if(!t.cancelled&&allowed()){
                    if(!results[0].ok){note=results[0].error.message||'PNG encoding failed.';}
                    else if(!results[1].ok){note='Clipboard or download was refused. Download captured PNG keeps the same frozen image.';}
                    else if(kind==='copy'){note='Copied '+t.width+' × '+t.height+' device pixels. The clipboard copy is not cleared when this session ends.';}
                }render();
            });return true;
        }
        el('snapshot-copy').onclick=function(){request('copy');};el('snapshot-save').onclick=function(){request('save');};
        el('snapshot-retry').onclick=function(){if(!busy()){try{download();}catch(e){note=e.message;render();}}};
        render();return {init:function(v){enabled=v===true;render();},changed:render,request:request,
            stop:function(){stopped=true;discard();o.clearTimeout(timer);timer=null;if(active){active.cancelled=true;if(active.reject){active.reject(new Error('Snapshot delivery cancelled.'));}}render();},
            resume:function(){stopped=false;render();}};
    }
    const api={compose:compose,clipboard:clipboard,bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeSnapshot=api;}
}(typeof window==='object'?window:this));
