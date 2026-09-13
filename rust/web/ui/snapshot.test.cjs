'use strict';
const assert=require('node:assert/strict'),S=require('./snapshot.js'),P=require('./protocol.js');
const flush=async()=>{for(let i=0;i<15;i++)await Promise.resolve();};
// A small independent nearest-pixel source-over canvas. This catches crop,
// stacking and transparent overlay errors without pretending to be browser PNG QA.
function harness() {
    const nodes=new Map(),encodes=[],downloads=[],copies=[],urls=new Map(),timers=new Map(),canvases=[];
    let next=0,ready=true,throwEncode=false,writeHook=null,selection=null;
    class Element {
        constructor(){this.children=[];this.hidden=false;this.disabled=false;this.style={};this.textContent='';}
        appendChild(c){this.children.push(c);return c;}removeChild(c){this.children.splice(this.children.indexOf(c),1);}
        click(){downloads.push({href:this.href,download:this.download,target:this.target,rel:this.rel});}
    }
    class Canvas {
        constructor(){this._width=1;this._height=1;this.reset();canvases.push(this);}
        reset(){this.data=new Uint8ClampedArray(this._width*this._height*4);}
        set width(v){this._width=v;this.reset();}get width(){return this._width;}
        set height(v){this._height=v;this.reset();}get height(){return this._height;}
        getContext(){const c=this;return {fillRect(){for(let i=3;i<c.data.length;i+=4)c.data[i]=255;},drawImage(src,x,y){
            for(let j=0;j<src.height;j++)for(let i=0;i<src.width;i++){
                const dx=x+i,dy=y+j;if(dx<0||dy<0||dx>=c.width||dy>=c.height)continue;
                const a=(j*src.width+i)*4,b=(dy*c.width+dx)*4,alpha=src.data[a+3]/255;
                for(let k=0;k<3;k++)c.data[b+k]=src.data[a+k]*alpha+c.data[b+k]*(1-alpha);c.data[b+3]=255;
            }
        }};}
        toBlob(fn,type){if(throwEncode)throw new Error('encoder failed');assert.equal(type,'image/png');encodes.push({fn,data:this.data.slice(),width:this.width,height:this.height});}
    }
    const el=id=>{if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
    const document={body:new Element(),getElementById:el,createElement:tag=>tag==='canvas'?new Canvas():new Element()};
    class Item {constructor(v){this.data=v;}}
    const window={isSecureContext:true,ClipboardItem:Item,navigator:{clipboard:{write(items){assert.equal(this,window.navigator.clipboard);
        copies.push(items);return writeHook?writeHook(items):Promise.all(items.map(i=>i.data['image/png']));}}},
        URL:{createObjectURL(b){const url='blob:'+ ++next;urls.set(url,b);return url;},revokeObjectURL:u=>urls.delete(u)},getSelection:()=>selection};
    function canvas(w,h,color){const c=new Canvas();c.width=w;c.height=h;for(let i=0;i<c.data.length;i+=4)c.data.set(color,i);return c;}
    const base=canvas(4,3,[10,20,30,255]),overlay=canvas(2,2,[200,100,50,128]);
    let scene={pixels:[3,2],layers:[{canvas:base,offset:[-1,-1]},{canvas:overlay,offset:[1,0]}]};
    const s=S.bind({document,window,protocol:P,ready:()=>ready,scene:()=>scene,
        setTimeout:(fn,ms)=>{timers.set(++next,{fn,ms});return next;},clearTimeout:id=>timers.delete(id)});
    return {s,el,base,overlay,window,document,encodes,downloads,copies,urls,timers,canvases,canvas,
        init:()=>s.init(true),get scene(){return scene;},set scene(v){scene=v;},set ready(v){ready=v;},set throwEncode(v){throwEncode=v;},set writeHook(f){writeHook=f;},
        finish:(v={type:'image/png',size:64})=>encodes.shift().fn(v)};
}
(async()=>{
    {
        const h=harness();const out=S.compose(h.document,P,h.scene);
        assert.deepEqual([...out.data],[10,20,30,255,105,60,40,255,105,60,40,255,10,20,30,255,105,60,40,255,105,60,40,255]);
        // Incoming margin strip, opaque old labeled foreground, then annotations.
        const margin=h.canvas(5,3,[0,80,0,255]),front=h.canvas(2,2,[80,0,0,255]),mark=h.canvas(1,1,[0,0,255,255]);
        const mix=S.compose(h.document,P,{pixels:[3,2],layers:[{canvas:margin,offset:[-2,-1]},{canvas:front,offset:[-1,0]},{canvas:mark,offset:[2,1]}]});
        assert.deepEqual([...mix.data],[80,0,0,255,0,80,0,255,0,80,0,255,80,0,0,255,0,80,0,255,0,0,255,255]);
        const centered=S.compose(h.document,P,{pixels:[3,2],layers:[{canvas:mark,offset:[1,0]}]});
        assert.deepEqual([...centered.data.slice(0,12)],[0,0,0,255,0,0,255,255,0,0,0,255]);
        for(const scene of [null,{pixels:[8193,1],layers:h.scene.layers},{pixels:[8192,8192],layers:h.scene.layers},
            {pixels:[1,1],layers:[]},{pixels:[1,1],layers:[{canvas:h.base,offset:[.5,0]}]},
            {pixels:[1,1],layers:new Array(6).fill(h.scene.layers[0])}])assert.throws(()=>S.compose(h.document,P,scene));
        h.s.stop();
    }
    {
        const h=harness();assert(h.el('snapshot-panel').hidden);assert(!h.s.request('save'));h.init();assert(!h.el('snapshot-copy').disabled);
        h.s.request('copy');assert.equal(h.copies.length,1,'clipboard write lost the original synchronous gesture');assert.equal(h.encodes.length,1);
        assert(!h.s.request('save'),'two encoders started');h.base.data.fill(0);assert.equal(h.encodes[0].data[0],10,'later display mutation changed the captured bitmap');
        h.finish();await flush();assert.match(h.el('snapshot-status').textContent,/Copied 3 × 2/);assert(!h.el('snapshot-retry').hidden);
        h.el('snapshot-retry').onclick();assert.equal(h.downloads.length,1);assert.equal(h.urls.size,1);assert.equal(h.downloads[0].download,'floe-view-3x2.png');
        h.el('snapshot-retry').onclick();assert.equal(h.urls.size,1);assert.equal(h.document.body.children.length,0);
        h.s.request('save');assert.equal(h.urls.size,0,'old object URL was retained');h.finish();await flush();assert.equal(h.urls.size,1);
        h.s.stop();assert.equal(h.urls.size,0);assert.equal(h.timers.size,0);assert(h.canvases.slice(2).every(c=>c.width===1&&c.height===1));
    }
    {
        const h=harness();h.window.navigator.clipboard.write=undefined;h.init();assert(h.el('snapshot-copy').disabled);assert(!h.el('snapshot-save').disabled);
        h.s.request('copy');assert.equal(h.encodes.length,0);assert.match(h.el('snapshot-status').textContent,/Save view PNG/);
        h.s.request('save');h.finish();await flush();assert.equal(h.downloads.length,1);assert.equal(h.copies.length,0);h.s.stop();
    }
    {
        const h=harness();h.window.isSecureContext=false;assert(!S.clipboard(h.window));h.window.isSecureContext=true;
        h.window.ClipboardItem.supports=()=>false;assert(!S.clipboard(h.window));h.s.stop();
    }
    {
        const h=harness();h.init();h.writeHook=()=>Promise.reject(new Error('permission refused'));
        h.s.request('copy');await flush();assert(!h.s.request('copy'),'early permission denial released a still-running encoder');
        h.finish();await flush();assert.match(h.el('snapshot-status').textContent,/refused/);assert.equal(h.downloads.length,0,'denial silently downloaded a file');
        h.ready=false;h.s.changed();h.el('snapshot-retry').onclick();assert.equal(h.downloads.length,1,'frozen fallback needed a new render');h.s.stop();
    }
    for(const value of [null,{type:'image/jpeg',size:64},{type:'image/png',size:0},{type:'image/png',size:80*1024*1024+1}]){
        const h=harness();h.init();h.s.request('copy');h.finish(value);await flush();assert.match(h.el('snapshot-status').textContent,/failed|80 MiB/);
        assert.equal(h.downloads.length,0);assert(h.el('snapshot-retry').hidden);assert(!h.el('snapshot-save').disabled);h.s.stop();
    }
    {
        const h=harness();h.init();h.throwEncode=true;h.s.request('copy');await flush();assert.match(h.el('snapshot-status').textContent,/encoder failed/);
        assert(!h.el('snapshot-save').disabled);h.s.stop();
    }
    {
        const h=harness();h.init();h.s.request('save');h.s.stop();await flush();h.s.resume();assert(!h.s.request('save'),'page restore doubled an uncancellable encoder');
        h.finish();await flush();assert.equal(h.downloads.length,0);assert.equal(h.urls.size,0);assert(!h.el('snapshot-save').disabled);
        h.s.request('save');h.finish();await flush();assert.equal(h.downloads.length,1);h.s.stop();
    }
    {
        const h=harness();h.init();let complete;h.writeHook=items=>new Promise(resolve=>{complete=()=>resolve(items);});
        h.s.request('copy');h.finish();await flush();for(const t of h.timers.values())t.fn();
        assert.match(h.el('snapshot-status').textContent,/still encoding|clipboard permission/);assert(!h.s.request('copy'));
        h.s.stop();complete();await flush();assert.equal(h.urls.size,0);assert(!h.el('snapshot-status').textContent.includes('Copied'));h.s.stop();
    }
    console.log('WEB SNAPSHOT: ALL OK (pixel crop/stack/alpha, synchronous clipboard gesture, one encoder, denial fallback, limits, stale callbacks, URL/canvas cleanup)');
})().catch(e=>{console.error(e);process.exitCode=1;});
