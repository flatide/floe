'use strict';
// Deterministic client-state tests, not a substitute for real browser pixel QA.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const P = require('./protocol.js');
const DRC = require('./drc.js'), drcDisplays = [], drcClicks = [], observers = [];
const Clip=require('./clip.js'),clipEnabled=process.env.FLOE_TEST_CLIP==='1',forms=[];
const snapshotEnabled=process.env.FLOE_TEST_SNAPSHOT==='1',captures=[],copies=[],downloads=[];
const settingsEnabled=process.env.FLOE_TEST_SETTINGS==='1';
const defaultsEnabled=process.env.FLOE_TEST_DEFAULTS==='1';let defaultOp=null;
const minimapEnabled=process.env.FLOE_TEST_MINIMAP==='1';
const exitEnabled=process.env.FLOE_TEST_EXIT==='1';
const exitFailure=process.env.FLOE_TEST_EXIT_FAILURE==='1';
const modeEnabled=process.env.FLOE_TEST_MODE==='1';
const startupEnabled=process.env.FLOE_TEST_STARTUP==='1';
const launchEnabled=process.env.FLOE_TEST_LAUNCH==='1';
const indexOpenEnabled=process.env.FLOE_TEST_INDEX_OPEN==='1',indexSource='f'.repeat(64),indexOperations=[];
const gotoEnabled=process.env.FLOE_TEST_GOTO==='1';
const paletteEnabled=process.env.FLOE_TEST_PALETTE==='1';
const fillEditorEnabled=process.env.FLOE_TEST_FILL_EDITOR==='1';
function presetFixture(){
    const lines=name=>fs.readFileSync(__dirname+'/../../../floe/'+name,'utf8').split('\n').map(l=>l.trim()).filter(l=>l&&!l.startsWith('#')).map(l=>l.split(/\s+/));
    return {version:1,colors:lines('colornames.def').map(([name,color])=>({name,color:'#'+color.toLowerCase()})),
        fills:lines('fillpatterns.def').map(([name,...words])=>{const rows=words.map(w=>parseInt(w,16));return {name,rows,fill:['solid','clear','speckle'].includes(name)?{kind:name}:{kind:'pattern',rows}};})};
}
let liveFillRows=presetFixture().fills.map(p=>({name:p.name,rows:p.rows}));
let launchState={revision:'0',pending:null},launchPolls=[],launchRegistered=false,launchReceipt=null;
let startupReceipt=null,startupFail=true;
const startupBody={depth:'17',detail:'high',thin:'keep',frames:true,labels:false,navigation:{kind:'goto',center_um:['1.25','-2.5']}};
let modeOperation=null,serverMode='chip',modeReadFailure=false,modeViewReadFailure=false,modeLosePost=false,modeNumber=0;
let textSelection=null;
let clipController,clipOp=null,clipFile=null;
function clipState(){return {available:true,kind:'exact_clip',jobs_default:4,jobs_min:1,jobs_max:16,
    operations:{last_seq:clipOp?'1':'0',active:null,history:clipOp?[clipOp]:[]},artifacts:clipFile?[clipFile]:[],
    limits:{artifacts:4,artifact_bytes:'536870912',total_bytes:'2147483648',readers:2,ttl_seconds:600},
    usage:{entries:clipFile?1:0,pending:0,bytes:clipFile?'64':'0',readers:0}};}
const nodes = new Map(), images = [], sockets = [], urls = new Set(), draws = [], requests = [];
const listeners = {}, docListeners = {};
function listen(target,k,fn){const old=target[k];target[k]=old?(event)=>{old(event);fn(event);}:fn;}
let clock = 10000, freezeTime=false;
let drcOptions, contextChanges=0, consumeDRC=false;
let viewportSize = [100, 80];
class Element {
    constructor(id, tag='div') { Object.assign(this, {id, tag, value:'', checked:false, disabled:false, hidden:false,
        dataset:{}, style:{}, children:[], className:'', textContent:'', width:1, height:1}); }
    appendChild(child) { this.children.push(child); if (child.tag==='option'&&!this.value) {this.value=child.value;} return child; }
    removeChild(child) {this.children.splice(this.children.indexOf(child),1);return child;}
    submit() {assert.equal(this.tag,'form');forms.push({method:this.method,action:this.action,body:this.children.map(c=>[c.name,c.value])});}
    click() {if(this.id==='settings-file'){return;}assert.equal(this.tag,'a');downloads.push({href:this.href,name:this.download});}
    toBlob(fn,type) {assert.equal(this.tag,'canvas');assert.equal(type,'image/png');captures.push({width:this.width,height:this.height,draws:draws.slice()});setImmediate(()=>fn(new Blob(['mock PNG'],{type})));}
    setAttribute(k,v) {this[k]=v;}
    getAttribute(k) {return Object.hasOwn(this,k)?this[k]:null;}
    removeAttribute(k) {delete this[k];}
    contains(n) {return this===n||this.children.some(c=>c.contains(n));}
    querySelectorAll(tag) {return this.children.flatMap(c=>[...(c.tag===tag?[c]:[]), ...c.querySelectorAll(tag)]);}
    addEventListener(k,f) {listen(this,k,f);}
    getBoundingClientRect() {const [w,h]=viewportSize;return {left:0,top:0,right:w,bottom:h,width:w,height:h};}
    getContext() {return this.id==='minimap'?{fillRect(){},drawImage(){}}:ctx;}
    focus() {document.activeElement=this;}
    select() {}
    get textContent(){return this._text||'';}
    set textContent(v){this._text=v;this.children=[];}
}
const ctx = {imageSmoothingEnabled:true, putImageData(data,x,y) {draws.push({kind:'raw',data:[...data.data],x,y});},
    drawImage(image,x,y) {draws.push({kind:'png',x,y,source:image.id});},clearRect(){},save(){},restore(){},beginPath(){},rect(){},clip(){},
    moveTo(){},lineTo(){},setLineDash(){},closePath(){},stroke(){},setTransform(){},fill(){},fillRect(){},fillText(){},measureText(s){return {width:s.length*7};}};
for (const id of [...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1])) {
    nodes.set(id,new Element(id));
}
nodes.get('levels-all').checked=true;
nodes.get('index-open').hidden=true;
const node = id=>nodes.get(id);
const bundle='d'.repeat(40), epoch='b'.repeat(64);let viewId='a'.repeat(64);
const snapshot={type:'snapshot',view_id:viewId,connection_epoch:epoch,dataset_revision:'1',state_rev:'1',
    render_rev:'1',render_key:'1',fill_slots_key:'c'.repeat(40),worker_epoch:'2',bbox_dbu:['-10.9375','0','89.0625','80'],camera_um:['39.0625','40','100'],
    dbu_um:'1',pixels:[100,80],depth:'full',max_depth:'2',detail:'high',thin:'auto',effective_thin:'cull',
    layers:{mode:'all'},layers_isolated:false,frames:false,labels:false,font_px:14,mono:false,status:'idle',source_stale:false,
    deck_skipped:'0',failure:null,submitted:'1',consumed:'1',discarded:'0',capabilities:{labels:true,clip:true}};
if(minimapEnabled){snapshot.minimap={size:180,base:'full',die:[0,0,180,180],marks:[[10,10,5,5,4]]};}
if(modeEnabled){snapshot.capabilities={labels:false,clip:false,mode:true};snapshot.effective_thin='keep';}
let open=false, lastSeq='0';
const layerRow={pair:[7,0],name:'MASK',aliases:[],parent:null,head:false,children:0,closed:false,visible:true,color:'#ffffff',fill:{kind:'solid'},width:1};
const paletteRows=[[3,1],[3,2],[3,300],[7,0]].map((pair,i)=>({...layerRow,pair,name:'MASK '+pair.join('/'),head:i===0,children:i===0?2:0,parent:i>0&&i<3?[3,1]:null}));
const document={hidden:false,activeElement:null,title:'',body:new Element('','body'),
    contains:n=>[...nodes.values()].includes(n),
    getElementById:node,querySelector:()=>({content:bundle}),createElement:tag=>new Element('',tag),
    createTextNode:text=>Object.assign(new Element(''),{textContent:text}),
    addEventListener:(k,f)=>listen(docListeners,k,f)};
