'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),V=require('./viewer.js'),P=require('./protocol.js');
const {environment}=require('./server.test.cjs');
function controls(prefix){
    const nodes=new Map(),edits=[],notices=[],doc={activeElement:null};let time=10000;
    const el=id=>{const key=prefix+id;if(!nodes.has(key)){nodes.set(key,{value:'',checked:false,listeners:{},
        addEventListener(k,f){this.listeners[k]=f;},focus(){doc.activeElement=this;},select(){}});}return nodes.get(key);};
    const state={depth:'full',detail:'high',thin:'auto',frames:true,labels:true,mono:false,font_px:14,max_depth:7,camera_um:['1.25','-4','20']};
    const context={id:'synthetic',state,ready:true};
    const ui=V.bindControls({el,document:doc,protocol:P,context:()=>context,edit:(body,done)=>edits.push({body,done}),notice:s=>notices.push(s),now:()=>time});
    ui.sync();return {ui,el,doc,state,context,edits,notices,advance(){time+=1001;}};
}
const owner=controls(''),demo=controls('server-'),both=[owner,demo];
const key=(k,extra={})=>({key:k,preventDefault(){},...extra});
for(const e of [key('ArrowLeft'),key('ArrowUp',{shiftKey:true}),key('+'),key('-'),key('Home'),key('a',{ctrlKey:true}),
    key('z',{ctrlKey:true}),key('f'),key('b'),key('Z'),key('<'),key('>'),key('9'),key('9'),key('2'),key('ㄹ',{code:'KeyF'})]){
    for(const h of both)h.ui.key(e);
    assert.deepEqual(owner.edits.at(-1).body,demo.edits.at(-1).body);
}
for(const k of ['depth','detail','thin','frames','labels','mono','font-px']){
    for(const h of both){h.el(k).value=k==='font-px'?'18':k==='depth'?'6':k==='detail'?'medium':'keep';h.el(k).checked=false;h.el(k).onchange();}
    assert.deepEqual(owner.edits.at(-1).body,demo.edits.at(-1).body);
}
for(const h of both){
    const count=h.edits.length;
    for(const extra of [{isComposing:true},{keyCode:229},{metaKey:true},{altKey:true}])h.ui.key(key('+',extra));
    h.el('depth').value='bad';h.el('depth').onchange();h.el('font-px').value='999';h.el('font-px').onchange();assert.equal(h.edits.length,count);
    h.el('goto-x').value='123.';h.el('goto-x').listeners.input();h.state.camera_um=['9','8','30'];h.ui.sync();assert.equal(h.el('goto-x').value,'123.');
    h.el('goto-x').listeners.keydown(key('Escape',{isComposing:true}));assert(h.ui.dirty());
    h.el('goto-x').listeners.keydown(key('Escape'));assert(!h.ui.dirty());assert.equal(h.el('goto-x').value,'9');
    h.el('goto-form').onsubmit({preventDefault(){}});assert.deepEqual(h.edits.at(-1).body,{navigation:{kind:'goto',center_um:['9','8'],width_um:'30'}});
    h.edits.at(-1).done('rejected');assert(h.ui.dirty());h.el('goto-form').onsubmit({preventDefault(){}});h.edits.at(-1).done(null);assert(!h.ui.dirty());
}
const rect={left:.25,top:.25,right:100.25,bottom:80.25,width:100,height:80};
assert.deepEqual(V.dimensions(P,rect,2),{pixels:[199,159],dpr:2,left:.25,top:.25});
const aligned=V.dimensions(P,rect,2);assert.deepEqual(V.screen(rect,aligned.pixels,aligned),aligned);
const cap=V.dimensions(P,{left:0,top:0,right:7680,bottom:4320,width:7680,height:4320},2,{axis:2048,area:2097152});
assert(cap.pixels[0]<=2048&&cap.pixels[1]<=2048&&cap.pixels[0]*cap.pixels[1]<=2097152);
assert.deepEqual(V.screen({width:96,height:32},[64,32]),{pixels:[64,32],dpr:1,left:16,top:0});
const s={view_id:'a',connection_epoch:'b',dataset_revision:'1',worker_epoch:'1',render_key:'1',render_rev:'1',pixels:[64,32],bbox_dbu:['0','0','64','32'],status:'idle'};
const frame={...s,width:64,height:32,final:true,purpose:'foreground',complete:true,deck_skipped:'0',generation:'9'};
const c={active:true,connected:true,state:s,frame};assert.equal(V.busy(P,c),false);
for(const delta of [{pending:true},{decoding:true},{presenting:true},{connecting:true},{frame:null},{frame:{...frame,final:false}},{state:{...s,render_key:'2'}},{state:{...s,status:'rendering'}}])assert(V.busy(P,{...c,...delta}));
for(const delta of [{active:false},{connected:false},{state:{...s,status:'failed'}},{state:{...s,status:'closed'}}])assert.equal(V.busy(P,{...c,pending:true,...delta}),false);
assert.equal(V.busy(P,{...c,frame:{...frame,complete:false}}),false);
assert.equal(V.busy(P,{...c,frame:null,completedFrame:frame}),false,'no-op composite waits for nonexistent work');
for(const delta of [{connected:false},{frame:null,completedFrame:frame},{acked:false},{frame:{...frame,render_key:'2'}}]){
    let sent=false;
    V.wheel({protocol:P,gestures:require('./gestures.js'),viewport:{getBoundingClientRect:()=>rect},navigate(){sent=true;},
        context:()=>({...c,acked:true,size:{pixels:[64,32],dpr:1},...delta})},{deltaY:1,preventDefault(){}});
    assert.equal(sent,false,'completion hint granted wheel input');
}
assert.equal(V.sameSource(frame,{...s,render_key:'2'}),true);
for(const k of ['view_id','connection_epoch','dataset_revision','worker_epoch'])assert.equal(V.sameSource(frame,{...s,[k]:'other'}),false);
assert.match(V.frameStatus({...frame,final:false}),/^Refining/);assert.match(V.frameStatus({...frame,complete:false,labels_truncated:true}),/^INCOMPLETE · labels partial/);
// Both entry points must delegate behaviour, not maintain forked fixes.
for(const file of ['app.js','server.js']){
    const text=fs.readFileSync(__dirname+'/'+file,'utf8');
    for(const name of ['compose','paint','freeze','dimensions','busy','cursor','wheel','band','bindControls','frameStatus'])assert(text.includes('V.'+name+'('),file+' misses shared '+name);
    assert(!text.includes('wheelNavigation('));assert(!text.includes('lastDigitAt'));
}
for(const file of ['index.html','server.html']){
    const text=fs.readFileSync(__dirname+'/'+file,'utf8');assert(text.includes('/viewer.js'));assert(text.includes('/viewer.css'));
}
assert(!/XMLHttpRequest|WebSocket|sessionStorage|localStorage|\/api\//.test(fs.readFileSync(__dirname+'/viewer.js','utf8')),'viewer acquired transport authority');
(async()=>{
    for(const [event,expected] of [[key('a',{ctrlKey:true}),{navigation:{kind:'fit'}}],[key('f'),{frames:false}],
        [key('b'),{mono:true}],[key('>'),{depth_step:1}],[key('9'),{depth:'9'}]]){
        const h=environment();await h.c.start();h.hello();h.el('viewport').listeners.keydown(event);
        assert.deepEqual(h.sockets[0].sent.at(-1).body,expected);h.c.stop();
    }
    const h=environment();await h.c.start();h.hello();assert.equal(h.el('x').value,'32');
    h.el('x').value='123.';h.el('x').listeners.input();h.sockets[0].text(h.state({state_rev:'2',camera_um:['9','8','30']}));assert.equal(h.el('x').value,'123.');
    h.el('x').listeners.keydown(key('Escape'));assert.equal(h.el('x').value,'9');
    h.el('font-px').value='18';h.el('font-px').onchange();assert.deepEqual(h.sockets[0].sent.at(-1).body,{font_px:18});h.c.stop();
    console.log('SHARED VIEWER: ALL OK (control/shortcut parity, goto drafts, native placement, limits, cursor lifecycle, shared-source/authority guards)');
})().catch(e=>{console.error(e);process.exitCode=1;});
