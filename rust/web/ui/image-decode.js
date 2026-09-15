/* ES2017. Shared viewer/diagnostic image path. Caller owns frame credit/CAS. */
(function(root){
    'use strict';
    const P=typeof module==='object'&&module.exports?require('./protocol.js'):root.FloeProtocol;
    function create(o,h,data,callback){
        let started=false,done=false,image=null,url=null,timer=null;
        function finish(draw,error){
            if(done){return;}done=true;
            if(timer!==null){o.clearTimeout(timer);timer=null;}
            try{callback(draw,error||null);}
            finally{
                if(image){image.onload=null;image.onerror=null;image.src='';}
                if(url){o.URL.revokeObjectURL(url);url=null;}
            }
        }
        return {
            start:function(){
                if(started||done){return;}started=true;
                try{
                    P.imagePayload(h.format,h.width,h.height,data);
                    if(h.format==='raw'){
                        finish(function(ctx){
                            const rgba=new Uint8ClampedArray(data.buffer,data.byteOffset+16,h.width*h.height*4);
                            ctx.putImageData(new o.ImageData(rgba,h.width,h.height),0,0);
                        });
                    }else{
                        image=new o.Image();url=o.URL.createObjectURL(new o.Blob([data],{type:'image/png'}));
                        image.onload=function(){
                            if(image.naturalWidth!==h.width||image.naturalHeight!==h.height){finish(null,new Error('Decoded PNG dimensions mismatch'));return;}
                            finish(function(ctx){ctx.drawImage(image,0,0);});
                        };
                        image.onerror=function(){finish(null,new Error('PNG decode failed'));};
                        timer=o.setTimeout(function(){finish(null,new Error('PNG decode timeout'));},5000);
                        image.src=url;
                    }
                }catch(error){finish(null,error);}
            },
            cancel:function(){finish(null);}
        };
    }
    const api={create:create};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeImageDecode=api;}
}(typeof window==='object'?window:this));