class XHR {
    abort(){if(this.onabort){this.onabort();}}
    open(method,path){this.method=method;this.path=path;}
    setRequestHeader(k,v) {if(!this.headers)this.headers={};this.headers[k]=v;}
    getResponseHeader() {return this.path.includes('/settings/')&&this.method==='GET'?'text/plain; charset=utf-8':'application/json';}
    send(text){
        const settingsPath=this.path.includes('/settings/');
        const raw=this.path.endsWith('/transfer/chunk');
        const body=text===null?null:settingsPath||raw?text:JSON.parse(text); requests.push(Object.assign({method:this.method,path:this.path,body},raw?{headers:this.headers}:{}));
        let value, status=200;
        if(raw){value={kind:'drc_review_transfer',phase:'queued',seq:this.headers['X-Floe-Transfer-Seq']};status=202;}
        else if (this.path==='/api/v1/session/exchange') {value={csrf:'c'.repeat(64),session_id:'c'.repeat(64),bundle,protocol:1};}
        else if (this.path==='/api/v1/session'&&this.method==='DELETE') {value=exitFailure?{error:'unavailable'}:null;status=exitFailure?503:204;}
        else if(startupEnabled&&this.path==='/api/v1/views/'+viewId&&this.method==='DELETE'){open=false;value=null;status=204;}
        else if(this.path==='/api/v1/capabilities') {value={protocol:1,bundle,index_open:indexOpenEnabled,launcher:launchEnabled,exports:clipEnabled,snapshot_png:snapshotEnabled,layer_settings:settingsEnabled,design_defaults:defaultsEnabled,jobdeck_modes:modeEnabled,fill_slot_edit:fillEditorEnabled};}
        else if(launchEnabled&&this.path==='/api/v1/launch'){value=launchState;}
        else if(launchEnabled&&this.path.startsWith('/api/v1/launch/poll/')){launchPolls.push(this);return;}
        else if(launchEnabled&&this.path.startsWith('/api/v1/launch/')){
            if(this.method==='POST'){
                if(body.action==='open'){
                    open=true;lastSeq=body.seq;snapshot.state_rev=P.next(snapshot.state_rev);
                    value=launchReceipt={phase:'submitted',operation:{seq:lastSeq}};
                }else{value=launchReceipt={phase:body.action==='present'?'presented':'dismissed'};}
                launchState={revision:P.next(launchState.revision),pending:null};
            }else{value={phase:launchReceipt?'submitted':'ready',receipt:launchReceipt};}
        }
        else if(this.path==='/api/v1/defaults/prepare') {value={token:'d'.repeat(64),view_id:body.view_id,state_rev:body.state_rev,name:'synthetic.oas.layerprops',title:'synthetic',mode:'level',levels:null,rows:1,bytes:'24',replaces_existing:false,expires_in_ms:'30000',scope:'shared_design_default',affects:'future_opens'};}
        else if(this.path==='/api/v1/defaults/revoke') {value=null;status=204;}
        else if(this.path==='/api/v1/defaults') {
            if(this.method==='POST'){defaultOp={seq:body.seq,kind:'design_default',phase:'succeeded',view_id:body.view_id,state_rev:body.state_rev,name:'synthetic.oas.layerprops',published:true,directory_synced:true};value=defaultOp;status=202;}
            else{value={available:true,kind:'design_default',scope:'shared_design_default',jobs:1,max_bytes:'4194304',operations:{last_seq:defaultOp?defaultOp.seq:'0',active:null,history:defaultOp?[defaultOp]:[]}};}
        }
        else if(settingsPath) {value=this.method==='POST'?{view_id:viewId,state_rev:'1',prepared_token:'d'.repeat(64),rows:0,malformed:0}:'{"format":"floe.layers"}';}
        else if(this.path==='/api/v1/exports') {
            if(this.method==='POST') {clipFile={id:body.seq,bytes:'64',expires_in_ms:'600000',name:'floe-clip-'+body.seq+'.oas'};
                clipOp={seq:body.seq,kind:'exact_clip',phase:'ready',view_id:body.view_id,artifact:{...clipFile,records:'2',bbox_dbu:['-11','0','89','80'],source_stale:false,available:true}};value=clipOp;
            }else{value=clipState();}
        }
        else if(this.method==='DELETE'&&this.path==='/api/v1/artifacts/1'){clipFile=null;clipOp.artifact.available=false;clipOp.artifact.expires_in_ms=null;value=null;status=204;}
        else if(indexOpenEnabled&&this.path.endsWith('/index-open')){value={...indexOperations[0].index_open,jobs_available:3};}
        else if(indexOpenEnabled&&/^\/api\/v1\/operations\/[0-9]+$/.test(this.path)){value=indexOperations.find(v=>v.seq===this.path.split('/').at(-1));}
        else if(this.path==='/api/v1/catalog') {value={sources:launchEnabled&&!launchRegistered?[]:[{source_id:indexOpenEnabled?indexSource:'src',title:'synthetic',deck:false,levels:0},{source_id:'deck',title:'synthetic deck',deck:true,levels:2}]};}
        else if(this.path==='/api/v1/catalog/deck/levels/0') {value={levels:[{id:'1',title:'Level 1'},{id:'2',title:'Level 2'}],next:null};}
        else if(this.path==='/api/v1/startup') {value={confirm_levels:startupEnabled,request:{kind:'open',seq:'1',source_id:modeEnabled||startupEnabled?'deck':'src',mode:modeEnabled?'chip':'level',levels:modeEnabled?{mode:'only',ids:['1']}:{mode:'all'},body:startupEnabled?startupBody:{detail:'high'},label_preference:false}};}
        else if(this.path==='/api/v1/operations'&&this.method==='POST') {
            open=true;lastSeq=body.seq;value={seq:lastSeq,kind:body.kind,phase:body.kind==='mode'?'preparing':'succeeded',view_id:viewId};status=202;
            if(body.kind==='mode') {assert(modeEnabled);modeOperation=value;}
            if(startupEnabled){open=body.kind==='open'&&!startupFail;startupReceipt=value={seq:lastSeq,kind:body.kind,phase:body.kind==='open'&&startupFail?'failed':'succeeded',view_id:open?viewId:null,error:body.kind==='open'&&startupFail?'index_required':null};}
            if(indexOpenEnabled){
                open=body.kind==='index_open';
                if(open){assert(storage.has('floe-index-open:'+'c'.repeat(64)),'approval must be saved before POST');value={seq:lastSeq,kind:'index_open',request_id:body.request_id,open_seq:body.open_seq,phase:'succeeded',stage:'open',view_id:viewId,index:{phase:'succeeded'}};}
                else{value={seq:lastSeq,kind:'open',phase:'failed',error:'index_unavailable',index_open:{open_seq:lastSeq,source_id:body.source_id,title:'synthetic',mode:body.mode,levels:body.levels,display_policy:body.display_policy||'explicit'}};}
                indexOperations.push(value);
            }
        }
        else if(this.path==='/api/v1/operations') {
            value={last_seq:lastSeq,active:modeOperation&&modeOperation.phase==='preparing'?lastSeq:null,history:modeOperation?[modeOperation]:open?[{seq:lastSeq,kind:'open',phase:'succeeded',view_id:viewId}]:[]};
            if(startupEnabled){value.history=startupReceipt?[startupReceipt]:[];}
            if(indexOpenEnabled){value={last_seq:lastSeq,active:null,history:indexOperations};}
            if(modeReadFailure){modeReadFailure=false;status=503;value={error:'unavailable'};}
        }
        else if(this.path==='/api/v1/view') {status=open?200:404;value=open?{title:'synthetic',source_id:indexOpenEnabled?indexSource:modeEnabled?'deck':'src',mode:modeEnabled?serverMode:'level',levels:modeEnabled?['1']:null,view:{...snapshot,connection_epoch:''}}:null;
            if(modeViewReadFailure){modeViewReadFailure=false;status=503;value={error:'unavailable'};}}
        else if(this.path.endsWith('/minimap/full')){value={view_id:viewId,dataset_revision:'1',base:'full',size:180,pixels:'0'.repeat(32400)};}
        else if(this.path.endsWith('/layers/0')) {value={state_rev:snapshot.state_rev,render_key:snapshot.render_key,total:1,start:0,next:null,rows:[layerRow]};}
        else if(this.path==='/api/v1/palette/presets') {value=presetFixture();}
        else if(this.path.includes('/fill-slots/')) {value={version:1,view_id:viewId,fill_slots_key:snapshot.fill_slots_key,editable:fillEditorEnabled,fills:liveFillRows};}
        else if(this.path.endsWith('/palette')) {
            assert.equal(body.kind,'page');
            const closed=p=>body.fold.closed!==body.fold.exceptions.some(v=>v.join('/')===p.join('/'));
            const rows=(paletteEnabled?paletteRows:[layerRow]).filter(r=>!r.parent||!closed(r.parent)).map(r=>({...r,closed:r.children>0&&closed(r.pair),visible:snapshot.layers.mode==='all'||snapshot.layers.mode==='only'&&snapshot.layers.pairs.some(p=>p.join('/')===r.pair.join('/'))}));
            value={state_rev:snapshot.state_rev,render_key:snapshot.render_key,total:rows.length,all_total:paletteEnabled?4:1,start:0,next:null,rows};
        }
        else {throw new Error('Unexpected HTTP '+this.path);}
        if(launchEnabled&&this.path==='/api/v1/startup'){value={request:null};}
        if(indexOpenEnabled&&this.path==='/api/v1/startup'){value.request.source_id=indexSource;}
        this.status=status;this.responseText=settingsPath&&this.method==='GET'?value:JSON.stringify(value);
        if(body&&body.kind==='mode'&&modeLosePost){modeLosePost=false;setImmediate(()=>this.ontimeout());return;}
        if(indexOpenEnabled&&body&&body.kind==='index_open'){setImmediate(()=>this.ontimeout());return;}
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
const window={FloeProtocol:P,FloeQuery:require('./query.js'),FloeInspect:require('./inspect.js'),FloeMeasure:require('./measure.js'),FloeSnapshot:require('./snapshot.js'),FloeClip:{...Clip,bind(o){clipController=Clip.bind(o);return clipController;}},FloeGestures:require('./gestures.js'),FloeRulers:require('./rulers.js'),FloeDRCGroups:require('./drc-groups.js'),FloeDRC:{...DRC,bind(o){
    drcOptions=o;const panel=DRC.bind(o),paint=panel.paint,changed=panel.contextChanged;
    panel.contextChanged=()=>{contextChanges++;changed();};panel.paint=(p,s)=>{drcDisplays.push({p,s});paint(p,s);};
    const click=panel.click;panel.click=(...v)=>{drcClicks.push(v);return consumeDRC || click(...v);};return panel;
}},FloePanelState:require('./panel-state.js'),FloeDRCBuild:require('./drc-build.js'),ResizeObserver:class {constructor(fn){this.fn=fn;observers.push(this);}observe(e){this.target=e;}disconnect(){this.target=null;}},devicePixelRatio:1,
    addEventListener:(k,f)=>listen(listeners,k,f),setTimeout,requestAnimationFrame:fn=>setTimeout(fn,0),cancelAnimationFrame:clearTimeout};
const storage=new Map();
window.isSecureContext=true;window.ClipboardItem=class {constructor(data){this.data=data;}};
window.FloeSettings=require('./settings.js');
window.FloeDefaults=require('./defaults.js');
window.FloeAbout=require('./about.js');
window.FloeSessionExit=require('./session-exit.js');
window.FloeNotices=require('./notices.js');
window.FloeMinimap=require('./minimap.js');
window.FloeLauncher=require('./launcher.js');
window.FloeBrowse=require('./browse.js');
window.FloeIndexOpen=require('./index-open.js');
window.FloePalette=require('./palette.js');
window.FloePresets=require('./presets.js');
window.FloeFillEditor=require('./fill-editor.js');
window.crypto={getRandomValues:a=>a.fill(37)};
window.FloeDRCNotes=require('./drc-notes.js');
window.FloeDRCNoteDisplay=require('./drc-note-display.js');
window.FloeDRCWaives=require('./drc-waives.js');
window.FloeDRCTransfer=require('./drc-transfer.js');
window.FileReader=class {readAsArrayBuffer(file){this.result=new TextEncoder().encode(file.text).buffer;setImmediate(()=>this.onload());}abort(){if(this.onabort){this.onabort();}}};
window.navigator={clipboard:{write(items){copies.push(items);return Promise.all(items.map(i=>i.data['image/png']));}}};
window.getSelection=()=>textSelection;
let captureUrl=0;const captureUrls=new Set();
window.URL={createObjectURL(){const u='blob:capture/'+ ++captureUrl;captureUrls.add(u);return u;},revokeObjectURL:u=>captureUrls.delete(u)};
const sandbox={window,document,XMLHttpRequest:XHR,WebSocket:Socket,Image,ImageData:class {constructor(data,w,h){this.data=data;this.width=w;this.height=h;}},
    location:{origin:'http://127.0.0.1:1234',hash:'#bootstrap='+'e'.repeat(64),pathname:'/'},
    history:{replaceState(){sandbox.location.hash='';}},sessionStorage:{getItem:k=>storage.get(k)||null,setItem:(k,v)=>storage.set(k,v),removeItem:k=>storage.delete(k)},
    URL:{createObjectURL(){const s='blob:test/'+images.length;urls.add(s);return s;},revokeObjectURL:s=>urls.delete(s)},
    Blob,TextEncoder,TextDecoder,ArrayBuffer,DataView,Uint8Array,Uint8ClampedArray,
    setTimeout:function(fn,ms){assert(!this||!this.context,'unbound Window timer receiver');return setTimeout(fn,modeEnabled?Math.min(ms,5):ms);},
    clearTimeout:function(id){assert(!this||!this.context,'unbound Window timer receiver');return clearTimeout(id);},
    setInterval:()=>0,Date:{now:()=>freezeTime?clock:(clock+=100)},console};
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
        final:true,partial:false,deferred:'0',labels_truncated:false,complete:true,approximate:false,query:true,
        query_scene:{generation:id,round:'1',complete:true,summary_layers:'0'},perf:{},...extra};
    const text=new TextEncoder().encode(JSON.stringify(header)),out=new Uint8Array(4+text.length+data.length);
    new DataView(out.buffer).setUint32(0,text.length,true);out.set(text,4);out.set(data,4+text.length);
    return out.buffer;
}
(async()=>{
    if(fillEditorEnabled){
        await wait(()=>sockets.length===1);let ws=sockets[0];hello(ws);ws.receive(packet('raw','1'));
        await wait(()=>node('layers').children.length&&!node('layers').children[0].children[3].disabled);
        node('palette-presets').open=true;node('palette-presets').ontoggle();await wait(()=>!node('fill-slot-open').disabled);
        const writes=()=>sockets.flatMap(s=>s.sent.filter(m=>m.type==='view.fill_slot'));
        const edit=()=>{node('fill-slot-choice').value='brick';node('fill-slot-open').onclick();assert(!node('fill-slot-editor').hidden);};
        const apply=()=>node('fill-slot-editor').onsubmit({preventDefault(){}});
        const slotReads=()=>requests.filter(q=>q.path.includes('/fill-slots/')).length;
        assert.equal(node('layers-selected').textContent,'0 selected');assert.equal(slotReads(),1);
        edit();node('fill-slot-clear').onclick();node('fill-slot-cancel').onclick();assert.equal(writes().length,0);
        edit();node('fill-slot-invert').onclick();apply();apply();await wait(()=>writes().length===1);
        const first=writes()[0],bits=presetFixture().fills[15].rows.map(n=>n^65535);
        assert.deepEqual(first.body,{name:'brick',rows:bits});assert.equal(first.base_state_rev,'1');assert.equal(first.connection_epoch,epoch);assert.equal(first.view_id,viewId);
        assert(node('fill-slot-apply').disabled);assert(!node('fill-slot-editor').hidden);
        ws.receive({type:'accepted',seq:first.seq,state_rev:'2',render_rev:'1'});assert(!node('fill-slot-editor').hidden,'ACK was treated as authoritative state');
        liveFillRows[15].rows=bits;snapshot.state_rev='2';snapshot.fill_slots_key='d'.repeat(40);ws.receive(snapshot);
        await wait(()=>node('fill-slot-editor').hidden&&!node('fill-slot-open').disabled);assert.equal(slotReads(),2);
        assert.equal(snapshot.render_rev,'1','unused bitmap edit needs no geometry generation');
        // Idle camera/state changes preserve table identity and do not refetch.
        snapshot.state_rev='3';snapshot.render_rev='2';ws.receive(snapshot);assert.equal(slotReads(),2);
        edit();freezeTime=true;node('fill-slot-reset').onclick();apply();const sent=writes().length;
        // The write is rate-limited in the real app queue. Another state wins
        // before send; the queued draft must keep base=3, never rebase to 4.
        snapshot.state_rev='4';ws.receive(snapshot);freezeTime=false;await new Promise(r=>setTimeout(r,90));await wait(()=>writes().length===sent+1);
        const stale=writes().at(-1);assert.equal(stale.base_state_rev,'3');assert.deepEqual(stale.body.rows,presetFixture().fills[15].rows);
        ws.receive({type:'error',seq:stale.seq,code:'stale_state'});ws.receive(snapshot);await wait(()=>node('fill-slot-editor').hidden);
        assert.match(node('fill-slot-status').textContent,/not replayed/);const afterFailure=writes().length;ws.receive(snapshot);assert.equal(writes().length,afterFailure);
        edit();node('fill-slot-clear').onclick();snapshot.state_rev='5';ws.receive(snapshot);assert(node('fill-slot-editor').hidden);apply();assert.equal(writes().length,afterFailure);
        // Ordinary swatches bind the name, not the edited pixel value.
        await wait(()=>!node('layers').children[0].children[3].disabled);
        node('layers').children[0].children[3].onclick({button:0,detail:1});await wait(()=>!node('presets-fills').children[15].disabled);
        node('presets-fills').children[15].onclick();const assigned=ws.sent.filter(m=>m.type==='view.set').at(-1);assert.equal(assigned.body.style_batch.fill_slot,'brick');assert(!assigned.body.style_batch.fill);
        ws.receive({type:'accepted',seq:assigned.seq,state_rev:'6',render_rev:'3'});snapshot.state_rev='6';snapshot.render_rev='3';snapshot.render_key='2';ws.receive(snapshot);await wait(()=>!node('fill-slot-open').disabled);
        edit();node('fill-slot-clear').onclick();apply();const uncertain=writes().at(-1);ws.close();assert(node('fill-slot-editor').hidden);
        const disconnected=writes().length;await new Promise(r=>setTimeout(r,550));await wait(()=>sockets.length===2);ws=sockets[1];hello(ws,'f'.repeat(64));await wait(()=>!node('fill-slot-open').disabled);
        assert.equal(writes().length,disconnected,'reconnect replayed a bitmap');assert.equal(slotReads(),3,'new connection must revalidate slots');
        sockets[0].receive({type:'accepted',seq:uncertain.seq,state_rev:'7',render_rev:'4'});assert(node('fill-slot-editor').hidden);
        edit();listeners.pagehide();assert(node('fill-slot-editor').hidden);assert.equal(writes().length,disconnected);
        assert(!Array.from(storage.keys()).some(k=>/bitmap|fill-slot/.test(k)));
        console.log('WEB FILL SLOT CLIENT: ALL OK (real app queue, slot references, ACK/snapshot, captured CAS, no rebase/replay, reconnect/cache/cleanup)');return;
    }
    if(paletteEnabled){
        await wait(()=>sockets.length===1);const ws=sockets[0];hello(ws);ws.receive(packet('raw','1'));
        const row=k=>node('layers').children.find(r=>r.children[3].children[0].dataset.pair===k);
        const click=(k,extra={})=>row(k).children[3].onclick({button:0,detail:1,...extra});
        const edits=()=>ws.sent.filter(m=>m.type==='view.set');
        await wait(()=>node('layers').children.length===4&&!row('3/1').children[3].disabled);
        const paint=draws.length;click('3/1');click('3/2',{ctrlKey:true});click('3/300',{shiftKey:true});click('3/1',{ctrlKey:true});
        assert.equal(node('layers-selected').textContent,'3 selected');assert.equal(edits().length,0);
        node('layers-collapse').onclick();await wait(()=>node('layers').children.length===2&&!node('layers-hide').disabled);
        assert.equal(draws.length,paint,'palette selection/folding repainted geometry');
        assert.equal(edits().length,0,'palette selection/folding submitted rendering');
        row('7/0').oncontextmenu({button:2,preventDefault(){},clientX:20,clientY:20});
        assert.equal(node('layers-selected').textContent,'3 selected');node('layer-menu-close').onclick();
        node('layers-hide').onclick();assert.equal(edits().length,1);
        const hide=edits()[0];assert.equal(hide.base_state_rev,'1');
        assert.deepEqual(hide.body,{layer_batch:{action:'hide',pairs:[[3,1],[3,2],[3,300]],collapsed:[[3,1]]}});
        assert.equal(node('layers-show').disabled,true);
        ws.receive({type:'accepted',seq:hide.seq,state_rev:'2',render_rev:'2'});
        assert.equal(node('layers-show').disabled,true,'ACK alone released the palette edit');
        snapshot.state_rev='2';snapshot.render_rev='2';snapshot.render_key='2';snapshot.layers={mode:'only',pairs:[[7,0]]};ws.receive(snapshot);
        await wait(()=>!node('layers-show').disabled);
        assert.equal(row('3/1').children[0].checked,false);assert.equal(row('7/0').children[0].checked,true);
        assert.equal(node('layers-selected').textContent,'3 selected');
        const stale=row('3/1').children[0];
        node('layers-toggle').onclick();assert.equal(edits().length,2);
        const toggle=edits()[1];assert.equal(toggle.base_state_rev,'2');
        ws.receive({type:'error',seq:toggle.seq,code:'stale_state',state_rev:'2',render_rev:'2'});ws.receive(snapshot);
        await wait(()=>!node('layers-show').disabled);assert.match(node('layers-note').textContent,/not replayed|changed/i);
        assert.equal(edits().length,2,'rejected palette edit was replayed');
        snapshot.state_rev='3';snapshot.render_rev='3';snapshot.render_key='3';ws.receive(snapshot);
        stale.checked=true;stale.onchange();assert.equal(edits().length,2,'stale row submitted visibility');
        await wait(()=>!node('layers-show').disabled);
        const reads=requests.filter(r=>r.path.endsWith('/palette')).length;ws.receive(snapshot);
        assert.equal(requests.filter(r=>r.path.endsWith('/palette')).length,reads);
        const beforeStylePaint=draws.length;node('layers-style').onclick();
        assert.equal(draws.length,beforeStylePaint);assert.equal(edits().length,2);
        node('palette-color-on').checked=true;node('palette-color').value='#22aa88';node('palette-fill').value='clear';node('palette-width').value='+1';
        node('palette-style').onsubmit({preventDefault(){}});await wait(()=>edits().length===3);
        const style=edits()[2];assert.equal(style.base_state_rev,'3');
        assert.deepEqual(style.body,{style_batch:{pairs:[[3,1],[3,2],[3,300]],collapsed:[[3,1]],color:'#22aa88',fill:{kind:'clear'},width_step:1}});
        ws.receive({type:'accepted',seq:style.seq,state_rev:'4',render_rev:'4'});
        assert.equal(node('layers-style').disabled,true,'style ACK alone enabled writes');
        paletteRows.slice(0,3).forEach(r=>{r.color='#22aa88';r.fill={kind:'clear'};r.width=2;});
        snapshot.state_rev='4';snapshot.render_rev='4';snapshot.render_key='4';ws.receive(snapshot);
        await wait(()=>!node('layers-style').disabled);assert.equal(row('3/1').children[1].value,'#22aa88');
        // Opening the immutable palette performs one read, never a render.
        const count=edits().length,paintBeforePresets=draws.length;
        node('palette-presets').open=true;node('palette-presets').ontoggle();
        await wait(()=>node('presets-colors').children.length===49&&!node('presets-fills').hidden);
        assert.equal(node('presets-fills').children.length,20);
        assert.equal(edits().length,count);assert.equal(draws.length,paintBeforePresets);
        node('presets-colors').children[8].onclick();await wait(()=>edits().length===count+1);
        const recolor=edits().at(-1);
        assert.deepEqual(recolor.body,{style_batch:{pairs:[[3,1],[3,2],[3,300]],collapsed:[[3,1]],color:'#ffff00'}});
        assert(node('presets-fills').children.every(b=>b.disabled));
        ws.receive({type:'accepted',seq:recolor.seq,state_rev:'5',render_rev:'5'});
        assert(node('presets-fills').children.every(b=>b.disabled));
        snapshot.state_rev='5';snapshot.render_rev='5';snapshot.render_key='5';ws.receive(snapshot);
        await wait(()=>!node('presets-fills').children[15].disabled);
        node('presets-fills').children[15].onclick();await wait(()=>edits().length===count+2);
        const fill=edits().at(-1);
        assert.deepEqual(fill.body,{style_batch:{pairs:[[3,1],[3,2],[3,300]],collapsed:[[3,1]],fill_slot:'brick'}});
        ws.receive({type:'error',seq:fill.seq,code:'stale_state',state_rev:'5',render_rev:'5'});ws.receive(snapshot);
        await wait(()=>!node('layers-style').disabled);assert.equal(edits().length,count+2,'preset error replayed a write');
        assert.equal(requests.filter(r=>r.path==='/api/v1/palette/presets').length,1,'style change reread immutable presets');
        // Row editors share folded-group semantics, and stale rows cannot act.
        row('3/1').children.at(-1).onclick();node('style-width').value='7';
        node('layers-expand').onclick();await wait(()=>node('layers').children.length===4&&!node('layers-style').disabled);
        node('style-editor').onsubmit({preventDefault(){}});assert.equal(edits().length,count+2,'old folded editor survived a fold change');
        node('layers-collapse').onclick();await wait(()=>node('layers').children.length===2&&!node('layers-style').disabled);
        row('3/1').children.at(-1).onclick();node('style-width').value='7';node('style-editor').onsubmit({preventDefault(){}});
        assert.deepEqual(edits().at(-1).body,{style_batch:{pairs:[[3,1]],collapsed:[[3,1]],width:7}});
        const width=edits().at(-1);ws.receive({type:'accepted',seq:width.seq,state_rev:'6',render_rev:'6'});
        snapshot.state_rev='6';snapshot.render_rev='6';snapshot.render_key='6';ws.receive(snapshot);await wait(()=>!node('layers-style').disabled);
        row('3/1').children.at(-1).onclick();node('layers-style').onclick();assert.equal(node('style-editor').hidden,true);
        row('3/1').children.at(-1).onclick();assert.equal(node('palette-style').hidden,true);
        listeners.pagehide();assert.equal(node('layers-show').disabled,true);assert.equal(node('layer-menu').hidden,true);
        assert(node('presets-colors').children.every(b=>b.disabled));
        console.log('WEB PALETTE CLIENT: ALL OK (real selection/fold/presets, read without render, one-field CAS, ACK/snapshot, row fold parity, rejected/stale edits, cleanup)');return;
    }
    if(gotoEnabled){
        const fields=()=>['goto-x','goto-y','goto-width'].map(k=>node(k).value);
        const type=(k,v)=>{node(k).value=v;node(k).input({});};
        const submit=()=>node('goto-form').onsubmit({preventDefault(){}});
        await wait(()=>sockets.length===1);let ws=sockets[0];hello(ws);
        assert.deepEqual(fields(),['39.0625','40','100'],'first open restores server decimal strings');
        node('goto-x').focus();snapshot.state_rev='2';snapshot.render_rev='2';snapshot.camera_um=['-10.9375','6','300'];ws.receive(snapshot);
        assert.deepEqual(fields(),['39.0625','40','100'],'focus prevents mixed coordinate replacement');
        type('goto-x','123.');document.activeElement=null;ws.receive(snapshot);
        assert.deepEqual(fields(),['123.','40','100'],'blur does not silently discard the draft');
        listeners.pagehide();listeners.pageshow({persisted:true});await wait(()=>sockets.length===2);ws=sockets[1];hello(ws);
        assert.deepEqual(fields(),['123.','40','100'],'same-view reconnect preserves edited inputs');
        let escaped=false;node('goto-x').keydown({key:'Escape',isComposing:true,preventDefault(){escaped=true;}});assert(!escaped);
        const wireCount=ws.sent.length;node('goto-x').keydown({key:'Escape',preventDefault(){escaped=true;}});
        assert(escaped);assert.equal(ws.sent.length,wireCount,'Escape restores text, never navigates');
        assert.deepEqual(fields(),snapshot.camera_um);
        type('goto-width','bad');submit();assert.equal(ws.sent.length,wireCount);
        type('goto-width','300');type('goto-x','-2.5');submit();let sent=ws.sent.at(-1);
        assert.deepEqual(sent.body.navigation,{kind:'goto',center_um:['-2.5','6'],width_um:'300'});
        ws.receive({type:'error',seq:sent.seq,code:'stale_state'});ws.receive(snapshot);
        assert.deepEqual(fields(),['-2.5','6','300'],'rejected goto preserves retryable user text');
        submit();sent=ws.sent.at(-1);snapshot.state_rev='3';snapshot.render_rev='3';snapshot.camera_um=['-2.5','6','300'];
        ws.receive({type:'accepted',seq:sent.seq,state_rev:'3',render_rev:'3'});ws.receive(snapshot);
        snapshot.state_rev='4';snapshot.render_rev='4';snapshot.camera_um=['-0.125','2','150'];ws.receive(snapshot);
        assert.deepEqual(fields(),snapshot.camera_um,'successful goto releases the draft for later pan/zoom');
        type('goto-x','8');submit();sent=ws.sent.at(-1);type('goto-x','88.');
        snapshot.state_rev='5';snapshot.render_rev='5';snapshot.camera_um=['8','2','150'];
        ws.receive({type:'accepted',seq:sent.seq,state_rev:'5',render_rev:'5'});ws.receive(snapshot);
        assert.equal(node('goto-x').value,'88.','late acceptance cannot overwrite a newer edit');
        node('goto-x').keydown({key:'Escape',preventDefault(){}});assert.deepEqual(fields(),snapshot.camera_um);
        ws.receive({...snapshot,state_rev:'4',camera_um:['999','999','999']});assert.deepEqual(fields(),snapshot.camera_um,'stale snapshot is ignored');
        type('goto-x','discard-on-new-source');node('goto-x').focus();listeners.pagehide();viewId='9'.repeat(64);snapshot.view_id=viewId;
        snapshot.camera_um=['1.0000000000000002','-0.0000000001','0.125'];
        listeners.pageshow({persisted:true});await wait(()=>sockets.length===3);ws=sockets[2];hello(ws);
        assert.deepEqual(fields(),snapshot.camera_um,'new view replaces old source draft without rounding');
        document.activeElement=null;
        snapshot.camera_um=null;ws.receive(snapshot);assert.deepEqual(fields(),['','',''],'unrepresentable units do not restore arbitrary defaults');
        snapshot.camera_um=['1e-198','0','1e-197'];ws.receive(snapshot);assert.deepEqual(fields(),snapshot.camera_um);
        ws.receive(packet('raw','1',snapshot.state_rev));assert(draws.length);
        assert.equal(requests.filter(r=>r.method==='POST'&&r.path==='/api/v1/operations').length,1,'recovery does not reopen or index');
        listeners.pagehide();console.log('WEB GOTO CLIENT: ALL OK (server strings, focus/draft, reconnect, Escape/IME, invalid/rejected/successful input, late ACK, stale snapshot, new source, pixels)');return;
    }
    if(indexOpenEnabled){
        const commands=()=>requests.filter(r=>r.method==='POST'&&r.path==='/api/v1/operations');
        await wait(()=>!node('index-open').hidden&&!node('index-open').disabled);
        assert.equal(commands().length,1);assert.equal(sockets.length,0);
        await node('index-open').onclick();assert.match(node('index-open-preview').textContent,/synthetic.*\nSource ID:/);
        assert.equal(commands().length,1);assert.equal(node('index-open-jobs').value,'3');assert(node('source').disabled);
        node('index-open-jobs').value='2';node('index-open-lod').checked=true;
        await node('index-open-approve').onclick();
        assert.equal(commands().length,2);assert.equal(commands()[1].body.kind,'index_open');assert.equal(commands()[1].body.open_seq,'1');
        assert.equal(commands()[1].body.approved,true);assert.deepEqual(commands()[1].body.options,{jobs:2,force:false,lod:true,occupancy:true});
        assert.deepEqual(commands()[1].body.target,{kind:'empty'});assert.deepEqual(commands()[1].body.pixels,[100,80]);
        assert(storage.has('floe-index-open:'+'c'.repeat(64)));assert(node('index').disabled);assert(node('source').disabled);assert.equal(sockets.length,0);
        node('cancel-job').dataset.seq='99';const cancelReads=requests.length;node('cancel-job').onclick();
        assert.equal(requests.length,cancelReads,'unresolved approval only uses its identity-bound cancel');
        node('index-open-close').onclick();assert(node('source').disabled,'hiding unresolved dialog cannot unlock mutation');
        listeners.pagehide();listeners.pageshow({persisted:true});
        await wait(()=>sockets.length===1&&!storage.has('floe-index-open:'+'c'.repeat(64)));
        assert.equal(commands().length,2,'BFCache recovery must only read the original receipt');
        hello(sockets[0]);sockets[0].receive(packet('raw','1'));assert(draws.length);
        assert.match(node('operation').textContent,/Index succeeded.*View attached/);
        assert(node('notice').hidden,'successful indexed open must clear the earlier missing-index notice');
        const edits=sockets[0].sent.length;node('fit').onclick();assert.equal(sockets[0].sent.length,edits,'approval modal gates viewport edits');
        node('index-open-close').onclick();assert(!node('source').disabled);assert(node('index-open').hidden);
        listeners.pagehide();
        console.log('WEB INDEX OPEN CLIENT: ALL OK (failed startup, read-only preview, original approval, disabled conflicts, lost response/BFCache recovery, no duplicate POST, restored pixels)');
        return;
    }
    if(launchEnabled){
        const posts=()=>requests.filter(r=>r.method==='POST'&&r.path.startsWith('/api/v1/launch/'));
        async function push(id,confirm=false){
            await wait(()=>launchPolls.length);
            launchRegistered=true;launchReceipt=null;
            launchState={revision:P.next(launchState.revision),pending:{id:id.repeat(64),phase:'ready',confirm_levels:confirm,
                request:{kind:'open',seq:'1',source_id:confirm?'deck':'src',mode:'level',levels:{mode:'all'},body:{navigation:{kind:'goto',center_um:['1','2'],width_um:'700'}}}}};
            const xhr=launchPolls.shift();xhr.status=200;xhr.responseText=JSON.stringify(launchState);xhr.onload();
        }
        await wait(()=>launchPolls.length);
        assert(node('open').disabled&&node('index').disabled);assert.equal(sockets.length,0);
        node('notice').textContent='previous source error';node('notice').hidden=false;
        await push('a');await wait(()=>sockets.length===1);hello(sockets[0]);
        assert.deepEqual(posts()[0].body,{action:'open',seq:'1',pixels:[100,80],levels:{mode:'all'}});
        assert(node('notice').hidden,'previous source error survived successful handoff');
        assert(!requests.some(r=>r.method==='POST'&&r.path==='/api/v1/operations'));
        await push('b',true);await wait(()=>!node('launch-open').hidden);
        assert.equal(posts().length,1);assert(node('source').disabled&&node('open').disabled);
        await wait(()=>node('level-list').querySelectorAll('input').length===2);
        node('levels-all').checked=false;node('levels-all').onchange();
        const selected=node('level-list').querySelectorAll('input')[1];selected.checked=true;selected.onchange();
        listeners.pagehide();listeners.pageshow({persisted:true});await wait(()=>sockets.length===2);hello(sockets[1]);
        assert.equal(node('source').value,'deck','live view restore replaced pending CLI source');
        assert.equal(node('goto-x').value,'1','live snapshot replaced pending CLI goto');
        assert(!node('levels-all').checked&&selected.checked,'reconnect discarded explicit levels');
        await wait(()=>!node('launch-open').disabled);
        node('launch-open').onclick();await wait(()=>posts().length===2&&node('launch-panel').hidden);
        assert.equal(posts()[1].body.seq,'2');assert.equal(posts()[1].body.view_id,viewId);
        assert.equal(posts()[1].body.state_rev,'2');assert.equal(snapshot.state_rev,'3');
        assert.deepEqual(posts()[1].body.levels,{mode:'only',ids:['2']});
        assert.equal(sockets.length,2,'handoff must not independently restart a frame stream');
        listeners.pagehide();console.log('WEB LAUNCH CLIENT: ALL OK (empty workspace, catalog refresh, one measured open, level approval, same-view CAS, no ordinary open)');return;
    }
    if(startupEnabled){
        const commands=()=>requests.filter(r=>r.method==='POST'&&r.path==='/api/v1/operations');
        await wait(()=>node('level-options').open);
        assert.equal(commands().length,0);assert.equal(sockets.length,0);
        assert.equal(node('source').value,'deck');assert.equal(node('goto-x').value,'1.25');
        await wait(()=>node('level-list').querySelectorAll('input').length===2);
        node('levels-all').checked=false;node('levels-all').onchange();
        const chosen=node('level-list').querySelectorAll('input')[1];chosen.checked=true;chosen.onchange();
        node('open').onclick();await wait(()=>commands().length===1&&!node('open').disabled);
        assert.deepEqual(commands()[0].body.levels,{mode:'only',ids:['2']});
        assert.deepEqual(commands()[0].body.body,{...startupBody,pixels:[100,80]});
        assert.equal(sockets.length,0,'failed open started a frame stream');
        node('index-jobs').value='2';node('index').onclick();
        await wait(()=>commands().length===2&&!node('open').disabled);
        assert.equal(commands()[1].body.kind,'index');
        startupFail=false;node('open').onclick();await wait(()=>sockets.length===1);
        assert.deepEqual(commands()[2].body.body,{...startupBody,pixels:[100,80]});
        assert.equal(commands()[2].body.display_policy,'explicit','CLI retry must not inherit window defaults');
        assert.equal(commands()[2].body.label_preference,false,'CLI label preference must survive index retry');
        assert(!Object.hasOwn(startupBody,'pixels'),'startup request mutated');
        hello(sockets[0]);sockets[0].receive(packet('raw','1'));
        await node('close').onclick();
        await wait(()=>!node('open').disabled);
        node('source').value='src';node('source').onchange();node('open').onclick();
        await wait(()=>commands().length===4);
        assert.deepEqual(commands()[3].body.body,{pixels:[100,80]},'startup leaked to a new source');
        assert.equal(commands()[3].body.display_policy,'window','manual file open preserves window display');
        listeners.pagehide();
        console.log('WEB STARTUP CLIENT: ALL OK (level consent, no implicit work, selected levels, index retry preserves CLI, no source leak)');
        return;
    }
    await wait(()=>sockets.length===1);
    assert.equal(sandbox.location.hash,'');
    const opened=requests.filter(r=>r.method==='POST'&&r.path==='/api/v1/operations');
    assert.equal(opened.length,1);assert.deepEqual(opened[0].body.body.pixels,[100,80]);
    const ws=sockets[0];hello(ws);
    ws.receive(packet('raw','1'));
    assert.equal(draws.length,1);assert.deepEqual(draws[0].data.slice(0,4),[16,0,127,255]);
    assert.equal(ws.sent.at(-1).disposition,'displayed');
    assert.equal(node('canvas').style.width,'100px');
    if(modeEnabled){
        const commands=()=>requests.filter(r=>r.method==='POST'&&r.path==='/api/v1/operations'&&r.body.kind==='mode');
        const key=(extra={})=>{let used=false;node('viewport').keydown({key:',',ctrlKey:true,target:node('viewport'),preventDefault(){used=true;},...extra});return used;};
        assert(!node('live-mode-row').hidden);assert(!node('live-mode').disabled);assert.equal(node('live-mode').value,'chip');
        for(const extra of [{repeat:true},{metaKey:true},{altKey:true},{shiftKey:true},{isComposing:true},{target:node('goto-x')}])assert(!key(extra));
        const camera=JSON.stringify(snapshot.bbox_dbu);
        async function complete(mode,failed=false){
            const prior=sockets.length;
            if(failed){modeOperation={...modeOperation,phase:'failed',error:'stale_state'};}
            else{serverMode=mode;viewId=String(++modeNumber).repeat(64);snapshot.view_id=viewId;snapshot.dataset_revision=String(modeNumber+1);snapshot.worker_epoch=String(modeNumber+2);modeOperation={...modeOperation,phase:'succeeded',view_id:viewId};}
            await wait(()=>sockets.length>prior);const next=sockets.at(-1);hello(next);next.receive(packet('raw',String(modeNumber+2)));
            await wait(()=>!node('live-mode').disabled);assert.equal(node('live-mode').value,serverMode);
            assert.equal(JSON.stringify(snapshot.bbox_dbu),camera);return next;
        }
        // Open-source selection is independent: the live mode command has
        // only the displayed view/revision, never levels/source paths.
        node('source').value='src';node('source').onchange();
        node('live-mode').value='layer';node('live-mode').onchange();
        await wait(()=>commands().length===1&&node('live-mode').disabled);
        assert.deepEqual(commands()[0].body,{kind:'mode',view_id:'a'.repeat(64),base_state_rev:'1',mode:'layer',seq:'2'});
        const sends=ws.sent.length;key();node('viewport').keydown({key:'ArrowUp',target:node('viewport'),preventDefault(){}});
        assert.equal(commands().length,1);assert.equal(ws.sent.length,sends,'input leaked during owner operation');
        const next=await complete('layer');assert.equal(node('levels-all').checked,false);
        assert.equal(node('source').value,'deck');assert(!node('labels').checked);
        const count=commands().length;node('live-mode').onchange();await new Promise(setImmediate);assert.equal(commands().length,count,'same-mode generated a request');
        modeLosePost=true;modeReadFailure=true;assert(key());
        // A failed status preflight never sends a mutation. Repeating is an
        // explicit user action after read-only reconciliation, not an auto retry.
        await wait(()=>!node('live-mode').disabled);assert.equal(commands().length,count);
        modeReadFailure=true;
        node('live-mode').value='level';node('live-mode').onchange();
        await wait(()=>!node('live-mode').disabled);assert.equal(commands().length,count);
        assert(key());await wait(()=>commands().length===count+1);
        assert.equal(commands().at(-1).body.mode,'level');
        const reads=requests.filter(r=>r.path==='/api/v1/view').length;
        modeViewReadFailure=true;
        await complete('level');assert.equal(commands().length,count+1,'lost POST was automatically replayed');
        assert(requests.filter(r=>r.path==='/api/v1/view').length>=reads+2,'terminal restore was not retried read-only');
        assert(key());await wait(()=>commands().length===count+2);assert.equal(commands().at(-1).body.mode,'chip');
        await complete('chip',true);assert.match(node('notice').textContent,/changed/);
        assert.equal(serverMode,'level');assert.equal(commands().length,count+2);
        next.receive(packet('raw','99','1',epoch,{view_id:'a'.repeat(64)}));
        listeners.pagehide();console.log('WEB DECK MODE CLIENT: ALL OK (scope/CAS, dropdown/Ctrl+, pending input/duplicates, raw/level/chip, stale/unknown reconciliation, no automatic replay)');return;
    }
    if(exitEnabled){
        const n=requests.length,commands=ws.sent.length;
        const key=(k,extra={})=>{let used=false;node('viewport').keydown({key:k,target:node('viewport'),preventDefault(){used=true;},...extra});return used;};
        for(const extra of [{ctrlKey:true},{metaKey:true},{altKey:true},{shiftKey:true},{isComposing:true},{target:node('goto-x')}])assert(!key('q',extra));
        node('viewport').focus();assert(key('q'));assert(!node('session-exit-dialog').hidden);assert.equal(document.activeElement,node('session-exit-cancel'));
        assert.equal(requests.length,n);assert.equal(ws.sent.length,commands);assert.equal(ws.readyState,1);
        node('session-exit-cancel').onclick();assert(node('session-exit-dialog').hidden);assert.equal(document.activeElement,node('viewport'));
        assert.equal(requests.length,n);assert(key('q'));docListeners.keydown({key:'Escape',preventDefault(){},stopPropagation(){}});
        assert(node('session-exit-dialog').hidden);assert.equal(requests.length,n);
        node('logout').onclick();assert(!node('session-exit-dialog').hidden);assert.equal(requests.length,n);
        storage.set('floe-default-pending','synthetic existing recovery record');
        const ending=node('session-exit-confirm').onclick();node('session-exit-confirm').onclick();await ending;
        assert.equal(requests.filter(r=>r.method==='DELETE'&&r.path==='/api/v1/session').length,1);
        assert.equal(node('connection').textContent,exitFailure?'Server shutdown unconfirmed':'Session ended');
        assert.equal(storage.has('floe-default-pending'),exitFailure);assert.equal(storage.has('floe-session:'+sandbox.location.origin),exitFailure);
        if(exitFailure){assert.match(node('notice').textContent,/No automatic retry/);}
        assert(node('logout').disabled);assert(node('session-exit-dialog').hidden);
        listeners.pagehide();console.log('WEB SESSION EXIT CLIENT: ALL OK (q/button wiring, modifiers, cancel no HTTP/WS, confirmed single DELETE, cleanup)');return;
    }
    if(minimapEnabled){
        await wait(()=>node('minimap-note').textContent==='Die outline · current view');
        const n=requests.length;
        node('minimap').mousedown({button:0,buttons:1,detail:1,clientX:25,clientY:20,preventDefault(){}});
        const move=ws.sent.at(-1);assert.equal(move.type,'view.set');assert.equal(move.base_state_rev,'1');
        assert.deepEqual(move.body.navigation,{kind:'minimap',point:[45,45]});assert.equal(requests.length,n,'click requested geometry/another base over HTTP');
        snapshot.state_rev='2';snapshot.render_rev='2';snapshot.minimap.marks=[[20,20,5,5,4]];
        ws.receive({type:'accepted',seq:move.seq,state_rev:'2',render_rev:'2'});ws.receive(snapshot);
        await new Promise(setImmediate);assert.equal(requests.length,n,'pan refetched minimap');
        listeners.pagehide();assert(node('minimap-panel').hidden);
        console.log('WEB MINIMAP CLIENT: ALL OK (real app wiring, CAS navigation, no per-pan GET, pagehide cleanup)');return;
    }
    const uploadBlob={size:16},uploadContext={drc_id:'1'.repeat(64),revision:'2'.repeat(64),view_id:viewId};
    await drcOptions.transferChunk('notes',{context:uploadContext,token:'3'.repeat(64),seq:'9007199254740993'},1048576,uploadBlob,{});
    const rawUpload=requests.at(-1);assert.equal(rawUpload.body,uploadBlob);assert.equal(rawUpload.headers['Content-Type'],'application/octet-stream');
    assert.equal(rawUpload.headers['X-Floe-CSRF'],'c'.repeat(64));assert.equal(rawUpload.headers['X-Floe-Transfer-Offset'],'1048576');assert.equal(rawUpload.headers['X-Floe-Transfer-Seq'],'9007199254740993');
    assert.equal(rawUpload.headers['X-Floe-DRC'],uploadContext.drc_id);assert.equal(rawUpload.headers['X-Floe-Revision'],uploadContext.revision);assert.equal(rawUpload.headers['X-Floe-View'],viewId);
    await assert.rejects(drcOptions.transferChunk('notes',{},0,{size:1048577},{}));
    assert.equal(node('snapshot-panel').hidden,!snapshotEnabled);
    assert.equal(node('settings-panel').hidden,!settingsEnabled);
    assert.equal(node('default-panel').hidden,!defaultsEnabled);
    if(defaultsEnabled){
        assert(!node('default-prepare').disabled);node('default-prepare').onclick();await wait(()=>!node('default-review').hidden);
        assert.equal(requests.filter(r=>r.path==='/api/v1/defaults'&&r.method==='POST').length,0);assert(node('default-approve').disabled);
        node('default-consent').checked=true;node('default-consent').onchange();node('default-approve').onclick();
        await wait(()=>node('default-status').textContent.startsWith('Published'));
        assert.equal(requests.filter(r=>r.path==='/api/v1/defaults'&&r.method==='POST').length,1);assert.equal(storage.get('floe-default-pending'),undefined);
        assert.equal(draws.length,1,'shared publication redrew native pixels');assert.equal(ws.sent.filter(m=>m.type==='view.set').length,0);
        await wait(()=>!node('default-prepare').disabled);node('default-prepare').onclick();await wait(()=>!node('default-review').hidden);
        node('default-review').onkeydown({key:'Escape',preventDefault(){},stopPropagation(){}});assert(node('default-review').hidden);
    }
    if(settingsEnabled){
        assert(!node('settings-load').disabled);node('settings-load').onclick();node('settings-file').files=[{size:0,text:''}];node('settings-file').onchange();
        await wait(()=>ws.sent.some(m=>m.type==='view.apply'));const command=ws.sent.find(m=>m.type==='view.apply');assert.equal(command.token,'d'.repeat(64));assert.equal(command.body,undefined);
        assert.equal(requests.filter(r=>r.path.includes('/settings/')&&r.method==='POST').length,1);
        ws.receive({type:'accepted',seq:command.seq,state_rev:'1',render_rev:'1'});ws.receive(snapshot);await wait(()=>node('settings-status').textContent.startsWith('Settings applied'));
        node('settings-format').value='native';node('settings-save').onclick();await wait(()=>downloads.length===1);assert.equal(downloads[0].name,'floe-layers.json');assert.equal(draws.length,1,'settings download rerendered the view');
    }
    if(clipEnabled) {
        assert(!node('clip-open').disabled,'display ACK did not enable clip');node('clip-open').onclick();
        node('clip-form').onsubmit({preventDefault(){}});const prepared=ws.sent.at(-1);
        assert.equal(prepared.type,'view.clip.prepare');assert.deepEqual(prepared.body.bounds,{kind:'viewport'});
        assert.equal(requests.filter(r=>r.method==='POST'&&r.path==='/api/v1/exports').length,0);
        ws.receive({type:'clip.prepared',seq:prepared.seq,view_id:viewId,connection_epoch:epoch,source_stale:false,
            draft:{token:'f'.repeat(64),dataset_revision:'1',bbox_dbu:['-11','0','89','80'],layers:{mode:'all'},jobs:4,cell_name:'FLOE_CLIP',expires_in_ms:'30000'}});
        assert(!node('clip-approve').disabled);await node('clip-approve').onclick();
        assert.equal(requests.filter(r=>r.method==='POST'&&r.path==='/api/v1/exports').length,1);
        const buttons=node('clip-files').children[0].children[1];buttons.children[0].onclick();
        assert.deepEqual(forms,[{method:'POST',action:'/api/v1/artifacts/1/download',body:[['csrf','c'.repeat(64)]]}]);
        assert.equal(document.body.children.length,0);await buttons.children[1].onclick();assert.equal(clipFile,null);
        assert.equal(draws.length,1,'clip changed the displayed render');clipController.stop();
    } else {assert(node('clip-panel').hidden);}
    // Real app wiring: the query uses the displayed/ACKed frame without
    // issuing another render or HTTP read. This is not a stub inspector.
    ws.receive({...snapshot,capabilities:{labels:true,query:true}});
    const geometry={kind:'pick',count:'2',index:'0',pair:[7,0],layer_name:'<img onerror=bad>',cell_name:'TOP <script>',area_dbu2:'1500',
        bbox_dbu:['0','10','30','60'],points_dbu:[['0','10'],['30','10'],['30','60'],['0','60']],points_truncated:false};
    const queryClick=(x,y,mod={})=>{
        node('viewport').mousedown({button:0,buttons:1,clientX:x,clientY:y,preventDefault(){},...mod});
        listeners.mouseup({button:0,buttons:0,clientX:x,clientY:y,preventDefault(){},...mod});
    };
    const queryLast=()=>ws.sent.filter(m=>m.type==='view.query').at(-1);
    const answerQuery=(request,hit,status='ok',target=ws)=>{
        target.receive({type:'query.accepted',seq:request.seq,query_id:'9007199254740993',view_id:request.view_id,connection_epoch:request.connection_epoch});
        const reply={type:'query.result',seq:request.seq,query_id:'9007199254740993',view_id:request.view_id,connection_epoch:request.connection_epoch,anchor:request.body.anchor,
            status,hit,scene:{generation:'1',round:'1',complete:true,summary_layers:'0'},requested_summary_layers:'0'};
        target.receive(reply);return reply;
    };
    const oldHttp=requests.length;queryClick(20,20);const picked=queryLast();assert(picked);
    assert.equal(picked.body.anchor.frame_id,'1');assert.deepEqual(picked.body.position,[.2,.25]);assert.equal(picked.body.radius_px,3);
    assert.equal(ws.sent.findIndex(m=>m.type==='frame.ack') < ws.sent.indexOf(picked),true);
    answerQuery(picked,geometry);assert.match(node('pick-details').textContent,/<img onerror=bad>/);assert.equal(node('pick-details').children.length,0);
    await wait(()=>!node('query-canvas').hidden);assert.equal(draws.length,1,'pick redrew native pixels');
    assert.equal(requests.length,oldHttp,'pick made an unrelated HTTP call');
    queryClick(20,20);assert.equal(queryLast().body.operation.nth,'1');answerQuery(queryLast(),{...geometry,index:'1',pair:[8,0]});
    queryClick(20,20,{shiftKey:true});assert.equal(queryLast().body.operation.nth,'0');answerQuery(queryLast(),geometry);
    assert.match(node('pick-status').textContent,/2 selected/);
    queryClick(20,20,{ctrlKey:true});answerQuery(queryLast(),geometry);assert.match(node('pick-status').textContent,/1 selected/);
    node('snap-probe').checked=true;node('snap-probe').onchange();node('viewport').mousemove({clientX:22,clientY:23});
    const snapped=queryLast();assert.equal(snapped.body.operation.kind,'snap');assert.equal(snapped.body.radius_px,10);
    answerQuery(snapped,{kind:'snap',point_dbu:['0','60'],snap:'vertex'});assert.match(node('snap-status').textContent,/vertex/);
    node('viewport').mouseleave();assert.equal(node('snap-status').textContent,'');node('snap-probe').checked=false;node('snap-probe').onchange();
    queryClick(20,20);const staleQuery=queryLast();node('pick-clear').onclick();answerQuery(staleQuery,geometry);
    assert.equal(node('pick-details').textContent,'','cleared request revived the selection');
    ws.receive({type:'error',seq:staleQuery.seq,code:'scene_summary'});assert.equal(node('notice').hidden,true,'old query error replaced current notice');
    // Marker geometry/hits are covered by drc-markers.test.cjs; here assert
    // that a consumed marker click does not also query a layout shape.
    queryClick(20,20);const pendingMarker=queryLast();consumeDRC=true;queryClick(20,20);
    assert.equal(queryLast(),pendingMarker);assert.equal(ws.sent.at(-1).type,'view.query.cancel');
    answerQuery(pendingMarker,geometry);assert.equal(node('pick-details').textContent,'');consumeDRC=false;
    // Inspector selection -> r -> owner bbox calculation -> shared ruler book.
    queryClick(20,20);answerQuery(queryLast(),geometry);
    queryClick(70,20,{shiftKey:true});answerQuery(queryLast(),{...geometry,bbox_dbu:['50','10','80','60'],points_dbu:[['50','10'],['80','10'],['80','60'],['50','60']]});
    assert.match(node('pick-status').textContent,/2 selected/);
    node('viewport').keydown({key:'r',preventDefault(){}});
    const autoRequest=ws.sent.filter(m=>m.type==='view.measure_selection').at(-1);assert(autoRequest);
    assert.deepEqual(autoRequest.body.boxes_dbu,[geometry.bbox_dbu,['50','10','80','60']]);
    ws.receive({type:'measure_selection.result',seq:autoRequest.seq,view_id:autoRequest.view_id,connection_epoch:autoRequest.connection_epoch,
        anchor:autoRequest.body.anchor,segments:[{endpoints_dbu:[['30','35'],['50','35']],delta_um:['20','0'],distance_um:'20'}]});
    assert.match(node('ruler-auto').textContent,/20.0000/);assert.equal(drcOptions.history.entries()[0].kind,'auto');
    assert.equal(draws.length,1);assert.equal(requests.length,oldHttp);
    node('viewport').keydown({key:'k',preventDefault(){}});assert.equal(drcOptions.history.entries().length,0);
    node('ruler-mode').onclick();node('pick-clear').onclick();
    // Actual app input dispatch: ruler mode owns the click, even over a DRC
    // marker. It uses snap then Rust measurement without native redraw/HTTP.
    node('ruler-mode').onclick();assert.equal(node('ruler-mode')['aria-pressed'],'true');
    assert(node('ruler-snap').checked);consumeDRC=true;queryClick(20,20);
    assert.equal(queryLast().body.operation.kind,'snap');answerQuery(queryLast(),{kind:'snap',point_dbu:['0','60'],snap:'vertex'});
    const measureLast=()=>ws.sent.filter(m=>m.type==='view.measure').at(-1);
    const measureReply=(point,segment)=>{const t=measureLast();ws.receive({type:'measure.result',seq:t.seq,view_id:t.view_id,connection_epoch:t.connection_epoch,
        anchor:t.body.anchor,point_dbu:point,snap:'vertex',segment});};
    assert.equal(measureLast().body.snap_query,'9007199254740993');measureReply(['0','60'],null);
    queryClick(40,40,{shiftKey:true});answerQuery(queryLast(),{kind:'snap',point_dbu:['30','40'],snap:'vertex'});
    assert(measureLast().body.free_angle);assert.deepEqual(measureLast().body.start_dbu,['0','60']);
    measureReply(['30','40'],{endpoints_dbu:[['0','60'],['30','40']],delta_um:['30','-20'],distance_um:String(Math.sqrt(1300))});
    assert.equal(node('ruler-count').textContent,'1 rulers');await wait(()=>!node('ruler-canvas').hidden);
    assert.equal(draws.length,1,'ruler redrew native pixels');assert.equal(requests.length,oldHttp);
    node('ruler-mode').onclick();node('ruler-clear').onclick();consumeDRC=false;
    assert.equal(node('ruler-count').textContent,'0 rulers');assert.equal(node('ruler-mode')['aria-pressed'],'false');
    ws.receive({...snapshot,capabilities:{labels:true,query:false}});drcClicks.length=0;
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
    function marginQuery(id){
        second.receive({...snapshot,capabilities:{...snapshot.capabilities,query:true}});
        queryClick(20,20);const q=second.sent.filter(m=>m.type==='view.query').at(-1);
        assert.equal(q.body.anchor.frame_id,id);assert.equal(q.body.anchor.state_rev,snapshot.state_rev);
        assert.equal(q.body.anchor.render_rev,snapshot.render_rev);assert.deepEqual(q.body.position,[.2,.25]);
        answerQuery(q,geometry,'ok',second);assert.match(node('pick-details').textContent,/TOP/);node('pick-clear').onclick();
        second.receive(snapshot);drcClicks.length=0;
    }
    marginQuery('8');
    assert.deepEqual(DRC.point(drcDisplays.at(-1).p,0,0),[10.9375,80]);
    assert(node('perf').textContent.includes('100 × 80 px'));
    assert(!node('perf').textContent.includes('196 × 176 px'));
    const painted=draws.length;
    const listRequests=requests.filter(r=>r.path.endsWith('/palette')).length;
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
    assert.equal(requests.filter(r=>r.path.endsWith('/palette')).length,listRequests,'pan refreshed the layer panel');
    marginQuery('8'); // Same ACKed margin, newer viewport state/render revisions.
    // Truncated labels are base only, not a completed replacement for foreground.
    snapshot.margin={frame_id:'9',origin_px:[96,48],crop_safe:false};second.receive(snapshot);
    second.receive(packet('raw','9','3',nextEpoch,{...margin,complete:false,labels_truncated:true}));
    assert.equal(node('canvas').hidden,false);assert.equal(node('margin-canvas').hidden,false);
    assert.equal(node('canvas').style.left,'-48px');
    marginQuery('9'); // Geometry-complete margin under old labels is queryable.
    if(snapshotEnabled){
        const at=draws.length,network=requests.length,sent=second.sent.length;
        node('snapshot-save').onclick();
        assert.deepEqual(captures.at(-1).draws.slice(at).map(d=>[d.source,d.x||0,d.y||0]),[['margin-canvas',-96,-48],['canvas',-48,0]]);
        await wait(()=>!node('snapshot-save').disabled);assert.equal(downloads.length,1);
        assert.equal(requests.length,network);assert.equal(second.sent.length,sent,'pan snapshot requested a native render');
    }
    // A policy change during slow margin decoding cannot land the stale image.
    snapshot.margin.frame_id='10';second.receive(snapshot);
    second.receive(packet('png','10','3',nextEpoch,margin));const staleMargin=images.at(-1);
    staleMargin.naturalWidth=196;staleMargin.naturalHeight=176;
    const previousLayerRow=node('layers').children[0];
    snapshot.state_rev='4';snapshot.render_rev='4';snapshot.render_key='2';snapshot.margin=null;
    second.receive(snapshot);const beforeLate=draws.length;staleMargin.onload();
    assert.equal(draws.length,beforeLate);assert.equal(node('margin-canvas').hidden,true);
    assert.equal(node('margin-canvas').width,1);
    assert.equal(second.sent.at(-1).disposition,'discarded');assert.equal(urls.size,0);
    await wait(()=>node('layers').children.length===1&&node('layers').children[0]!==previousLayerRow);
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
    assert.deepEqual(sent.body.style_batch,{pairs:[[7,0]],collapsed:[],fill:{kind:'pattern',rows:new Array(16).fill(0xa55a)},width:4});
    async function applied(policy=true){
        const s=second.sent.filter(m=>m.type==='view.set').at(-1);
        snapshot.state_rev=P.next(snapshot.state_rev);snapshot.render_rev=P.next(snapshot.render_rev);
        if(policy){snapshot.render_key=P.next(snapshot.render_key);}
        second.receive({type:'accepted',seq:s.seq,state_rev:snapshot.state_rev,render_rev:snapshot.render_rev});second.receive(snapshot);
        await new Promise(setImmediate);
    }
    await applied();
    await wait(()=>node('layers').children.length===1);
    const colorInput=node('layers').children[0].children[1];colorInput.value='#123456';colorInput.onchange();
    assert.deepEqual(second.sent.at(-1).body,{style_batch:{pairs:[[7,0]],collapsed:[],color:'#123456'}});await applied();
    await wait(()=>node('layers').children.length===1);
    node('layers').children[0].children.at(-1).onclick();
    const noChange=second.sent.length;node('style-editor').onsubmit(submitEvent);
    assert.equal(second.sent.length,noChange,'unchanged style form materialized inherited settings');
    node('layers').children[0].children.at(-1).onclick();node('style-width').value='7';node('style-editor').onsubmit(submitEvent);
    assert.deepEqual(second.sent.at(-1).body,{style_batch:{pairs:[[7,0]],collapsed:[],width:7}});await applied();
    node('font-px').value='18';node('font-px').onchange();assert.equal(second.sent.at(-1).body.font_px,18);await applied();
    node('viewport').keydown({key:'f',preventDefault(){}});assert.deepEqual(second.sent.at(-1).body,{frames:true});await applied();
    node('viewport').keydown({key:'a',ctrlKey:true,preventDefault(){}});assert.equal(second.sent.at(-1).body.navigation.kind,'fit');await applied(false);
    node('viewport').keydown({key:'9',preventDefault(){}});assert.equal(second.sent.at(-1).body.depth,'9');await applied();
    node('viewport').keydown({key:'9',preventDefault(){}});assert.equal(second.sent.at(-1).body.depth,'full');await applied();
    node('viewport').keydown({key:'<',shiftKey:true,preventDefault(){}});assert.deepEqual(second.sent.at(-1).body,{depth_step:-1});
    const firstDepthSeq=second.sent.at(-1).seq;
    node('viewport').keydown({key:'>',shiftKey:true,preventDefault(){}});assert.equal(second.sent.at(-1).seq,firstDepthSeq,'relative depth bypassed inflight CAS');
    await applied();await wait(()=>second.sent.at(-1).seq!==firstDepthSeq);
    assert.deepEqual(second.sent.at(-1).body,{depth_step:1});await applied();
    const depthCount=second.sent.length;
    for(const extra of [{ctrlKey:true},{metaKey:true},{altKey:true},{isComposing:true}]){
        node('viewport').keydown({key:'<',preventDefault(){assert.fail('modified/composing depth key consumed');},...extra});
    }
    assert.equal(second.sent.length,depthCount);
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
    const right=(x,y,buttons=2)=>({...mouse(x,y),button:2,buttons});
    node('viewport').mousedown(right(20,20));listeners.mousemove(right(60,50));
    await wait(()=>!node('zoom-band').hidden);
    assert.equal(node('zoom-band').style.left,'20px');assert.equal(node('zoom-band').style.width,'40px');
    assert.match(node('zoom-band-hint').textContent,/Zoom in/);assert.equal(dragEdits(),beforeDrag+1);
    node('viewport').keydown({key:'Escape',preventDefault(){}});assert(node('zoom-band').hidden);
    listeners.mouseup(right(60,50,0));assert.equal(dragEdits(),beforeDrag+1);
    node('overlays').value='none';node('overlays').onchange();
    node('viewport').mousedown(right(20,20));listeners.mousemove(right(60,50));
    await wait(()=>!node('zoom-band').hidden);listeners.mouseup(right(60,50,0));
    const bandWire=second.sent.at(-1).body.navigation;
    assert.deepEqual({...bandWire,end:null},{kind:'band',start:[.2,.25],end:null,axes:[true,true],outward:false});
    assert(Math.abs(bandWire.end[0]-.6)<1e-15);assert.equal(bandWire.end[1],.625);
    assert(node('zoom-band').hidden);assert.equal(drcClicks.length,2);await applied(false);
    // Until the corresponding frame lands, an old frozen image is not a band anchor.
    const afterBand=dragEdits();node('viewport').mousedown(right(20,20));listeners.mousemove(right(60,50));listeners.mouseup(right(60,50,0));
    assert.equal(dragEdits(),afterBand);assert(node('zoom-band').hidden);
    node('overlays').value='all';node('overlays').onchange();
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
    const oversized=[],sentBefore=second.sent.length;
    drcOptions.navigate({},'a'.repeat(9000),e=>oversized.push(e));assert.match(oversized[0],/too large.*nothing was applied/);
    assert.equal(second.sent.length,sentBefore,'oversized edit reached the wire');
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
    if(snapshotEnabled){
        reconnected.receive(packet('raw','13',snapshot.render_rev,'9'.repeat(64),{width:120,height:90}));
        assert(!node('snapshot-copy').disabled);assert(!node('snapshot-save').disabled);
        const beforeRequests=requests.length,beforeSent=reconnected.sent.length,copyBase=copies.length,captureBase=captures.length,downloadBase=downloads.length;
        function key(k,extra={}){let used=false;node('viewport').keydown({key:k,target:node('viewport'),preventDefault(){used=true;},...extra});return used;}
        for(const mode of ['focus','none','all']){assert(key('Tab'));assert.equal(node('overlays').value,mode);}
        assert(!key('Tab',{shiftKey:true}));assert.equal(node('overlays').value,'all');
        const cut=draws.length;assert(key('c',{ctrlKey:true}));assert.equal(copies.length,copyBase+1,'copy was not called synchronously from keydown');
        assert.equal(captures.length,captureBase+1);assert.deepEqual([captures.at(-1).width,captures.at(-1).height],[120,90]);
        assert.deepEqual(captures.at(-1).draws.slice(cut).map(d=>[d.source,d.x||0,d.y||0]),[['canvas',0,0]]);
        await wait(()=>!node('snapshot-copy').disabled);assert.match(node('snapshot-status').textContent,/Copied 120 × 90/);
        assert(key('c',{metaKey:true}));await wait(()=>!node('snapshot-copy').disabled);assert.equal(copies.length,copyBase+2);
        textSelection={isCollapsed:false};assert(!key('c',{ctrlKey:true}));textSelection=null;
        for(const extra of [{shiftKey:true},{altKey:true},{isComposing:true},{target:node('goto-x')},{target:node('canvas')}])assert(!key('c',{ctrlKey:true,...extra}));
        assert.equal(copies.length,copyBase+2,'copy hijacked selection, an editor or browser shortcut');
        node('snapshot-save').onclick();await wait(()=>downloads.length===downloadBase+1&&!node('snapshot-save').disabled);
        assert.equal(downloads.at(-1).name,'floe-view-120x90.png');assert.equal(captureUrls.size,1);
        node('snapshot-retry').onclick();assert.equal(downloads.length,downloadBase+2);assert.equal(captures.length,captureBase+3,'download fallback rerendered');
        assert.equal(requests.length,beforeRequests,'snapshot or overlays made an HTTP request');
        assert.equal(reconnected.sent.length,beforeSent,'snapshot or overlays dispatched a native command');
    }
    listeners.pagehide();
    assert.equal(captureUrls.size,0);
    assert.equal(observers[0].target,null);
    console.log('WEB CLIENT: ALL OK (startup/frames/epochs, margin/pan/DRC, controls, token-only edits, ACK+snapshot ordering, cancellation/limits/reconnect, cleanup)');
})().catch(e=>{console.error(e);process.exitCode=1;});
