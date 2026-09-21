'use strict';
const test = require('node:test'), assert = require('node:assert/strict');
const fs = require('node:fs'), vm = require('node:vm');
const script = '(' + fs.readFileSync(__dirname + '/frame-parity-probe.js', 'utf8') + ')()';
function setup() {
    const a = new Uint8ClampedArray(4*3*4), b = new Uint8ClampedArray(a.length);
    for(let i=3;i<a.length;i+=4){a[i]=b[i]=255;}
    let reads=0;
    const f={width:4,height:3,hidden:true,dataset:{frameId:'f',renderRev:'1',bboxDbu:'[0,0,4,3]'},
        getContext:()=>({getImageData:(x,y,w,h)=>{reads++;assert.deepEqual([x,y,w,h],[0,0,4,3]);return {data:a};}})};
    const m={width:36,height:35,hidden:false,dataset:{frameId:'m',renderRev:'1',bboxDbu:'[-16,-16,20,19]'},
        getContext:()=>({getImageData:(x,y,w,h)=>{reads++;assert.deepEqual([x,y,w,h],[16,16,4,3]);return {data:b};}})};
    const document={hidden:false,getElementById:id=>id==='canvas'?f:m};
    return {a,b,f,m,document,reads:()=>reads,run:()=>JSON.parse(JSON.stringify(vm.runInNewContext(script,{document})))};
}
test('aligned original Canvas and margin crop compare every RGBA byte, without resampling',()=>{
    const h=setup();
    assert.equal(h.run().changed_pixels,0);assert.equal(h.run().compared_pixels,12);
    h.a.set([1,2,3,255],0);h.b.set([255,0,0,255],4);h.b[11]=254;
    const r=h.run();
    assert.equal(r.changed_pixels,3);assert.equal(r.foreground_only,1);assert.equal(r.margin_only,1);
    assert.equal(r.max_channel_delta,255);assert.deepEqual(r.bounds,[0,0,2,0]);assert.equal(r.examples.length,3);
});
test('hidden, mismatched, unaligned, incomplete and oversized inputs fail before any readback',()=>{
    for(const alter of [h=>h.document.hidden=true,h=>h.m.hidden=true,h=>delete h.f.dataset.frameId,
        h=>{delete h.f.dataset.renderRev;delete h.m.dataset.renderRev;},h=>h.f.dataset.bboxDbu='[0,0,4,3,5]',
        h=>h.m.dataset.renderRev='2',h=>h.m.dataset.bboxDbu='[-15,-16,21,19]',
        h=>h.m.dataset.bboxDbu='[-16,-16,21,19]',h=>h.m.width=16,h=>h.f.width=20000000,
        h=>h.f.dataset.bboxDbu='[0,0,0,3]',h=>h.f.dataset.bboxDbu='[0,0,"NaN",3]']) {
        const h=setup();alter(h);assert.throws(h.run);assert.equal(h.reads(),0);
    }
});
test('all pixels count even when diagnostic examples reach their fixed cap',()=>{
    const h=setup();for(let i=0;i<h.b.length;i+=4){h.b[i]=10;}
    const r=h.run();assert.equal(r.changed_pixels,12);assert.equal(r.examples.length,8);
});
