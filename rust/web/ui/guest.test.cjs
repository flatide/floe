'use strict';
const assert=require('node:assert/strict'),P=require('./protocol.js'),Guest=require('./guest.js');
const Decode=require('./image-decode.js');
const id='a'.repeat(64),view='b'.repeat(64),epoch='c'.repeat(64),secret='d'.repeat(64),bundle='test-bundle';
const auth={protocol:1,bundle,share_id:id,session_id:'e'.repeat(64),csrf:'f'.repeat(64)};
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function environment(mode='explore',hash='#invite='+secret,grant=false){
    const nodes=new Map(),events={},requests=[],sockets=[],timers=new Map(),rafs=new Map(),storage=new Map([['floe-session:http://127.0.0.1:1234','OWNER'],['floe-default-pending:OWNER','PRIVATE']]),reads=[];
    let timerId=0,doc,sessionCode=200,clock=10000;
    class Element{
        constructor(name){this.id=name;this.value='';this.checked=false;this.disabled=false;this.hidden=false;this.width=1;this.height=1;this.pixels=null;this.textContent='';this.listeners={};this.children=[];this.attrs={};}
        getContext(){const self=this;return {save(){},restore(){},fillRect(){self.pixels=new Uint8ClampedArray(self.width*self.height*4);},
            putImageData(image){self.pixels=image.data.slice();},drawImage(image,dx=0,dy=0){if(!self.pixels||self.pixels.length!==self.width*self.height*4){self.pixels=new Uint8ClampedArray(self.width*self.height*4);}
                for(let y=0;y<image.height;y++)for(let x=0;x<image.width;x++){const a=x+dx,b=y+dy;if(a>=0&&b>=0&&a<self.width&&b<self.height){self.pixels.set(image.pixels.slice((y*image.width+x)*4,(y*image.width+x+1)*4),(b*self.width+a)*4);}}}};}
        getBoundingClientRect(){return {width:64,height:32,left:0,top:0};}
        addEventListener(k,f){this.listeners[k]=f;}focus(){doc.activeElement=this;}
    }
    const el=k=>{if(!nodes.has(k)){nodes.set(k,new Element(k));}return nodes.get(k);};
    doc={getElementById:el,querySelector:()=>({content:bundle}),createElement:k=>new Element(k),activeElement:null,addEventListener(k,f){events[k]=f;}};
    const win={devicePixelRatio:1,sessionStorage:{getItem(k){reads.push(k);return storage.get(k)||null;},setItem(k,v){storage.set(k,v);},removeItem(k){storage.delete(k);}},
        setTimeout(f,ms){const n=++timerId;timers.set(n,{f,ms});return n;},clearTimeout(n){timers.delete(n);},setInterval(f,ms){const n=++timerId;timers.set(n,{f,ms,interval:true});return n;},clearInterval(n){timers.delete(n);},
        requestAnimationFrame(f){const n=++timerId;rafs.set(n,f);return n;},cancelAnimationFrame(n){rafs.delete(n);},addEventListener(k,f){events[k]=f;}};
    const location={origin:'http://127.0.0.1:1234',pathname:'/guest/'+id,hash};
    const history={replaceState(a,b,path){assert.equal(path,location.pathname);location.hash='';}};
    class XHR{
        open(method,path){this.method=method;this.path=path;this.headers={};}setRequestHeader(k,v){this.headers[k]=v;}
        abort(){if(this.onabort){this.onabort();}}
        send(body){assert.equal(location.hash,'','fragment must be removed before the first HTTP request');const entry={method:this.method,path:this.path,body:body?JSON.parse(body):null,headers:this.headers};requests.push(entry);
            assert(this.path.startsWith('/api/v1/guest/'+id+'/'));assert(!('X-Floe-CSRF' in this.headers));
            let v;if(this.path.endsWith('/exchange')){assert.equal(entry.body.invite,secret);v=auth;this.status=200;}
            else{assert.equal(this.headers['X-Floe-Guest-CSRF'],auth.csrf);this.status=sessionCode;
                if(this.path.endsWith('/layers')){assert.equal(entry.body.view_id,view);v={view_id:view,data:{state_rev:entry.body.state_rev,render_key:'1',start:0,next:null,total:0,all_total:0,rows:[]}};}
                else if(this.path.endsWith('/drc')){assert(grant);v={view_id:view,revision:'drc-rev',data:{checks:'0',errors:'0',precision:'1000',format:'ice',truncated_records:'0',read_only:true}};}
                else if(this.path.endsWith('/drc/panel')){assert(grant);v={view_id:view,revision:'drc-rev',data:{panel_rev:'1',body:null}};}
                else if(this.path.endsWith('/drc/selection')){assert(grant);v={view_id:view,revision:'drc-rev',data:{selection_rev:'1',total:'0',limit:5000,rules:[]}};}
                else if(this.path.endsWith('/drc/read')){assert(grant);assert.equal(entry.body.body.kind,'rules');assert.equal(entry.body.view_id,view);v={view_id:view,revision:'drc-rev',data:{rows:[],next:null}};}
                else{v=this.method==='DELETE'?null:{share_id:id,mode,read_only:true,delivery:mode==='follow'?'follow_frames':'explore_frames',...grant&&{drc:{id:'7'.repeat(64),revision:'drc-rev'}}};}}
            this.responseText=v?JSON.stringify(v):'';this.onload();}
    }
    class WS{
        constructor(url,protocols){assert.equal(url,'ws://127.0.0.1:1234/api/v1/guest/'+id+'/events');assert.deepEqual(protocols,['floe.v1','bundle.'+bundle,'guest-csrf.'+auth.csrf]);this.sent=[];this.readyState=1;sockets.push(this);}
        send(text){this.sent.push(JSON.parse(text));}close(){this.readyState=3;}text(v){this.onmessage({data:JSON.stringify(v)});}binary(b){this.onmessage({data:b});}
    }
    const c=Guest.bind({window:win,document:doc,location,history,XHR,WebSocket:WS,protocol:P,now:()=>clock,
        drc:require('./guest-drc.js'),geometry:require('./drc-geometry.js'),selection:require('./drc-groups.js'),
        layers:require('./guest-layers.js'),
        decode(h,data,cb){return Decode.create({ImageData:class{constructor(data){this.data=data;}},setTimeout:win.setTimeout,clearTimeout:win.clearTimeout},h,data,cb);}});
    function hello(ws=sockets.at(-1),connection=epoch){ws.onopen();ws.text({type:'share.hello',protocol:1,bundle,share_id:id,view_id:view,connection_epoch:connection,mode,read_only:true});}
    function state(extra={}){return {type:'share.state',view_id:view,connection_epoch:epoch,dataset_revision:'1',worker_epoch:'1',state_rev:'1',render_rev:'1',render_key:'1',bbox_dbu:['0','0','64','32'],dbu_um:'1',camera_um:['32','16','64'],pixels:[64,32],depth:'full',detail:'high',thin:'keep',frames:true,labels:true,mono:false,...extra};}
    function raf(){for(const [n,f] of [...rafs]){rafs.delete(n);f();}}
    function timer(ms){const found=[...timers].find(([,t])=>t.ms===ms);assert(found,'timer '+ms);if(!found[1].interval){timers.delete(found[0]);}clock+=ms;found[1].f();}
    return {c,el,win,doc,events,requests,sockets,storage,reads,location,hello,state,raf,timer,code(n){sessionCode=n;}};
}
function packet(extra={},color=[1,2,3,255]){
    const h={type:'frame',protocol:1,row0:'top',purpose:'foreground',format:'raw',view_id:view,connection_epoch:epoch,frame_id:'1',dataset_revision:'1',worker_epoch:'1',state_rev:'1',render_rev:'1',render_key:'1',generation:'1',round:'1',deferred:'0',deck_skipped:'0',final:true,partial:false,labels_truncated:false,complete:true,approximate:false,query:false,query_scene:{generation:null,round:null,complete:false,summary_layers:'0'},bbox_dbu:['0','0','64','32'],width:64,height:32,...extra};
    const data=new Uint8Array(16+h.width*h.height*4);data.set(new TextEncoder().encode('FLOERAW1'));const dv=new DataView(data.buffer);dv.setUint32(8,h.width,true);dv.setUint32(12,h.height,true);
    for(let i=16;i<data.length;i+=4){data.set(color,i);}h.payload_length=String(data.length);const text=new TextEncoder().encode(JSON.stringify(h)),out=new Uint8Array(4+text.length+data.length);new DataView(out.buffer).setUint32(0,text.length,true);out.set(text,4);out.set(data,4+text.length);return out.buffer;
}
(async()=>{
    const invalid=environment('follow','#bootstrap='+secret);await invalid.c.start();assert.equal(invalid.requests.length,0);assert.equal(invalid.storage.get('floe-session:http://127.0.0.1:1234'),'OWNER');
    for(const mode of ['follow','explore']){
        const e=environment(mode);await e.c.start();const ws=e.sockets[0];e.hello();ws.text(e.state());
        assert(e.storage.has('floe-guest-session:http://127.0.0.1:1234:'+id));assert.equal(e.storage.get('floe-session:http://127.0.0.1:1234'),'OWNER');assert.equal(e.reads.length,0);
        assert.equal(e.el('guest-controls').hidden,mode==='follow');ws.binary(packet());assert.equal(ws.sent.length,0,'ACK only after presentation');e.raf();assert.equal(ws.sent[0].disposition,'displayed');assert.equal(e.el('guest-empty').hidden,true);
        assert.deepEqual([...e.el('guest-canvas').pixels.slice(0,4)],[1,2,3,255]);
        e.el('guest-in').onclick();if(mode==='follow'){assert.equal(ws.sent.length,1);}else{
            const command=ws.sent.at(-1);assert.equal(command.type,'explore.set');assert.equal(command.view_id,view);assert.deepEqual(command.body.navigation,{kind:'zoom',factor:0.8,anchor:[0.5,0.5]});
            e.el('guest-out').onclick();assert.equal(ws.sent.length,2,'serialize edits until accepted state');ws.text({type:'accepted',seq:command.seq,view_id:view,connection_epoch:epoch,state_rev:'2',render_rev:'2'});assert.equal(ws.sent.length,2);
            ws.text(e.state({state_rev:'2',render_rev:'2'}));assert.equal(ws.sent.length,2);e.timer(65);assert.equal(ws.sent.length,3);assert.equal(ws.sent.at(-1).base_state_rev,'2');
        }
        // Disconnect clears pixels and queued edits; reconnect never exchanges
        // the one-use invitation again or replays navigation.
        ws.onclose();assert.equal(e.el('guest-empty').hidden,false);e.timer(500);await tick();assert.equal(e.sockets.length,2);assert.equal(e.requests.filter(r=>r.path.endsWith('/exchange')).length,1);
        const next=e.sockets[1];e.hello(next,'9'.repeat(64));next.text(e.state({connection_epoch:'9'.repeat(64)}));assert.equal(next.sent.length,0);
        await e.el('guest-leave').onclick();assert.equal(e.requests.at(-1).method,'DELETE');assert.equal(e.storage.get('floe-session:http://127.0.0.1:1234'),'OWNER');assert.equal(e.storage.get('floe-default-pending:OWNER'),'PRIVATE');
        assert(!e.storage.has('floe-guest-session:http://127.0.0.1:1234:'+id));
    }
    // Margin + prior label frame compose new strip immediately without an ACK
    // for a stale delayed frame or reuse after epoch/revoke.
    const e=environment();await e.c.start();const ws=e.sockets[0];e.hello();ws.text(e.state());ws.binary(packet({},[255,0,0,255]));e.raf();
    ws.text(e.state({margin:{frame_id:'2',origin_px:[16,16],crop_safe:false}}));ws.binary(packet({frame_id:'2',purpose:'margin',width:96,height:64,bbox_dbu:['-16','-16','80','48']},[0,255,0,255]));e.raf();
    ws.text(e.state({state_rev:'2',bbox_dbu:['16','0','80','32'],margin:{frame_id:'2',origin_px:[32,16],crop_safe:false}}));
    const pixels=e.el('guest-canvas').pixels;assert.deepEqual([...pixels.slice(0,4)],[255,0,0,255]);assert.deepEqual([...pixels.slice(63*4,64*4)],[0,255,0,255]);
    ws.binary(packet({frame_id:'3',bbox_dbu:['16','0','80','32']}));ws.text(e.state({state_rev:'3',render_rev:'2',render_key:'2'}));e.raf();assert.equal(ws.sent.at(-1).disposition,'discarded');assert.equal(e.el('guest-empty').hidden,false);
    ws.onclose();e.code(401);e.timer(500);await tick();assert.equal(e.sockets.length,1);assert(!e.storage.has('floe-guest-session:http://127.0.0.1:1234:'+id));
    // The native view permits 4096 explicit u32 layer/datatype pairs. State
    // replies must not confuse that valid selection with the frame-header cap.
    const large=environment();await large.c.start();large.hello();const largeSocket=large.sockets[0];
    const largeState=large.state({selection:{mode:'only',pairs:Array.from({length:4096},(_,i)=>[4294967295,4294963200+i])}});
    assert(JSON.stringify(largeState).length>65536);largeSocket.text(largeState);
    assert.equal(largeSocket.readyState,1);assert.equal(large.el('guest-in').disabled,false);
    large.el('guest-in').onclick();assert.equal(largeSocket.sent.at(-1).type,'explore.set');
    largeSocket.text({...largeState,padding:'x'.repeat(256*1024)});
    assert.equal(largeSocket.readyState,3);assert(!large.storage.has('floe-guest-session:http://127.0.0.1:1234:'+id));
    const suspended=environment();await suspended.c.start();suspended.hello();suspended.sockets[0].text(suspended.state());suspended.sockets[0].binary(packet());suspended.events.pagehide();suspended.raf();assert.equal(suspended.sockets[0].sent.length,0);
    const hidden=environment();await hidden.c.start();hidden.hello();hidden.sockets[0].text(hidden.state());hidden.doc.hidden=true;hidden.events.visibilitychange();assert.equal(hidden.sockets[0].readyState,3);hidden.doc.hidden=false;hidden.events.visibilitychange();await tick();assert.equal(hidden.sockets.length,2);assert.equal(hidden.requests.filter(r=>r.path.endsWith('/exchange')).length,1);
    for(const mode of ['follow','explore']){const review=environment(mode,'#invite='+secret,true);await review.c.start();assert(!review.requests.some(r=>r.path.includes('/drc')),'wait for WS view admission');review.hello();review.sockets[0].text(review.state());await tick();
        assert.equal(review.el('gd-panel').hidden,false);assert.equal(review.requests.filter(r=>r.path.includes('/drc')).length,4);review.sockets[0].binary(packet());review.raf();assert.equal(review.sockets[0].sent.at(-1).disposition,'displayed');
        review.sockets[0].onclose();assert(review.el('gd-panel').hidden);}
    console.log('WEB GUEST UI: ALL OK (separate credentials/routes/storage, follow+explore, raw pixels, presented ACK, margin strips, stale rejection, queue/reconnect/revoke/logout)');
})().catch(e=>{console.error(e);process.exitCode=1;});
