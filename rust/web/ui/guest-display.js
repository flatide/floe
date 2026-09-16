/* Display-only guest query projection. No world-coordinate arithmetic or
 * authority is inferred from pixels; native receipts are checked again. */
(function(root){
    'use strict';
    function size(pixels,canvas,viewport){
        if(!Array.isArray(pixels)||pixels.length!==2||!pixels.every(function(n){return Number.isInteger(n)&&n>0;})||
            !canvas||!viewport||![canvas.width,canvas.height,canvas.left,canvas.top,viewport.left,viewport.top].every(Number.isFinite)||
            canvas.width<=0||canvas.height<=0){throw Error('Guest canvas has no display area');}
        // The guest base canvas uses object-fit:contain / centered positioning.
        // Use its actual CSS image rectangle, not the requested native DPR.
        const scale=Math.min(canvas.width/pixels[0],canvas.height/pixels[1]);
        return {pixels:pixels.slice(),dpr:1/scale,
            left:canvas.left-viewport.left+(canvas.width-pixels[0]*scale)/2,
            top:canvas.top-viewport.top+(canvas.height-pixels[1]*scale)/2};
    }
    function context(o){
        const P=o.protocol,s=o.state,h=o.hello;
        // Coordinate-only measurement bypasses query capability in the common
        // module. Follow must have NO context, not merely query:false.
        if(!s||!h||!o.session||o.session.mode!=='explore'||h.mode!=='explore'||h.measure!==true||
            typeof h.query!=='boolean'||h.view_id!==s.view_id||h.connection_epoch!==s.connection_epoch){return null;}
        let dimensions;try{dimensions=size(s.pixels,o.canvasRect,o.viewportRect);}catch(_){return null;}
        let frame=null,origin=null;
        [o.foreground,o.margin].some(function(f){
            if(!f||!P.matches(f,s)){return false;}
            const p=f.purpose==='foreground'?[0,0]:P.placement(f,s);
            // An old foreground may still paint the overlap; that alone is
            // not authority for querying the newly exposed strip.
            if(!p||p[0]<0||p[1]<0||p[0]+s.pixels[0]>f.width||p[1]+s.pixels[1]>f.height){return false;}
            frame=f;origin=p;return true;
        });
        return {id:h.view_id,state:Object.assign({},s,{status:s.failure?'failed':s.rendering?'rendering':'idle',capabilities:{query:h.query}}),
            frame:frame,origin:origin,size:dimensions,rect:o.viewportRect,
            acked:!!frame&&!!o.acked&&o.acked[frame.purpose]===frame.frame_id,
            connected:!!o.connected,hidden:!!o.hidden,pending:!!o.pending};
    }
    const api={size:size,context:context};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeGuestDisplay=api;}
}(typeof window==='object'?window:this));
