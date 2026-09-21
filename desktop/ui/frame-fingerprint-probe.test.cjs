'use strict';
const test=require('node:test'),assert=require('node:assert/strict'),vm=require('node:vm'),fs=require('node:fs');
const {webcrypto,createHash}=require('node:crypto');
const code='('+fs.readFileSync(__dirname+'/frame-fingerprint-probe.js','utf8')+')(comparison)';
function setup() {
    const bytes=new Uint8ClampedArray([1,2,3,255,4,5,6,255]);
    const f={width:2,height:1,dataset:{frameId:'f',renderRev:'1',bboxDbu:'[0,0,2,1]'},
        getContext:()=>({getImageData:()=>({data:bytes})})};
    const m={width:34,height:33,hidden:false,dataset:{frameId:'m',renderRev:'1',bboxDbu:'[-16,-16,18,17]'},getContext:()=>({getImageData:(...args)=>{
        assert.deepEqual(args,[16,16,2,1]);return {data:bytes};}})};
    const document={hidden:false,getElementById:id=>id==='canvas'?f:m};
    const ctx={document,devicePixelRatio:2,crypto:webcrypto,comparison:{bbox:[0,0,2,1],margin_bbox:[-16,-16,18,17],pixels:[2,1],offset:[16,16]}};
    return {f,m,ctx,bytes,run:()=>vm.runInNewContext(code,ctx)};
}
test('digest includes every unresampled RGBA byte of each counterpart',async()=>{
    const h=setup(),expected=createHash('sha256').update(h.bytes).digest('hex');
    assert.equal(await h.run(),'2 1 2 0 0 2 1 '+expected+' '+expected);
});
test('hidden or mismatched frame and revision changes while hashing reject the report',async()=>{
    for(const change of [h=>h.ctx.document.hidden=true,h=>h.m.hidden=true,h=>h.f.dataset.renderRev='other',
        h=>h.f.width=3,h=>h.f.dataset.bboxDbu='[0,0,3,1]',h=>h.ctx.comparison.offset=[100,0],
        h=>h.m.dataset.bboxDbu='[0,0,2,1]',h=>h.ctx.crypto={subtle:{digest:async(...args)=>{
            h.m.dataset.frameId='replacement';return webcrypto.subtle.digest(...args);}}}]) {
        const h=setup();change(h);await assert.rejects(h.run());
    }
});
test('comparison viewport dimensions follow actual DPR without overwriting it',()=>{
    const script='('+fs.readFileSync(__dirname+'/cross-viewport-probe.js','utf8')+')()';
    for(const dpr of [1,1.25,2]) {
        const v={style:{}};
        const r=vm.runInNewContext(script,{document:{getElementById:()=>v},devicePixelRatio:dpr});
        assert.deepEqual(Array.from(r),[800*dpr,600*dpr]);assert.equal(v.style.flex,'none');
        assert.equal(v.style.width,'800px');assert.equal(v.style.height,'600px');
    }
    for(const dpr of [0,NaN,8,1.333])assert.throws(()=>vm.runInNewContext(script,{document:{getElementById:()=>({style:{}})},devicePixelRatio:dpr}));
});
