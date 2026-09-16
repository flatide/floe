'use strict';
const assert=require('node:assert/strict'),P=require('./protocol.js'),Guest=require('./guest.js');
const Decode=require('./image-decode.js');
const id='a'.repeat(64),view='b'.repeat(64),epoch='c'.repeat(64),secret='d'.repeat(64),bundle='test-bundle';
const auth={protocol:1,bundle,share_id:id,session_id:'e'.repeat(64),csrf:'f'.repeat(64)};
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function environment(mode='explore',hash='#invite='+secret,grant=false){
    const nodes=new Map(),events={},requests=[],sockets=[],timers=new Map(),rafs=new Map(),storage=new Map([['floe-session:http://127.0.0.1:1234','OWNER'],['floe-default-pending:OWNER','PRIVATE']]),reads=[];
    let timerId=0,doc,sessionCode=200,clock=10000,reviewRev='1',groupRev='1',group=[];
    const reviewRow={check:'0',local:'0',global:'1',kind:'p',status:0,bbox_um:['20','8','24','12'],points:'4'};
    class Element{
        constructor(name){this.id=name;this.value='';this.checked=false;this.disabled=false;this.hidden=false;this.width=1;this.height=1;this.pixels=null;this.textContent='';this.listeners={};this.children=[];this.attrs={};this.style={};}
        setAttribute(k,v){this.attrs[k]=v;}
        set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}appendChild(v){this.children.push(v);}
        getContext(){const self=this;return {save(){},restore(){},clearRect(){self.pixels=new Uint8ClampedArray(self.width*self.height*4);},beginPath(){},rect(){},clip(){},moveTo(){},lineTo(){},closePath(){},stroke(){},strokeRect(){},fill(){},setTransform(){},setLineDash(){},fillText(){},measureText(){return {width:20};},fillRect(){self.pixels=new Uint8ClampedArray(self.width*self.height*4);},
            putImageData(image){self.pixels=image.data.slice();},drawImage(image,dx=0,dy=0){if(!self.pixels||self.pixels.length!==self.width*self.height*4){self.pixels=new Uint8ClampedArray(self.width*self.height*4);}
                for(let y=0;y<image.height;y++)for(let x=0;x<image.width;x++){const a=x+dx,b=y+dy;if(a>=0&&b>=0&&a<self.width&&b<self.height){self.pixels.set(image.pixels.slice((y*image.width+x)*4,(y*image.width+x+1)*4),(b*self.width+a)*4);}}}};}
        getBoundingClientRect(){return this.rect||{width:64,height:32,left:0,top:0};}
        addEventListener(k,f){const old=this.listeners[k];this.listeners[k]=old?function(e){old(e);f(e);}:f;}focus(){doc.activeElement=this;}
    }
    const el=k=>{if(!nodes.has(k)){nodes.set(k,new Element(k));}return nodes.get(k);};
    function listen(k,f){const old=events[k];events[k]=old?function(e){old(e);f(e);}:f;}
    doc={getElementById:el,querySelector:()=>({content:bundle}),createElement:k=>new Element(k),activeElement:null,addEventListener:listen};
    const win={devicePixelRatio:1,sessionStorage:{getItem(k){reads.push(k);return storage.get(k)||null;},setItem(k,v){storage.set(k,v);},removeItem(k){storage.delete(k);}},
        setTimeout(f,ms){const n=++timerId;timers.set(n,{f,ms,at:clock+ms});return n;},clearTimeout(n){timers.delete(n);},setInterval(f,ms){const n=++timerId;timers.set(n,{f,ms,at:clock+ms,interval:true});return n;},clearInterval(n){timers.delete(n);},
        requestAnimationFrame(f){const n=++timerId;rafs.set(n,f);return n;},cancelAnimationFrame(n){rafs.delete(n);},addEventListener:listen};
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
                else if(this.path.endsWith('/drc')){assert(grant);v={view_id:view,revision:'drc-rev',data:{checks:grant==='records'?'1':'0',errors:grant==='records'?'1':'0',precision:'1000',format:'ice',truncated_records:'0',read_only:true}};}
                else if(this.path.endsWith('/drc/panel')){assert(grant);if(this.method==='POST'){assert.equal(entry.body.base_panel_rev,reviewRev);reviewRev=P.next(reviewRev);}v={view_id:view,revision:'drc-rev',data:{panel_rev:reviewRev,body:entry.body?entry.body.body:null}};}
                else if(this.path.endsWith('/drc/selection')){assert(grant);if(this.method==='POST'){assert.equal(entry.body.base_selection_rev,groupRev);groupRev=P.next(groupRev);group=entry.body.body.errors||[];}
                    v={view_id:view,revision:'drc-rev',data:{selection_rev:groupRev,total:String(group.length),limit:5000,rules:group.length?[{check:'0',errors:group}]:[]}};}
                else if(this.path.endsWith('/drc/read')){assert(grant);assert.equal(entry.body.view_id,view);const r=entry.body.body;let data;
                    if(r.kind==='rules'){data={rows:grant==='records'?[{check:'0',name:'WIDTH',errors:'1',waived:'0'}]:[],next:null};}
                    else if(r.kind==='rule'){data={check:'0',description:'width'};}
                    else if(r.kind==='list'||r.kind==='records'){data={rows:[reviewRow],next:null};}
                    else if(r.kind==='geometry'){data={...reviewRow,start:'0',total:'4',next:null,precision:'1000',points_dbu:[['20000','8000'],['24000','8000'],['24000','12000'],['20000','12000']]};}
                    else if(r.kind==='focus'){data={check:'0',local:'0',navigation:{kind:'goto',center_um:['22','10'],width_um:'8'}};}
                    else if(r.kind==='filtered_step'){data={hit:reviewRow,next:null,scanned:'1',bbox_um:null,selection_rev:r.selection_rev};}
                    else{throw Error('unexpected guest DRC read '+r.kind);}v={view_id:view,revision:'drc-rev',data};}
                else{v=this.method==='DELETE'?null:{share_id:id,mode,read_only:true,delivery:mode==='follow'?'follow_frames':'explore_frames',...grant&&{drc:{id:'7'.repeat(64),revision:'drc-rev'}}};}}
            this.responseText=v?JSON.stringify(v):'';this.onload();}
    }
    class WS{
        constructor(url,protocols){assert.equal(url,'ws://127.0.0.1:1234/api/v1/guest/'+id+'/events');assert.deepEqual(protocols,['floe.v1','bundle.'+bundle,'guest-csrf.'+auth.csrf]);this.sent=[];this.readyState=1;sockets.push(this);}
        send(text){this.sent.push(JSON.parse(text));}close(){this.readyState=3;}text(v){this.onmessage({data:JSON.stringify(v)});}binary(b){this.onmessage({data:b});}
    }
    const c=Guest.bind({window:win,document:doc,location,history,XHR,WebSocket:WS,protocol:P,now:()=>clock,
        drc:require('./guest-drc.js'),drcSteps:require('./guest-drc-step.js'),geometry:require('./drc-geometry.js'),selection:require('./drc-groups.js'),
        layers:require('./guest-layers.js'),
        display:require('./guest-display.js'),tools:require('./guest-tools.js'),queryWire:require('./guest-query-wire.js'),query:require('./query.js'),
        inspect:require('./inspect.js'),measure:require('./measure.js'),rulers:require('./rulers.js'),gestures:require('./gestures.js'),
        decode(h,data,cb){return Decode.create({ImageData:class{constructor(data){this.data=data;}},setTimeout:win.setTimeout,clearTimeout:win.clearTimeout},h,data,cb);}});
    function hello(ws=sockets.at(-1),connection=epoch,query=mode==='explore'){ws.onopen();ws.text({type:'share.hello',protocol:1,bundle,share_id:id,view_id:view,connection_epoch:connection,mode,read_only:true,query,measure:mode==='explore'});}
    function state(extra={}){return {type:'share.state',view_id:view,connection_epoch:epoch,dataset_revision:'1',worker_epoch:'1',state_rev:'1',render_rev:'1',render_key:'1',bbox_dbu:['0','0','64','32'],dbu_um:'1',camera_um:['32','16','64'],pixels:[64,32],depth:'full',detail:'high',thin:'keep',frames:true,labels:true,mono:false,rendering:false,failure:null,...extra};}
    function raf(){for(const [n,f] of [...rafs]){rafs.delete(n);f();}}
    function timer(ms){const found=[...timers].find(([,t])=>t.ms===ms);assert(found,'timer '+ms);if(!found[1].interval){timers.delete(found[0]);}clock+=ms;found[1].f();}
    function advance(ms=100){const end=clock+ms;while(true){const entry=[...timers].sort((a,b)=>a[1].at-b[1].at)[0];if(!entry||entry[1].at>end){break;}clock=entry[1].at;if(entry[1].interval){entry[1].at+=entry[1].ms;}else{timers.delete(entry[0]);}entry[1].f();}clock=end;}
    function mouse(type,x,y,button=0,extra={}){const e={clientX:x,clientY:y,button,buttons:type==='mouseup'?0:button===0?1:button===1?4:2,preventDefault(){},...extra};
        if(type==='mousedown'){el('guest-viewport').listeners.mousedown(e);}else if(type==='mousemove'){if(el('guest-viewport').listeners.mousemove){el('guest-viewport').listeners.mousemove(e);}events.mousemove(e);}else{if(el('guest-viewport').listeners.mouseup){el('guest-viewport').listeners.mouseup(e);}events.mouseup(e);}}
    return {c,el,win,doc,events,requests,sockets,storage,reads,location,hello,state,raf,timer,advance,mouse,timers,rafs,code(n){sessionCode=n;}};
}
function packet(extra={},color=[1,2,3,255]){
    const h={type:'frame',protocol:1,row0:'top',purpose:'foreground',format:'raw',view_id:view,connection_epoch:epoch,frame_id:'1',dataset_revision:'1',worker_epoch:'1',state_rev:'1',render_rev:'1',render_key:'1',generation:'1',round:'1',deferred:'0',deck_skipped:'0',final:true,partial:false,labels_truncated:false,complete:true,approximate:false,query:false,query_scene:{generation:null,round:null,complete:false,summary_layers:'0'},bbox_dbu:['0','0','64','32'],width:64,height:32,...extra};
    const data=new Uint8Array(16+h.width*h.height*4);data.set(new TextEncoder().encode('FLOERAW1'));const dv=new DataView(data.buffer);dv.setUint32(8,h.width,true);dv.setUint32(12,h.height,true);
    for(let i=16;i<data.length;i+=4){data.set(color,i);}h.payload_length=String(data.length);const text=new TextEncoder().encode(JSON.stringify(h)),out=new Uint8Array(4+text.length+data.length);new DataView(out.buffer).setUint32(0,text.length,true);out.set(text,4);out.set(data,4+text.length);return out.buffer;
}
const exactScene={query:true,query_scene:{generation:'1',round:'1',complete:true,summary_layers:'0'}};
function click(e,x,y,extra={}){e.mouse('mousedown',x,y,0,extra);e.mouse('mouseup',x,y,0,extra);}
function queryReply(ws,request,index='0'){
    const hit=request.body.operation.kind==='snap'?{kind:'snap',snap:'vertex',point_dbu:['16','16']}:
        {kind:'pick',count:'2',index,pair:[7,0],layer_name:'<img> approved layer',cell_name:'synthetic',area_dbu2:'100',bbox_dbu:['0','0','10','10'],
            points_dbu:[['0','0'],['10','0'],['10','10'],['0','10']],points_truncated:false};
    ws.text({type:'query.accepted',seq:request.seq,view_id:view,connection_epoch:epoch,query_id:request.seq});
    ws.text({type:'query.result',seq:request.seq,view_id:view,connection_epoch:epoch,query_id:request.seq,anchor:request.body.anchor,status:'ok',hit,
        scene:exactScene.query_scene,requested_summary_layers:'0'});
}
function measureReply(ws,request,point){ws.text({type:'measure.result',seq:request.seq,view_id:view,connection_epoch:epoch,anchor:request.body.anchor,
    point_dbu:point,snap:request.body.snap_query===null?null:'vertex',segment:request.body.start_dbu?
        {endpoints_dbu:[request.body.start_dbu,point],delta_um:['10','0'],distance_um:'10'}:null});}
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
        assert(!review.el('guest-drc-canvas').hidden);assert.equal(review.el('guest-drc-canvas').style.width,'64px');
        review.sockets[0].onclose();assert(review.el('gd-panel').hidden);assert(review.el('guest-drc-canvas').hidden);assert.equal(review.el('guest-drc-canvas').width,1);}
    // Exercise actual mouse events and the real inspection/ruler controllers,
    // including a real DRC page: gestures may not cancel a box on mousedown.
    for(const mode of ['follow','explore']){
        const r=environment(mode,'#invite='+secret,'records');await r.c.start();r.hello();const w=r.sockets[0];w.text(r.state());await tick();
        w.binary(packet(exactScene));r.raf();click(r,22,22);await tick();assert.match(r.el('gd-selection').textContent,/1 selected/);
        assert.equal(w.sent.length,1,'marker selection must not send a geometry query or move');
        r.el('gd-box').onclick();r.raf();click(r,19,19);r.raf();assert.equal(r.el('gd-box').attrs['aria-pressed'],'true');
        click(r,25,25);await tick();const change=r.requests.filter(v=>v.path.endsWith('/drc/selection')&&v.method==='POST').at(-1);
        assert.deepEqual(change.body.body.bbox_um,['19','7','25','13']);assert.equal(change.body.state_rev,'1');assert.equal(w.sent.length,1);
        let prevented=false;r.el('guest-viewport').listeners.keydown({key:'Tab',preventDefault(){prevented=true;}});await tick();assert(prevented);
        assert(r.requests.some(v=>v.body&&v.body.body&&v.body.body.kind==='filtered_step'));assert.equal(w.sent.length,1);
        if(mode==='explore'){
            r.el('gd-box').onclick();r.raf();r.mouse('mousedown',10,10);r.mouse('mousemove',30,10);r.raf();r.mouse('mouseup',30,10);assert.equal(w.sent.at(-1).type,'explore.set','box mode drag still pans');
        }
        w.onclose();
    }
    // Exercise actual mouse events and the real inspection/ruler controllers,
    // not a second mock API that can skip the displayed-frame ACK boundary.
    const inspect=environment();await inspect.c.start();const iw=inspect.sockets[0];inspect.hello();iw.text(inspect.state());
    click(inspect,16,8);assert.equal(iw.sent.length,0);iw.binary(packet(exactScene));click(inspect,16,8);assert.equal(iw.sent.length,0,'no query before RAF/ACK');inspect.raf();
    click(inspect,16,8);let request=iw.sent.at(-1);assert.equal(request.type,'explore.query');assert.deepEqual(request.body.position,[.25,.25]);
    assert.equal(iw.sent[0].type,'frame.ack');queryReply(iw,request);assert.match(inspect.el('pick-details').textContent,/<img> approved layer/);
    inspect.advance();click(inspect,16,8);request=iw.sent.at(-1);assert.equal(request.body.operation.nth,'1','mouse-down alone must not reset overlap cycling');queryReply(iw,request,'1');
    inspect.raf();assert(!inspect.el('query-canvas').hidden);
    let count=iw.sent.length;inspect.mouse('mousedown',16,8);inspect.mouse('mousemove',30,8);inspect.raf();inspect.mouse('mousemove',16,8);inspect.mouse('mouseup',16,8);
    assert.equal(iw.sent.length,count,'a returned pan is not an object click or a navigation');
    inspect.mouse('mousedown',16,8);inspect.mouse('mousemove',32,8);inspect.raf();inspect.mouse('mouseup',32,8);request=iw.sent.at(-1);
    assert.equal(request.type,'explore.set');assert.deepEqual(request.body.navigation,{kind:'pan',x:-.25,y:0,snap:false});
    count=iw.sent.length;click(inspect,20,8);assert.equal(iw.sent.length,count,'no query behind an unacknowledged display edit');
    iw.text({type:'accepted',seq:request.seq,view_id:view,connection_epoch:epoch,state_rev:'2',render_rev:'2'});
    iw.text(inspect.state({state_rev:'2',render_rev:'2',bbox_dbu:['-16','0','48','32']}));click(inspect,20,8);assert.equal(iw.sent.length,count,'overlap-only old image is not a query receipt');
    iw.binary(packet({...exactScene,frame_id:'2',state_rev:'2',render_rev:'2',bbox_dbu:['-16','0','48','32']}));inspect.raf();
    inspect.advance();click(inspect,20,8);assert.equal(iw.sent.at(-1).body.anchor.frame_id,'2');
    const late=iw.onmessage;iw.onclose();assert(inspect.el('guest-inspect').hidden);assert.equal(inspect.el('pick-details').textContent,'');assert(inspect.el('query-canvas').hidden);
    late({data:JSON.stringify({type:'error',seq:iw.sent.at(-1).seq,code:'stale_frame'})});assert.equal(inspect.el('pick-details').textContent,'');
    // Authoritative margin geometry can answer in the newly exposed strip,
    // including when the old foreground still supplies overlapping labels.
    const crop=environment();await crop.c.start();const cw=crop.sockets[0];crop.hello();cw.text(crop.state());cw.binary(packet(exactScene));crop.raf();
    cw.text(crop.state({margin:{frame_id:'2',origin_px:[16,16],crop_safe:false}}));
    cw.binary(packet({...exactScene,frame_id:'2',purpose:'margin',width:96,height:64,bbox_dbu:['-16','-16','80','48']}));crop.raf();
    cw.text(crop.state({state_rev:'2',bbox_dbu:['16','0','80','32'],margin:{frame_id:'2',origin_px:[32,16],crop_safe:false}}));
    click(crop,63,16);request=cw.sent.at(-1);assert.equal(request.type,'explore.query');assert.equal(request.body.anchor.frame_id,'2');assert.equal(request.body.anchor.state_rev,'2');assert.deepEqual(request.body.position,[63/64,.5]);
    cw.onclose();
    // The base image is centered/contained; pixel positions follow actual CSS
    // scale, not a guessed browser DPR or the outer viewport's black padding.
    for(const box of [{width:64,height:64,left:10,top:20,x:26,y:44,dpr:1},{width:32,height:16,left:10,top:20,x:18,y:24,dpr:2}]){
        const positioned=environment();positioned.el('guest-canvas').rect=positioned.el('guest-viewport').rect=box;
        await positioned.c.start();const pw=positioned.sockets[0];positioned.hello();pw.text(positioned.state());pw.binary(packet(exactScene));positioned.raf();
        if(box.height===64){click(positioned,26,24);assert.equal(pw.sent.length,1,'no pick in letterbox');}
        click(positioned,box.x,box.y);request=pw.sent.at(-1);assert.equal(request.type,'explore.query');assert.deepEqual(request.body.position,[.25,.25]);assert.equal(request.body.radius_px,3*box.dpr);
        queryReply(pw,request);positioned.raf();assert.equal(positioned.el('query-canvas').style.width,64/box.dpr+'px');
        assert.equal(positioned.el('query-canvas').style.top,box.height===64?'16px':'0px');
        assert(positioned.el('guest-drc-canvas').hidden);assert.equal(positioned.el('guest-drc-canvas').width,1);pw.onclose();
    }
    const deck=environment();await deck.c.start();const dw=deck.sockets[0];deck.hello(dw,epoch,false);dw.text(deck.state());dw.binary(packet());deck.raf();
    assert(deck.el('snap-probe').disabled);deck.el('ruler-mode').onclick();assert(deck.el('ruler-snap').disabled);assert(!deck.el('ruler-snap').checked);
    click(deck,16,16);request=dw.sent.at(-1);assert.equal(request.type,'explore.measure');assert.equal(request.body.snap_query,null);measureReply(dw,request,['16','16']);
    deck.advance();click(deck,26,16,{shiftKey:true});request=dw.sent.at(-1);assert.equal(request.body.free_angle,true);assert.deepEqual(request.body.start_dbu,['16','16']);measureReply(dw,request,['26','16']);
    assert.match(deck.el('ruler-count').textContent,/1 rulers/);deck.raf();assert(!deck.el('ruler-canvas').hidden);dw.onclose();assert.equal(deck.el('ruler-details').textContent,'');assert.equal(deck.rafs.size,0);
    const follow=environment('follow');await follow.c.start();const fw=follow.sockets[0];follow.hello();fw.text(follow.state());fw.binary(packet());follow.raf();
    click(follow,16,16);follow.el('ruler-mode').onclick();follow.el('guest-viewport').listeners.wheel({preventDefault(){throw Error('Follow wheel should not dispatch');}});
    assert.equal(fw.sent.length,1);assert(follow.el('guest-inspect').hidden);assert(follow.el('guest-rulers').hidden);fw.onclose();
    // Wheel keeps its cursor anchor and drops queued/rendering input. Box zoom
    // and Escape are separate from picking and commit only on release.
    const pointer=environment();await pointer.c.start();const ww=pointer.sockets[0];pointer.hello();ww.text(pointer.state());ww.binary(packet(exactScene));pointer.raf();
    const wheel={deltaY:120,deltaMode:0,clientX:16,clientY:8,buttons:0,preventDefault(){}};pointer.el('guest-viewport').listeners.wheel(wheel);
    request=ww.sent.at(-1);assert.equal(request.type,'explore.set');assert.deepEqual(request.body.navigation.anchor,[.25,.25]);assert.equal(request.body.navigation.factor,Math.pow(.96,-1));
    count=ww.sent.length;pointer.el('guest-viewport').listeners.wheel(wheel);assert.equal(ww.sent.length,count);ww.onclose();
    const band=environment();await band.c.start();const bw=band.sockets[0];band.hello();bw.text(band.state());bw.binary(packet(exactScene));band.raf();
    band.mouse('mousedown',12,8,2);band.mouse('mousemove',40,24,2);band.raf();assert(!band.el('zoom-band').hidden);assert.equal(bw.sent.length,1);
    band.el('guest-viewport').listeners.keydown({key:'Escape',preventDefault(){}});assert(band.el('zoom-band').hidden);
    band.mouse('mouseup',40,24,2);assert.equal(bw.sent.length,1,'Escape cancels without a pick/navigation');
    band.mouse('mousedown',12,8,2);band.mouse('mousemove',40,24,2);band.raf();
    band.mouse('mouseup',40,24,2);assert.equal(bw.sent.at(-1).body.navigation.kind,'band');assert(band.el('zoom-band').hidden);bw.onclose();
    const limits=environment();await limits.c.start();const lw=limits.sockets[0];limits.hello();lw.text(limits.state());lw.binary(packet(exactScene));limits.raf();
    lw.text(limits.state({rendering:true}));limits.el('guest-viewport').listeners.wheel(wheel);assert.equal(lw.sent.length,1,'wheel waits for final display');
    lw.text(limits.state());lw.bufferedAmount=16385;click(limits,16,8);assert.equal(lw.sent.length,1,'query never adds to saturated socket');
    assert.equal(lw.readyState,1);lw.bufferedAmount=0;limits.advance();click(limits,16,8);assert.equal(lw.sent.at(-1).type,'explore.query');lw.onclose();
    console.log('WEB GUEST UI: ALL OK (isolated auth, follow/explore, displayed ACK/margin queries, pick cycling, native rulers, DPR/letterbox overlays, pan/band/wheel, cancellation/backpressure, stale/revoke cleanup)');
})().catch(e=>{console.error(e);process.exitCode=1;});
