'use strict';
// Independent pixel canvas: tests frozen received vs crop/overlay composition,
// not browser PNG encoding or OS/ETX display acceptance.
const assert=require('node:assert/strict'),D=require('./display-dump.js'),S=require('./snapshot.js'),P=require('./protocol.js');
function harness(){
    const nodes=new Map(),canvases=[],encodes=[],downloads=[],urls=new Set(),raf=new Map();let next=0,refuse=false,throwEncode=false;
    class Element {
        constructor(){this.children=[];this.checked=false;this.hidden=false;this.disabled=false;this.textContent='';}
        appendChild(c){this.children.push(c);}removeChild(c){this.children.splice(this.children.indexOf(c),1);}
        click(){if(refuse)throw new Error('refused');downloads.push(this.download);}
    }
    class Canvas {
        constructor(){this._width=this._height=1;this.reset();canvases.push(this);}
        reset(){this.data=new Uint8ClampedArray(this._width*this._height*4);}
        set width(n){this._width=n;this.reset();}get width(){return this._width;}
        set height(n){this._height=n;this.reset();}get height(){return this._height;}
        getContext(){const c=this;return {fillRect(){for(let i=3;i<c.data.length;i+=4)c.data[i]=255;},drawImage(src,x,y){
            for(let j=0;j<src.height;j++)for(let i=0;i<src.width;i++){
                const dx=x+i,dy=y+j;if(dx<0||dy<0||dx>=c.width||dy>=c.height)continue;
                const a=(j*src.width+i)*4,b=(dy*c.width+dx)*4,alpha=src.data[a+3]/255;
                for(let k=0;k<3;k++)c.data[b+k]=src.data[a+k]*alpha+c.data[b+k]*(1-alpha);c.data[b+3]=255;
            }
        }};}
        toBlob(fn,type){if(throwEncode)throw new Error('encoder');assert.equal(type,'image/png');encodes.push({fn,pixels:[...this.data],width:this.width,height:this.height});}
    }
    const el=id=>{if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
    const document={hidden:false,body:new Element(),getElementById:el,createElement:tag=>tag==='canvas'?new Canvas():new Element()};
    const window={requestAnimationFrame:fn=>{raf.set(++next,fn);return next;},cancelAnimationFrame:n=>raf.delete(n),
        URL:{createObjectURL(){const u='blob:'+ ++next;urls.add(u);return u;},revokeObjectURL:u=>urls.delete(u)}};
    function canvas(w,h,color){const c=new Canvas();c.width=w;c.height=h;for(let i=0;i<c.data.length;i+=4)c.data.set(color,i);return c;}
    const base=canvas(4,3,[10,20,30,255]),front=canvas(2,2,[90,0,0,255]),mark=canvas(1,1,[0,0,255,255]);
    let scene={pixels:[3,2],layers:[{canvas:base,offset:[-1,-1]},{canvas:front,offset:[-1,0]},{canvas:mark,offset:[2,1]}]};
    const dump=D.bind({document,window,protocol:P,compose:S.compose,scene:()=>scene});
    return {dump,el,document,window,base,front,mark,canvases,encodes,downloads,urls,raf,
        set scene(v){scene=v;},set refuse(v){refuse=v;},set throwEncode(v){throwEncode=v;},
        enabled(v){el('dump-enabled').checked=v;el('dump-enabled').onchange();},
        frame(extra={}){dump.received({generation:'1',purpose:'foreground',format:'raw',width:4,height:3,complete:true,...extra},base);},
        paint(){const jobs=[...raf.values()];raf.clear();jobs.forEach(fn=>fn());},
        finish(blob={type:'image/png',size:100}){encodes.shift().fn(blob);}};
}
{
    const h=harness();h.dump.init(true,false);h.frame();h.dump.changed();h.paint();
    assert(h.el('dump-received').disabled&&h.el('dump-display').disabled);assert.equal(h.canvases.length,3,'off copied pixels');
    h.enabled(true);h.frame();h.dump.changed();h.dump.changed();assert.equal(h.raf.size,1);h.paint();
    assert.equal(h.encodes.length,0,'collection encoded PNG');assert.equal(h.downloads.length,0,'collection downloaded');
    assert.equal(h.canvases.filter(c=>c.width>1).length,4); // two source canvases + two retained
    h.dump.request('received');assert.equal(h.encodes.length,1);assert(!h.dump.request('display'));
    assert.deepEqual(h.encodes[0].pixels.slice(0,4),[10,20,30,255]);
    h.base.data.fill(0);h.front.data.fill(0);h.mark.data.fill(0);h.finish();
    assert.match(h.downloads[0],/^floe-dump-received-\d+-4x3\.png$/);
    h.dump.request('display');assert.equal(h.urls.size,0,'new request kept old URL');
    assert.deepEqual(h.encodes[0].pixels,[90,0,0,255,10,20,30,255,10,20,30,255,90,0,0,255,10,20,30,255,0,0,255,255]);
    h.finish();h.el('dump-retry').onclick();assert.equal(h.downloads.length,3);assert.equal(h.urls.size,1);assert.equal(h.document.body.children.length,0);
    for(let i=0;i<100;i++){h.frame({purpose:'margin',format:'png',generation:String(i+1)});h.dump.changed();h.paint();}
    assert.equal(h.canvases.filter(c=>c.width>1).length,4,'retention grew with frames');
    h.dump.reset();assert.equal(h.urls.size,0);assert(h.el('dump-received').disabled);assert(h.el('dump-display').disabled);
    assert(h.canvases.slice(3).every(c=>c.width===1&&c.height===1));h.dump.stop();
}
{
    const h=harness();h.dump.init(true,true);h.frame();h.paint();h.dump.request('received');
    h.enabled(false);h.enabled(true);h.frame();h.paint();assert(!h.dump.request('display'),'opt-out freed a running encoder');
    h.finish();assert.equal(h.downloads.length,0,'cancelled callback downloaded');assert(!h.el('dump-display').disabled);
    h.dump.request('display');h.dump.stop();h.dump.resume();h.enabled(true);h.frame();h.paint();assert(!h.dump.request('display'));
    h.finish();assert.equal(h.downloads.length,0);h.dump.request('display');h.finish();assert.equal(h.downloads.length,1);h.dump.stop();
}
for(const blob of [null,{type:'image/jpeg',size:50},{type:'image/png',size:0},{type:'image/png',size:80*1024*1024+1}]){
    const h=harness();h.dump.init(true,true);h.frame();h.dump.request('received');h.finish(blob);
    assert.match(h.el('dump-status').textContent,/failed|80 MiB/);assert.equal(h.downloads.length,0);assert(!h.el('dump-received').disabled);h.dump.stop();
}
{
    const h=harness();h.dump.init(true,true);h.frame();h.refuse=true;h.dump.request('received');h.finish();
    assert.match(h.el('dump-status').textContent,/refused/);assert(!h.el('dump-retry').hidden);h.refuse=false;h.el('dump-retry').onclick();assert.equal(h.downloads.length,1);
    h.throwEncode=true;h.dump.request('received');assert.match(h.el('dump-status').textContent,/could not encode/);assert(!h.el('dump-received').disabled);
    h.frame({generation:'path/to/file'});assert(h.el('dump-received').disabled);assert(!h.el('dump-status').textContent.includes('path/to/file'));
    h.frame();h.document.hidden=true;h.frame({purpose:'margin'});h.dump.changed();h.paint();assert.match(h.el('dump-received-info').textContent,/foreground/);
    h.document.hidden=false;h.scene={pixels:[8193,2],layers:[{canvas:h.base,offset:[0,0]}]};h.dump.changed();h.paint();assert(h.el('dump-display').disabled);
    h.scene=null;h.el('dump-refresh').onclick();assert.match(h.el('dump-status').textContent,/No displayed/);
    h.dump.stop();h.dump.resume();assert(!h.el('dump-enabled').checked);assert.equal(h.urls.size,0);assert.equal(h.raf.size,0);
}
console.log('WEB DISPLAY DUMP: ALL OK (off/no-copy, frozen received/composite pixels, bounded retention, no auto encode/download, one encoder across reset, late callback/limits/refusal/cleanup)');
