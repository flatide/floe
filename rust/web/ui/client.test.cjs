'use strict';
// Deterministic client-state tests, not a substitute for real browser pixel QA.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const P = require('./protocol.js');
const DRC = require('./drc.js'), drcDisplays = [], drcClicks = [], observers = [];
const nodes = new Map(), images = [], sockets = [], urls = new Set(), draws = [], requests = [];
const listeners = {}, docListeners = {};
function listen(target,k,fn){const old=target[k];target[k]=old?(event)=>{old(event);fn(event);}:fn;}
let clock = 10000;
let drcOptions, contextChanges=0;
let viewportSize = [100, 80];
class Element {
    constructor(id, tag='div') { Object.assign(this, {id, tag, value:'', checked:false, disabled:false, hidden:false,
        dataset:{}, style:{}, children:[], className:'', textContent:'', width:1, height:1}); }
    appendChild(child) { this.children.push(child); if (child.tag==='option'&&!this.value) {this.value=child.value;} return child; }
    setAttribute(k,v) {this[k]=v;}
    querySelectorAll(tag) {return this.children.flatMap(c=>[...(c.tag===tag?[c]:[]), ...c.querySelectorAll(tag)]);}
    addEventListener(k,f) {listen(this,k,f);}
    getBoundingClientRect() {const [w,h]=viewportSize;return {left:0,top:0,right:w,bottom:h,width:w,height:h};}
    getContext() {return ctx;}
    focus() {}
    select() {}
    get textContent(){return this._text||'';}
    set textContent(v){this._text=v;this.children=[];}
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
    layers:{mode:'all'},layers_isolated:false,frames:false,labels:false,font_px:14,mono:false,status:'idle',source_stale:false,
    deck_skipped:'0',failure:null,submitted:'1',consumed:'1',discarded:'0',capabilities:{labels:true}};
let open=false, lastSeq='0';
const layerRow={pair:[7,0],name:'MASK',aliases:[],parent:null,head:false,visible:true,color:'#ffffff',fill:{kind:'solid'},width:1};
const document={hidden:false,activeElement:null,title:'',
    getElementById:node,querySelector:()=>({content:bundle}),createElement:tag=>new Element('',tag),
    createTextNode:text=>Object.assign(new Element(''),{textContent:text}),
    addEventListener:(k,f)=>listen(docListeners,k,f)};
