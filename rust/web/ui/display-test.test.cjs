'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),zlib=require('node:zlib'),D=require('./display-test.js'),I=require('./image-decode.js');
const tick=()=>new Promise(setImmediate),W=360,H=160;
function source(){
    if(process.env.FLOE_DISPLAY_TEST_DIR){return new Uint8ClampedArray(fs.readFileSync(process.env.FLOE_DISPLAY_TEST_DIR+'/test.raw').subarray(16));}
    const a=new Uint8ClampedArray(W*H*4);for(let i=3;i<a.length;i+=4)a[i]=255;
    [[255,51,51,255],[51,255,51,255],[51,51,255,255],[255,255,51,255]].forEach((c,n)=>{for(let y=30;y<130;y++)for(let x=20+n*85;x<90+n*85;x++)a.set(c,(y*W+x)*4);});return a;
}
const rgba=source();assert.deepEqual(D.compare(rgba,W,H,D.expected),{pixels:57600,different_pixels:0});
const broken=rgba.slice();broken[0]=1;assert.equal(D.compare(broken,W,H,D.expected).different_pixels,1);assert.throws(()=>D.compare([],W,H,D.expected));
function payload(format){
    if(process.env.FLOE_DISPLAY_TEST_DIR){return new Uint8Array(fs.readFileSync(process.env.FLOE_DISPLAY_TEST_DIR+'/test.'+format));}
    const b=new Uint8Array(format==='raw'?16+rgba.length:33),v=new DataView(b.buffer);
    if(format==='raw'){b.set(new TextEncoder().encode('FLOERAW1'));v.setUint32(8,W,true);v.setUint32(12,H,true);b.set(rgba,16);}
    else{b.set([137,80,78,71,13,10,26,10]);v.setUint32(8,13);v.setUint32(12,0x49484452);v.setUint32(16,W);v.setUint32(20,H);}return b;
}
let pngPixels=rgba;
if(process.env.FLOE_DISPLAY_TEST_DIR){
    const b=Buffer.from(payload('png')),parts=[];for(let p=8;p<b.length;){const n=b.readUInt32BE(p),kind=b.toString('ascii',p+4,p+8);if(kind==='IDAT')parts.push(b.subarray(p+8,p+8+n));p+=12+n;}
    const packed=zlib.inflateSync(Buffer.concat(parts)),decoded=[];assert.equal(packed.length,H*(1+W*4));
    for(let y=0;y<H;y++){assert.equal(packed[y*(1+W*4)],0);decoded.push(...packed.subarray(y*(1+W*4)+1,(y+1)*(1+W*4)));}
    pngPixels=new Uint8ClampedArray(decoded);assert.deepEqual(pngPixels,rgba);
}
function harness(){
    const nodes=new Map(),requests=[],images=[],urls=new Set(),timers=new Map();
    class Element{constructor(){this.style={};this.value='unverified';this.hidden=false;this.disabled=false;this.textContent='';}}
    class Canvas extends Element{
        constructor(){super();this._width=this._height=1;this.reset();}
        reset(){this.data=new Uint8ClampedArray(this._width*this._height*4);}
        set width(v){this._width=v;this.reset();}get width(){return this._width;}
        set height(v){this._height=v;this.reset();}get height(){return this._height;}
        getContext(){const c=this;return {
            putImageData(img){c.data.set(img.data);},getImageData(){return {data:c.data.slice()};},clearRect(){c.data.fill(0);},
            fillRect(x,y,w,h){assert.equal(this.fillStyle,'#ffffff');for(let j=y;j<y+h;j++)for(let i=x;i<x+w;i++)c.data.set([255,255,255,255],(j*c.width+i)*4);},
            drawImage(src,x,y){for(let j=0;j<src.height;j++)for(let i=0;i<src.width;i++){const dx=x+i,dy=y+j;if(dx<0||dy<0||dx>=c.width||dy>=c.height)continue;
                const a=(j*src.width+i)*4,b=(dy*c.width+dx)*4,alpha=src.data[a+3]/255;for(let k=0;k<3;k++)c.data[b+k]=src.data[a+k]*alpha+c.data[b+k]*(1-alpha);c.data[b+3]=255;}}
        };}
    }
    const el=id=>{if(!nodes.has(id))nodes.set(id,/-(png|raw|base|overlay)$/.test(id)?new Canvas():new Element());return nodes.get(id);};
    const document={createElement:tag=>{assert.equal(tag,'canvas');return new Canvas();}};
    class XHR{
        open(method,path){assert.equal(method,'GET');assert(/^\/api\/v1\/display-test\/(png|raw)$/.test(path));this.path=path;}
        setRequestHeader(k,v){assert.equal(k,'X-Floe-CSRF');assert.equal(v,'fixed-csrf');}
        send(body){assert.equal(body,null);assert.equal(this.responseType,'arraybuffer');assert.equal(this.timeout,5000);requests.push(this);}
        getResponseHeader(){return this.mime||(this.path.endsWith('raw')?'application/octet-stream':'image/png');}
        abort(){this.aborted=true;if(this.onabort)this.onabort();}
    }
    const environment={Image:class{constructor(){this.naturalWidth=this.width=W;this.naturalHeight=this.height=H;this.data=pngPixels;images.push(this);}},
        ImageData:class{constructor(data,width,height){Object.assign(this,{data,width,height});}},Blob:class{},URL:{createObjectURL(){urls.add('test');return 'test';},revokeObjectURL(u){urls.delete(u);}},
        setTimeout(fn){timers.set(1,fn);return 1;},clearTimeout(id){timers.delete(id);}};
    const ui=D.bind({el,document,window:{devicePixelRatio:2},XHR,bundle:'a'.repeat(40),csrf:()=> 'fixed-csrf',decode:(h,b,fn)=>I.create(environment,h,b,fn)});
    function reply(q=requests.at(-1),options={}){const bytes=payload(q.path.endsWith('raw')?'raw':'png');Object.assign(q,{status:200,response:bytes.buffer.slice(bytes.byteOffset,bytes.byteOffset+bytes.byteLength)},options);q.onload();}
    async function complete(p){reply();await tick();images.at(-1).onload();await tick();reply();await p;}
    return {ui,el,requests,images,urls,timers,reply,complete};
}
async function main(){
    const h=harness();await h.ui.run();assert.equal(h.requests.length,0);h.ui.open();assert.equal(h.requests.length,0);
    const p=h.ui.run();await h.ui.run();assert.equal(h.requests.length,1);await h.complete(p);assert.equal(h.requests.length,2);
    assert(!h.el('display-test-results').hidden);const r=JSON.parse(h.el('display-test-report').textContent);
    assert.deepEqual(Object.values(r.pixel_checks).map(p=>p.different_pixels),[0,0,0]);assert.equal(r.desktop_acceptance,'unverified');assert.equal(r.screen_observation,'unverified');
    assert.equal(h.el('display-test-base').style.left,'-10px');assert.equal(h.el('display-test-stack').style.width,'160px');assert.equal(h.el('display-test-png').style.width,'180px');
    h.el('display-test-observed').value='all_visible';h.el('display-test-observed').onchange();assert.equal(JSON.parse(h.el('display-test-report').textContent).desktop_acceptance,'unverified');
    h.ui.close();assert.equal(h.el('display-test-png').width,1);assert.equal(h.el('display-test-report').textContent,'');assert.equal(h.urls.size,0);
    for(const phase of ['read','after_read','decode']){
        const s=harness();s.ui.open();const pending=s.ui.run();
        if(phase!=='read'){s.reply();if(phase==='decode')await tick();}
        const late=s.images[0]&&s.images[0].onload;s.ui.close();if(late)late();await pending;
        assert.equal(s.requests.length,1);assert.equal(s.urls.size,0);assert.equal(s.timers.size,0);assert.equal(s.el('display-test-png').width,1);
        s.ui.open();assert.equal(s.requests.length,1,'automatic replay');const fresh=s.ui.run();await s.complete(fresh);assert(!s.el('display-test-results').hidden);
    }
    for(const mode of ['status','mime','timeout','limit','decode']){
        const f=harness();f.ui.open();const pending=f.ui.run(),q=f.requests[0];
        if(mode==='status')f.reply(q,{status:401});if(mode==='mime')f.reply(q,{mime:'text/html'});
        if(mode==='timeout')q.ontimeout();if(mode==='limit')q.onprogress({loaded:1e6});
        if(mode==='decode'){f.reply();await tick();f.images[0].onerror();}
        await pending;assert(f.el('display-test-results').hidden);assert.equal(f.requests.length,1);assert.equal(f.urls.size,0);assert.equal(f.el('display-test-run').disabled,false);
    }
    console.log('WEB DISPLAY TEST: ALL OK (PNG/raw/crop pixels, explicit GET, limits/errors, cancel at read/decode boundary, DPR, unverified screen, no replay)');
}
main().catch(e=>{console.error(e);process.exitCode=1;});
