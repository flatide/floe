'use strict';
// Deterministic client-state tests, not a substitute for real browser pixel QA.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const P = require('./protocol.js');
const nodes = new Map(), images = [], sockets = [], urls = new Set(), draws = [], requests = [];
const listeners = {}, docListeners = {};
let clock = 10000;
class Element {
    constructor(id, tag='div') { Object.assign(this, {id, tag, value:'', checked:false, disabled:false, hidden:false,
        dataset:{}, style:{}, children:[], className:'', textContent:'', width:1, height:1}); }
    appendChild(child) { this.children.push(child); if (child.tag==='option'&&!this.value) {this.value=child.value;} return child; }
    setAttribute(k,v) {this[k]=v;}
    querySelectorAll(tag) {return this.children.flatMap(c=>[...(c.tag===tag?[c]:[]), ...c.querySelectorAll(tag)]);}
    addEventListener(k,f) {this[k]=f;}
    getBoundingClientRect() {return {left:0,top:0,right:100,bottom:80,width:100,height:80};}
    getContext() {return ctx;}
    focus() {}
}
const ctx = {imageSmoothingEnabled:true, putImageData(data,x,y) {draws.push({kind:'raw',data:[...data.data],x,y});},
    drawImage(image,x,y) {draws.push({kind:'png',x,y});}};
for (const id of [...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1])) {
    nodes.set(id,new Element(id));
}
nodes.get('levels-all').checked=true;
const node = id=>nodes.get(id);
const bundle='d'.repeat(40), viewId='a'.repeat(64), epoch='b'.repeat(64);
const snapshot={type:'snapshot',view_id:viewId,connection_epoch:epoch,dataset_revision:'1',state_rev:'1',
    render_rev:'1',render_key:'1',worker_epoch:'2',bbox_dbu:['-10.9375','0','89.0625','80'],
    dbu_um:'1',pixels:[100,80],depth:'full',max_depth:'2',detail:'high',thin:'auto',effective_thin:'cull',
    layers:{mode:'all'},frames:false,labels:false,font_px:14,mono:false,status:'idle',source_stale:false,
    deck_skipped:'0',failure:null,submitted:'1',consumed:'1',discarded:'0',capabilities:{labels:true}};
let open=false, lastSeq='0';
const document={hidden:false,activeElement:null,title:'',
    getElementById:node,querySelector:()=>({content:bundle}),createElement:tag=>new Element('',tag),
    createTextNode:text=>Object.assign(new Element(''),{textContent:text}),
    addEventListener:(k,f)=>{docListeners[k]=f;}};
class XHR {
    open(method,path){this.method=method;this.path=path;}
    setRequestHeader() {}
    send(text){
        const body=text===null?null:JSON.parse(text); requests.push({method:this.method,path:this.path,body});
        let value, status=200;
        if (this.path==='/api/v1/session/exchange') {value={csrf:'c'.repeat(64),bundle,protocol:1};}
        else if(this.path==='/api/v1/capabilities') {value={protocol:1,bundle};}
        else if(this.path==='/api/v1/catalog') {value={sources:[{source_id:'src',title:'synthetic',deck:false,levels:0}]};}
        else if(this.path==='/api/v1/startup') {value={request:{kind:'open',seq:'1',source_id:'src',mode:'level',levels:{mode:'all'},body:{detail:'high'}}};}
        else if(this.path==='/api/v1/operations'&&this.method==='POST') {open=true;lastSeq=body.seq;value={seq:lastSeq,kind:'open',phase:'succeeded',view_id:viewId};status=202;}
        else if(this.path==='/api/v1/operations') {value={last_seq:lastSeq,active:null,history:open?[{seq:lastSeq,kind:'open',phase:'succeeded',view_id:viewId}]:[]};}
        else if(this.path==='/api/v1/view') {status=open?200:404;value=open?{title:'synthetic',source_id:'src',mode:'level',levels:null,view:{...snapshot,connection_epoch:''}}:null;}
        else if(this.path.endsWith('/layers/0')) {value={state_rev:snapshot.state_rev,total:0,start:0,next:null,rows:[]};}
        else {throw new Error('Unexpected HTTP '+this.path);}
        this.status=status;this.responseText=JSON.stringify(value);
        setImmediate(()=>this.onload());
    }
}
class Socket {
    static OPEN=1;
    constructor(){this.readyState=1;this.bufferedAmount=0;this.sent=[];sockets.push(this);}
    send(text){this.sent.push(JSON.parse(text));}
    close(){this.readyState=3;if(this.onclose){this.onclose();}}
    receive(value){this.onmessage({data:typeof value==='object'&&!(value instanceof ArrayBuffer)?JSON.stringify(value):value});}
}
class Image {
    constructor(){this.naturalWidth=100;this.naturalHeight=80;images.push(this);}
}
const window={FloeProtocol:P,devicePixelRatio:1,addEventListener:(k,f)=>{listeners[k]=f;},setTimeout};
const storage=new Map();
const sandbox={window,document,XMLHttpRequest:XHR,WebSocket:Socket,Image,ImageData:class {constructor(data,w,h){this.data=data;this.width=w;this.height=h;}},
    location:{origin:'http://127.0.0.1:1234',hash:'#bootstrap='+'e'.repeat(64),pathname:'/'},
    history:{replaceState(){sandbox.location.hash='';}},sessionStorage:{getItem:k=>storage.get(k)||null,setItem:(k,v)=>storage.set(k,v),removeItem:k=>storage.delete(k)},
    URL:{createObjectURL(){const s='blob:test/'+images.length;urls.add(s);return s;},revokeObjectURL:s=>urls.delete(s)},
    Blob,TextEncoder,TextDecoder,ArrayBuffer,DataView,Uint8Array,Uint8ClampedArray,
    setTimeout,clearTimeout,setInterval:()=>0,Date:{now:()=>clock+=100},console};
