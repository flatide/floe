'use strict';
// Shared real decoder + deterministic Image/XHR/Canvas doubles, not browser QA.
const assert=require('node:assert/strict'),Input=require('./display-input.js'),Decode=require('./image-decode.js');
const flush=async()=>{for(let i=0;i<8;i++){await Promise.resolve();}};
const bytes=new Uint8Array(57);bytes.set([137,80,78,71,13,10,26,10]);
const view=new DataView(bytes.buffer);view.setUint32(8,13);view.setUint32(12,0x49484452);view.setUint32(16,4);view.setUint32(20,2);
const meta={width:4,height:2,bytes:57};
function harness(){
    const nodes=new Map(),requests=[],images=[],urls=new Set(),timers=new Map(),calls=[];
    const pixels=new Uint8ClampedArray(360*160*4);for(let i=3;i<pixels.length;i+=4){pixels[i]=i%8===3?255:0;}
    const context={save(){calls.push('save');},restore(){calls.push('restore');},clearRect(...a){calls.push(['clear',...a]);},scale(...a){calls.push(['scale',...a]);},drawImage(i,x,y){assert.ok(i.src);calls.push(['draw',x,y]);},getImageData(){return {data:pixels};}};
    function el(id){if(!nodes.has(id)){nodes.set(id,{style:{},hidden:true,value:'',textContent:'',getContext(){return context;}});}return nodes.get(id);}
    class XHR{
        constructor(){requests.push(this);this.headers={};this.status=200;this.mime='image/png';this.response=bytes.slice().buffer;}
        open(m,p){this.method=m;this.path=p;}setRequestHeader(k,v){this.headers[k]=v;}getResponseHeader(){return this.mime;}
        send(v){assert.equal(v,null);}abort(){this.aborted=true;this.onabort();}answer(){this.onload();}
    }
    const env={Image:class{constructor(){this.naturalWidth=4;this.naturalHeight=2;images.push(this);}},ImageData:class{},Blob:class{},
        URL:{createObjectURL(){const key='blob:'+images.length;urls.add(key);return key;},revokeObjectURL(key){urls.delete(key);}},setTimeout(fn){timers.set(1,fn);return 1;},clearTimeout(k){timers.delete(k);}};
    const input=Input.bind({el,window:{devicePixelRatio:2},XHR,bundle:'bundle',csrf:()=> 'csrf',decode:(h,data,cb)=>Decode.create(env,h,data,cb)});
    return {input,el,context,requests,images,urls,timers,calls};
}
(async()=>{
    assert.equal(Input.metadata(null),null);
    for(const m of [{...meta,width:0},{...meta,width:8193},{...meta,width:8192,height:8192},{...meta,bytes:80*1024*1024+1},{...meta,bytes:44},{...meta,width:'4'}]){assert.throws(()=>Input.metadata(m));}
    const h=harness();h.input.init(meta);assert.equal(h.requests.length,0);assert.equal(h.el('display-input').hidden,false);
    const p=h.input.run();h.input.run();assert.equal(h.requests.length,1);const r=h.requests[0];
    assert.equal(r.path,'/api/v1/display-test/input');assert.equal(r.method,'GET');assert.equal(r.headers['X-Floe-CSRF'],'csrf');assert.equal(r.timeout,5000);
    r.answer();await flush();assert.equal(h.images.length,1);h.images[0].onload();await p;
    assert.deepEqual(h.calls,['save',['clear',0,0,360,160],['scale',90,80],['draw',0,0],'restore']);
    assert.equal(h.context.imageSmoothingEnabled,true);assert.equal(h.urls.size,0);assert.equal(h.timers.size,0);
    assert.equal(h.el('display-input-canvas').style.width,'180px');assert.equal(h.el('display-input-canvas').style.height,'80px');
    const report=JSON.parse(h.el('display-input-report').textContent);assert.equal(report.nontransparent_pixels,28800);assert.equal(report.desktop_acceptance,'unverified');assert.equal(report.screen_observation,'unverified');
    h.el('display-input-observed').value='visible';h.el('display-input-observed').onchange();assert.equal(JSON.parse(h.el('display-input-report').textContent).screen_observation,'visible');
    h.input.close();assert.equal(h.el('display-input-report').textContent,'');assert.equal(h.el('display-input-canvas').width,1);assert.equal(h.requests.length,1);
    for(const stage of ['read','between','decode']){
        const c=harness();c.input.init(meta);const work=c.input.run();
        if(stage!=='read'){c.requests[0].answer();}if(stage==='decode'){await flush();}
        c.input.close();await work;assert.equal(c.urls.size,0);assert.equal(c.timers.size,0);assert.equal(c.el('display-input-report').textContent,'');
        assert.equal(c.calls.length,0);assert.equal(c.el('display-input-run').disabled,true);
        c.requests[0].answer();c.requests[0].onprogress({loaded:1000});await flush();assert.equal(c.calls.length,0);
        c.input.init(meta);assert.equal(c.requests.length,1);
    }
    for(const failure of ['status','mime','size','progress','signature','dimensions','decode','timeout']){
        const c=harness();c.input.init(meta);const work=c.input.run(),x=c.requests[0];
        if(failure==='status'){x.status=401;}if(failure==='mime'){x.mime='text/html';}if(failure==='size'){x.response=new ArrayBuffer(56);}
        if(failure==='signature'||failure==='dimensions'){const a=new Uint8Array(x.response);a[failure==='signature'?0:19]=0;}
        if(failure==='progress'){x.onprogress({loaded:58});}else{x.answer();}
        await flush();if(failure==='decode'){c.images[0].onerror();}if(failure==='timeout'){c.timers.get(1)();}
        await work;assert.equal(c.el('display-input-canvas').hidden,true,failure);assert.equal(c.urls.size,0);assert.equal(c.el('display-input-report').textContent,'');
        assert.equal(c.requests.length,1);
    }
    const none=harness();none.input.init(null);await none.input.run();assert.equal(none.requests.length,0);assert.equal(none.el('display-input').hidden,true);
    console.log('DISPLAY INPUT: ALL OK (shared decoder, stretch/alpha/readback plumbing, bounded auth GET, no autorun, cancellation/errors; no browser parity claim)');
})().catch(e=>{console.error(e);process.exitCode=1;});
