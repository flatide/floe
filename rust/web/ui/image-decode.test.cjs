'use strict';
const assert=require('node:assert/strict'),D=require('./image-decode.js');
function payload(format){const b=new Uint8Array(format==='png'?33:16+8*4),v=new DataView(b.buffer);
    if(format==='png'){b.set([137,80,78,71,13,10,26,10]);v.setUint32(8,13);v.setUint32(12,0x49484452);v.setUint32(16,4);v.setUint32(20,2);}
    else{b.set(new TextEncoder().encode('FLOERAW1'));v.setUint32(8,4,true);v.setUint32(12,2,true);for(let i=16;i<b.length;i++)b[i]=i;}
    return b;}
function test(format){
    const images=[],urls=new Set(),timers=new Map(),calls=[],draws=[];
    const env={Image:class{constructor(){this.naturalWidth=4;this.naturalHeight=2;images.push(this);}},ImageData:class{constructor(data,w,h){Object.assign(this,{data,w,h});}},Blob:class{},
        URL:{createObjectURL(){urls.add('test');return 'test';},revokeObjectURL(u){urls.delete(u);}},setTimeout(fn,ms){assert.equal(ms,5000);timers.set(1,fn);return 1;},clearTimeout(id){timers.delete(id);}};
    const bytes=payload(format),task=D.create(env,{format,width:4,height:2},bytes,(draw,error)=>{calls.push({draw,error});if(draw){draw({putImageData(data){draws.push(data.data.slice());},drawImage(image){assert.equal(image.src,'test');draws.push(image);}});}});
    return {task,images,urls,timers,calls,draws,bytes};
}
const raw=test('raw');assert.equal(raw.calls.length,0);raw.task.start();raw.task.start();raw.task.cancel();assert.equal(raw.calls.length,1);assert.deepEqual([...raw.draws[0]],[...raw.bytes.slice(16)]);
const cancelled=test('png');cancelled.task.cancel();cancelled.task.start();assert.equal(cancelled.images.length,0);assert.equal(cancelled.calls.length,1);
for(const mode of ['ok','cancel','mismatch','error','timeout']){
    const t=test('png');t.task.start();t.task.start();assert.equal(t.images.length,1);const image=t.images[0],late=image.onload;
    if(mode==='ok'){image.onload();assert.equal(t.draws.length,1);}
    if(mode==='cancel'){t.task.cancel();}
    if(mode==='mismatch'){image.naturalWidth=8;image.onload();}
    if(mode==='error'){image.onerror();}
    if(mode==='timeout'){t.timers.get(1)();}
    late();t.task.cancel();assert.equal(t.calls.length,1);assert.equal(t.urls.size,0);assert.equal(t.timers.size,0);assert.equal(image.src,'');assert.equal(image.onload,null);
    assert.equal(!!t.calls[0].error,['mismatch','error','timeout'].includes(mode));
}
const invalid=test('raw');invalid.bytes[8]=5;invalid.task.start();assert(invalid.calls[0].error);assert.equal(invalid.draws.length,0);
console.log('WEB IMAGE DECODE: ALL OK (shared raw/PNG, dimensions, explicit start, one completion, cancel/timeout/error, URL cleanup)');