class XHR {
    open(method,path){this.method=method;this.path=path;}
    setRequestHeader() {}
    send(text){
        const body=text===null?null:JSON.parse(text); requests.push({method:this.method,path:this.path,body});
        let value, status=200;
        if (this.path==='/api/v1/session/exchange') {value={csrf:'c'.repeat(64),bundle,protocol:1};}
        else if(this.path==='/api/v1/capabilities') {value={protocol:1,bundle};}
        else if(this.path==='/api/v1/catalog') {value={sources:[{source_id:'src',title:'synthetic',deck:false,levels:0},{source_id:'deck',title:'synthetic deck',deck:true,levels:2}]};}
        else if(this.path==='/api/v1/catalog/deck/levels/0') {value={levels:[{id:'1',title:'Level 1'},{id:'2',title:'Level 2'}],next:null};}
        else if(this.path==='/api/v1/startup') {value={request:{kind:'open',seq:'1',source_id:'src',mode:'level',levels:{mode:'all'},body:{detail:'high'}}};}
        else if(this.path==='/api/v1/operations'&&this.method==='POST') {open=true;lastSeq=body.seq;value={seq:lastSeq,kind:'open',phase:'succeeded',view_id:viewId};status=202;}
        else if(this.path==='/api/v1/operations') {value={last_seq:lastSeq,active:null,history:open?[{seq:lastSeq,kind:'open',phase:'succeeded',view_id:viewId}]:[]};}
        else if(this.path==='/api/v1/view') {status=open?200:404;value=open?{title:'synthetic',source_id:'src',mode:'level',levels:null,view:{...snapshot,connection_epoch:''}}:null;}
        else if(this.path.endsWith('/layers/0')) {value={state_rev:snapshot.state_rev,render_key:snapshot.render_key,total:1,start:0,next:null,rows:[layerRow]};}
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
const window={FloeProtocol:P,FloeGestures:require('./gestures.js'),FloeRulers:require('./rulers.js'),FloeDRCGroups:require('./drc-groups.js'),FloeDRC:{...DRC,bind(o){
    drcOptions=o;const panel=DRC.bind(o),paint=panel.paint,changed=panel.contextChanged;
    panel.contextChanged=()=>{contextChanges++;changed();};panel.paint=(p,s)=>{drcDisplays.push({p,s});paint(p,s);};
    const click=panel.click;panel.click=(...v)=>{drcClicks.push(v);return click(...v);};return panel;
}},FloePanelState:require('./panel-state.js'),FloeDRCBuild:require('./drc-build.js'),ResizeObserver:class {constructor(fn){this.fn=fn;observers.push(this);}observe(e){this.target=e;}disconnect(){this.target=null;}},devicePixelRatio:1,
    addEventListener:(k,f)=>listen(listeners,k,f),setTimeout,requestAnimationFrame:fn=>setTimeout(fn,0),cancelAnimationFrame:clearTimeout};
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
function packet(format,id,rev='1',ep=epoch,extra={}){
    const width=extra.width||100,height=extra.height||80;
    const data=new Uint8Array(format==='raw'?16+width*height*4:33),d=new DataView(data.buffer);
    if(format==='raw'){data.set(new TextEncoder().encode('FLOERAW1'));d.setUint32(8,width,true);d.setUint32(12,height,true);
        for(let i=16;i<data.length;i+=4){data.set([i%256,((i-16)/400)|0,127,255],i);}
    }else{data.set([137,80,78,71,13,10,26,10]);d.setUint32(8,13);d.setUint32(12,0x49484452);d.setUint32(16,width);d.setUint32(20,height);}
    const header={...snapshot,type:'frame',protocol:1,connection_epoch:ep,state_rev:rev,render_rev:rev,frame_id:id,
        generation:id,round:'1',purpose:'foreground',width,height,row0:'top',format,payload_length:String(data.length),
        final:true,partial:false,deferred:'0',labels_truncated:false,complete:true,approximate:false,query:false,perf:{},...extra};
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
    // A landed complete margin supplies the incoming half-screen immediately:
    // no new pixel draw or network reply is needed for the CSS crop.
    const margin={purpose:'margin',width:196,height:176,bbox_dbu:['-58.9375','-48','137.0625','128']};
    snapshot.connection_epoch=nextEpoch;snapshot.capabilities.margin=true;
    snapshot.margin={frame_id:'8',origin_px:[48,48],crop_safe:true};
    second.receive(snapshot);second.receive(packet('raw','8','1',nextEpoch,margin));
    assert.equal(second.sent.at(-1).disposition,'displayed');
    assert.equal(node('canvas').hidden,true);assert.equal(node('margin-canvas').hidden,false);
    assert.equal(node('margin-canvas').style.left,'-48px');
    assert.deepEqual(DRC.point(drcDisplays.at(-1).p,0,0),[10.9375,80]);
    assert(node('perf').textContent.includes('100 × 80 px'));
    assert(!node('perf').textContent.includes('196 × 176 px'));
    const painted=draws.length;
    const listRequests=requests.filter(r=>r.path.endsWith('/layers/0')).length;
    node('viewport').keydown({key:'ArrowRight',preventDefault(){},shiftKey:false});
    assert.equal(draws.length,painted);assert.equal(node('margin-canvas').style.left,'-96px');
    assert.equal(node('canvas').hidden,true);
    const panEdit=second.sent.filter(m=>m.type==='view.set').at(-1);
    second.receive({type:'accepted',seq:panEdit.seq,state_rev:'3',render_rev:'3'});
    snapshot.state_rev='3';snapshot.render_rev='3';snapshot.bbox_dbu=['37.0625','0','137.0625','80'];
    snapshot.margin.origin_px=[96,48];second.receive(snapshot);
    assert.equal(node('margin-canvas').style.left,'-96px');
    assert.deepEqual(DRC.point(drcDisplays.at(-1).p,0,0),[-37.0625,80]);
    assert(node('status').textContent.includes('Live · margin crop'));
    assert.equal(requests.filter(r=>r.path.endsWith('/layers/0')).length,listRequests,'pan refreshed the layer panel');
    // Truncated labels are base only, not a completed replacement for foreground.
    snapshot.margin={frame_id:'9',origin_px:[96,48],crop_safe:false};second.receive(snapshot);
    second.receive(packet('raw','9','3',nextEpoch,{...margin,complete:false,labels_truncated:true}));
    assert.equal(node('canvas').hidden,false);assert.equal(node('margin-canvas').hidden,false);
    assert.equal(node('canvas').style.left,'-48px');
    // A policy change during slow margin decoding cannot land the stale image.
    snapshot.margin.frame_id='10';second.receive(snapshot);
    second.receive(packet('png','10','3',nextEpoch,margin));const staleMargin=images.at(-1);
    staleMargin.naturalWidth=196;staleMargin.naturalHeight=176;
    snapshot.state_rev='4';snapshot.render_rev='4';snapshot.render_key='2';snapshot.margin=null;
    second.receive(snapshot);const beforeLate=draws.length;staleMargin.onload();
    assert.equal(draws.length,beforeLate);assert.equal(node('margin-canvas').hidden,true);
    assert.equal(node('margin-canvas').width,1);
    assert.equal(second.sent.at(-1).disposition,'discarded');assert.equal(urls.size,0);
    await wait(()=>node('layers').children.length===1);
    const submitEvent={preventDefault(){}};
    node('font-px').value='999';const beforeInvalid=second.sent.length;node('font-px').onchange();
    assert.equal(second.sent.length,beforeInvalid);
    node('layers').children[0].children.at(-1).onclick();
    node('style-fill').value='pattern';node('style-fill').onchange();
    node('style-pattern').value='ffff';node('style-editor').onsubmit(submitEvent);
    assert.equal(second.sent.length,beforeInvalid);
    node('style-pattern').value=new Array(16).fill('a55a').join(' ');node('style-width').value='4';
    node('style-editor').onsubmit(submitEvent);
    let sent=second.sent.at(-1);assert.equal(sent.type,'view.set');
    assert.deepEqual(sent.body.styles,[{pair:[7,0],color:'#ffffff',fill:{kind:'pattern',rows:new Array(16).fill(0xa55a)},width:4}]);
    async function applied(policy=true){
        const s=second.sent.filter(m=>m.type==='view.set').at(-1);
        snapshot.state_rev=P.next(snapshot.state_rev);snapshot.render_rev=P.next(snapshot.render_rev);
        if(policy){snapshot.render_key=P.next(snapshot.render_key);}
        second.receive({type:'accepted',seq:s.seq,state_rev:snapshot.state_rev,render_rev:snapshot.render_rev});second.receive(snapshot);
        await new Promise(setImmediate);
    }
    await applied();
    node('font-px').value='18';node('font-px').onchange();assert.equal(second.sent.at(-1).body.font_px,18);await applied();
    node('viewport').keydown({key:'f',preventDefault(){}});assert.deepEqual(second.sent.at(-1).body,{frames:true});await applied();
    node('viewport').keydown({key:'a',ctrlKey:true,preventDefault(){}});assert.equal(second.sent.at(-1).body.navigation.kind,'fit');await applied(false);
    node('viewport').keydown({key:'9',preventDefault(){}});assert.equal(second.sent.at(-1).body.depth,'9');await applied();
    node('viewport').keydown({key:'9',preventDefault(){}});assert.equal(second.sent.at(-1).body.depth,'full');await applied();
    // Free mouse pan has no network traffic while moving and preserves the
    // translated foreground until its new (non-16px) native phase arrives.
    second.receive(packet('raw','11',snapshot.render_rev,nextEpoch));
    const dragEdits=()=>second.sent.filter(m=>m.type==='view.set').length;
    const beforeDrag=dragEdits(),beforeDragDraws=draws.length;
    const mouse=(x,y)=>({button:0,buttons:1,clientX:x,clientY:y,preventDefault(){}});
    node('viewport').mousedown(mouse(20,20));listeners.mouseup({...mouse(20,20),buttons:0});
    node('viewport').mousedown(mouse(20,20));listeners.mouseup({...mouse(20,20),buttons:0,detail:2});
    assert.deepEqual(drcClicks,[[20,20,false,undefined],[20,20,true,undefined]]);assert.equal(dragEdits(),beforeDrag);
    node('viewport').mousedown(mouse(20,20));listeners.mousemove(mouse(33,31));
    await wait(()=>node('canvas').style.left==='13px');
    assert.deepEqual(DRC.point(drcDisplays.at(-1).p,0,0),[-24.0625,91]);
    assert.equal(dragEdits(),beforeDrag);assert.equal(draws.length,beforeDragDraws);
    listeners.mouseup(mouse(33,31));
    assert.equal(dragEdits(),beforeDrag+1);
    assert.deepEqual(second.sent.at(-1).body.navigation,{kind:'pan',x:-0.13,y:0.1375,snap:false});
    assert.deepEqual(draws.slice(beforeDragDraws).map(d=>[d.x,d.y]),[[13,11],[0,0]],'preview composite jumped before native reply');
    assert.equal(node('canvas').style.left,'0px');
    snapshot.bbox_dbu=['24.0625','11','124.0625','91'];await applied(false);
    assert.deepEqual(DRC.point(drcDisplays.at(-1).p,0,0),[-24.0625,91],'DRC did not follow frozen pan preview');
    assert.equal(node('canvas').style.left,'0px');
    assert.equal(draws.length,beforeDragDraws+2,'snapshot redrew the frozen preview');
    second.receive(packet('raw','12',snapshot.render_rev,nextEpoch));
    node('viewport').mousedown(mouse(20,20));listeners.mousemove(mouse(42,41));
    await wait(()=>node('canvas').style.left==='22px');listeners.blur();
    assert.equal(node('canvas').style.left,'0px');assert.equal(dragEdits(),beforeDrag+1);
    listeners.mouseup(mouse(42,41));assert.equal(dragEdits(),beforeDrag+1,'blur left a late mouseup edit');
    assert.equal(drcClicks.length,2,'pan or cancelled drag became a DRC click');
    // Panel/element resize, including a height change without window.resize:
    // keep old pixels/overlay centered, request native dimensions exactly once.
    const beforeResize=dragEdits();viewportSize=[120,90];observers[0].fn();
    await new Promise(resolve=>setTimeout(resolve,140));
    assert.equal(dragEdits(),beforeResize+1);assert.deepEqual(second.sent.at(-1).body.pixels,[120,90]);
    assert.deepEqual(DRC.point(drcDisplays.at(-1).p,0,0),[-14.0625,96]);
    observers[0].fn();await new Promise(resolve=>setTimeout(resolve,140));
    assert.equal(dragEdits(),beforeResize+1,'duplicate pending resize');
    snapshot.pixels=[120,90];await applied(false);
    observers[0].fn();await new Promise(resolve=>setTimeout(resolve,140));
    assert.equal(dragEdits(),beforeResize+1,'same-size observer invalidated native pixels');
    node('source').value='deck';node('source').onchange();node('level-more').onclick();node('level-more').onclick();
    await wait(()=>node('level-list').children.length===2);
    assert.equal(requests.filter(r=>r.path.includes('/catalog/deck/levels/')).length,1,'duplicate level page');
    // Prepared edits send only a short token. Dependent UI is completed once,
    // after ACK+snapshot and before viewport-follow observers run.
    const done=[],token='7'.repeat(64),beforeObservers=contextChanges;
    const cancelSent=drcOptions.navigate({kind:'goto',center_um:['bad','ignored'],width_um:'999'},token,e=>{
        done.push(e);assert.equal(drcOptions.context().state.layers_isolated,true);
        assert.equal(contextChanges,beforeObservers,'live filters observed the new view before ACK callback');
    });
    const prepared=second.sent.at(-1);
    assert.deepEqual(prepared,{type:'view.apply',connection_epoch:nextEpoch,view_id:viewId,base_state_rev:snapshot.state_rev,token,seq:prepared.seq});
    assert.equal(cancelSent(),false,'sent edits cannot be unsent');assert.equal(done.length,0);
    snapshot.state_rev=P.next(snapshot.state_rev);snapshot.render_rev=P.next(snapshot.render_rev);snapshot.layers_isolated=true;
    second.receive({type:'accepted',seq:prepared.seq,state_rev:snapshot.state_rev,render_rev:snapshot.render_rev});assert.equal(done.length,0);
    second.receive(snapshot);assert.deepEqual(done,[null]);second.receive(snapshot);assert.equal(done.length,1);
    const restored=[],cancelled=[];
    drcOptions.restoreLayers(e=>restored.push(e));const restore=second.sent.at(-1);assert.deepEqual(restore.body,{restore_layers:true});
    const cancelQueued=drcOptions.navigate({},token,e=>cancelled.push(e));assert.equal(second.sent.at(-1),restore);
    assert.equal(cancelQueued(),true);assert.match(cancelled[0],/cancelled/);assert.equal(cancelQueued(),false);
    second.receive({type:'error',seq:restore.seq,code:'stale_state'});assert.equal(restored.length,0);
    second.receive(snapshot);assert.equal(restored.length,1);assert.match(restored[0],/not replayed/);assert.equal(done.length,1);
    assert.equal(second.sent.at(-1),restore,'cancelled queued token was transmitted');
    const failed=[];second.bufferedAmount=20000;
    drcOptions.navigate({},token,e=>failed.push(e));assert.match(failed[0],/Input limit/);second.bufferedAmount=0;
    // Another authorized connection can edit between controller.edit() and
    // the reply snapshot. Do not attach an old jump's CD/filter to that view.
    const superseded=[];drcOptions.navigate({},token,e=>superseded.push(e));const supersededWire=second.sent.at(-1);
    const approved=P.next(snapshot.state_rev);snapshot.state_rev=P.next(approved);
    second.receive({type:'accepted',seq:supersededWire.seq,state_rev:approved,render_rev:snapshot.render_rev});
    second.receive(snapshot);assert.equal(superseded.length,1);assert.match(superseded[0],/changed again/);
    // Closing a socket rejects all queued callbacks, never silently leaving a
    // pending review effect or replaying one on the replacement connection.
    const interrupted=[];
    drcOptions.navigate({},token,e=>interrupted.push(e));
    for(let i=0;i<64;i++)drcOptions.restoreLayers(e=>interrupted.push(e));
    drcOptions.restoreLayers(e=>interrupted.push(e));assert.equal(interrupted.length,1);assert.match(interrupted[0],/queue is full/);
    second.close();assert.equal(interrupted.length,66);assert(interrupted.every(e=>typeof e==='string'));
    document.hidden=false;docListeners.visibilitychange();const reconnected=sockets.at(-1);hello(reconnected,'9'.repeat(64));
    assert(!reconnected.sent.some(m=>m.type==='view.set'||m.type==='view.apply'));
    second.receive({type:'accepted',seq:prepared.seq,state_rev:snapshot.state_rev,render_rev:snapshot.render_rev});assert.equal(done.length,1);
    for(const s of sockets){for(let i=1;i<s.sent.length;i++){assert(P.compare(s.sent[i-1].seq,s.sent[i].seq)<0);}}
    listeners.pagehide();
    assert.equal(observers[0].target,null);
    console.log('WEB CLIENT: ALL OK (startup/frames/epochs, margin/pan/DRC, controls, token-only edits, ACK+snapshot ordering, cancellation/limits/reconnect, cleanup)');
})().catch(e=>{console.error(e);process.exitCode=1;});