vm.runInNewContext(fs.readFileSync(__dirname+'/app.js','utf8'),sandbox,{filename:'app.js'});
async function wait(test){for(let i=0;i<1000;i++){if(test()){return;}await new Promise(setImmediate);}throw new Error('client did not progress');}
function hello(ws,ep=epoch){ws.receive({type:'hello',protocol:1,bundle,view_id:viewId,connection_epoch:ep});ws.receive({...snapshot,connection_epoch:ep});}
function packet(format,id,rev='1',ep=epoch){
    const data=new Uint8Array(format==='raw'?16+100*80*4:33),d=new DataView(data.buffer);
    if(format==='raw'){data.set(new TextEncoder().encode('FLOERAW1'));d.setUint32(8,100,true);d.setUint32(12,80,true);
        for(let i=16;i<data.length;i+=4){data.set([i%256,((i-16)/400)|0,127,255],i);}
    }else{data.set([137,80,78,71,13,10,26,10]);d.setUint32(8,13);d.setUint32(12,0x49484452);d.setUint32(16,100);d.setUint32(20,80);}
    const header={...snapshot,type:'frame',protocol:1,connection_epoch:ep,state_rev:rev,render_rev:rev,frame_id:id,
        generation:id,round:'1',purpose:'foreground',width:100,height:80,row0:'top',format,payload_length:String(data.length),
        final:true,partial:false,deferred:'0',labels_truncated:false,complete:true,approximate:false,query:false,perf:{}};
    const text=new TextEncoder().encode(JSON.stringify(header)),out=new Uint8Array(4+text.length+data.length);
    new DataView(out.buffer).setUint32(0,text.length,true);out.set(text,4);out.set(data,4+text.length);
    return out.buffer;
}
(async()=>{
    await wait(()=>sockets.length===1);
    assert.equal(sandbox.location.hash,'');
    const opened=requests.filter(r=>r.method==='POST'&&r.path==='/api/v1/operations');
    assert.equal(opened.length,1);assert.deepEqual(opened[0].body.body.pixels,[100,80]);
    const ws=sockets[0];hello(ws);
    ws.receive(packet('raw','1'));
    assert.equal(draws.length,1);assert.deepEqual(draws[0].data.slice(0,4),[16,0,127,255]);
    assert.equal(ws.sent.at(-1).disposition,'displayed');
    assert.equal(node('canvas').style.width,'100px');
    // An accepted newer render invalidates old PNG even before its snapshot arrives.
    ws.receive(packet('png','2'));const old=images.at(-1), oldOnload=old.onload;
    node('zoom-in').onclick();const edit=ws.sent.find(m=>m.type==='view.set');
    assert(edit);ws.receive({type:'accepted',seq:edit.seq,state_rev:'2',render_rev:'2'});
    oldOnload();assert.equal(draws.length,1);assert.equal(ws.sent.at(-1).disposition,'discarded');assert.equal(urls.size,0);
    snapshot.state_rev='2';snapshot.render_rev='2';ws.receive(snapshot);
    ws.receive(packet('png','3','2'));images.at(-1).onload();
    assert.equal(draws.length,2);assert.equal(draws.at(-1).kind,'png');assert.equal(urls.size,0);
    // Hidden documents return credit without painting; visibility restores a fresh epoch.
    ws.receive(packet('png','4','2'));const late=images.at(-1).onload;
    document.hidden=true;docListeners.visibilitychange();assert.equal(urls.size,0);
    assert.equal(ws.sent.at(-1).disposition,'discarded');
    document.hidden=false;docListeners.visibilitychange();assert.equal(sockets.length,2);
    const second=sockets[1],nextEpoch='f'.repeat(64);hello(second,nextEpoch);late();
    assert.equal(draws.length,2);
    second.receive(packet('raw','5','2',epoch));
    assert.equal(second.sent.at(-1).disposition,'discarded');assert.equal(draws.length,2);
    second.receive(packet('raw','6','2',nextEpoch));assert.equal(draws.length,3);
    // Decode errors release the URL and credit exactly once.
    second.receive(packet('png','7','2',nextEpoch));images.at(-1).onerror();
    assert.equal(urls.size,0);assert.equal(second.sent.at(-1).disposition,'discarded');
    for(const s of sockets){for(let i=1;i<s.sent.length;i++){assert(P.compare(s.sent[i-1].seq,s.sent[i].seq)<0);}}
    listeners.pagehide();
    console.log('WEB CLIENT: ALL OK (single startup, raw/PNG, stale decode, epochs, hidden-tab credit, URL cleanup)');
})().catch(e=>{console.error(e);process.exitCode=1;});
