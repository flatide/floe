'use strict';
const assert=require('node:assert/strict'),P=require('./protocol.js'),Client=require('./server.js'),Decode=require('./image-decode.js');
const packet=require('./guest.test.cjs').packet;
const id='a'.repeat(64),epoch='c'.repeat(64),secret='d'.repeat(64),bundle='test-bundle';
const auth={launch_id:id,csrf:'f'.repeat(64),bundle,protocol:'floe-server-v1',viewer_ready:true,render_transport:true};
const origin='https://service.example.test',base='/api/v1/server/sessions/'+id,key='floe-server-session:'+origin+':'+id;
const tick=()=>new Promise(r=>setImmediate(r));
function environment(options={}) {
    const origin=options.http?'http://10.0.0.10:8080':'https://service.example.test';
    const id=options.id||'a'.repeat(64),auth={launch_id:id,csrf:'f'.repeat(64),bundle,protocol:'floe-server-v1',viewer_ready:true,render_transport:true};
    const base='/api/v1/server/sessions/'+id,key='floe-server-session:'+origin+':'+id;
    const nodes=new Map(),events={},requests=[],sockets=[],timers=new Map(),rafs=new Map(),reads=[],deferred=[],holds=[],decoders=[];
    const storage=new Map([['floe-session:'+origin,'OWNER'],['floe-server-session:'+origin+':'+'b'.repeat(64),'OTHER']]);
    if(options.resume){storage.set(key,JSON.stringify(auth));}
    let number=0,clock=10000,opened=!!options.resume,code=200,confirm=true,doc;
    class Element {
        constructor(tag){this.tag=tag||'div';this.value='';this.checked=false;this.hidden=false;this.disabled=false;this.style={};this.listeners={};this.attributes={};this.width=this.height=1;this.children=[];this.dataset={};this.className='';}
        appendChild(c){this.children.push(c);return c;}
        contains(n){return this===n||this.children.some(c=>c.contains(n));}
        querySelectorAll(tag){return this.children.flatMap(c=>[...(c.tag===tag?[c]:[]),...c.querySelectorAll(tag)]);}
        removeAttribute(k){delete this.attributes[k];}
        click(){if(this.onclick){this.onclick({preventDefault(){}});}}
        get textContent(){return this._text||'';}set textContent(v){this._text=v;this.children=[];}
        setAttribute(k,v){this.attributes[k]=String(v);}getAttribute(k){return this.attributes[k]===undefined?null:this.attributes[k];}
        get width(){return this._width;}set width(v){this._width=v;this.resets=(this.resets||0)+1;this.pixels=new Uint8ClampedArray((this._width||1)*(this._height||1)*4);}
        get height(){return this._height;}set height(v){this._height=v;this.resets=(this.resets||0)+1;this.pixels=new Uint8ClampedArray((this._width||1)*(this._height||1)*4);}
        getContext(){const self=this;return {fillRect(){self.pixels=new Uint8ClampedArray(self.width*self.height*4);},putImageData(i){self.pixels=i.data.slice();},
            drawImage(i,x=0,y=0){for(let yy=0;yy<i.height;yy++){for(let xx=0;xx<i.width;xx++){
                const dx=xx+x,dy=yy+y;if(dx<0||dy<0||dx>=self.width||dy>=self.height){continue;}
                const src=(yy*i.width+xx)*4,dst=(dy*self.width+dx)*4;self.pixels.set(i.pixels.subarray(src,src+4),dst);
            }}self.blits=(self.blits||0)+1;self.lastBlit=[x||0,y||0];}};}
        getBoundingClientRect(){return {width:options.width||64,height:options.height||32,left:0,top:0,right:options.width||64,bottom:options.height||32};}focus(){doc.activeElement=this;}select(){}
        addEventListener(k,f){this.listeners[k]=f;}
    }
    const el=k=>{if(!nodes.has(k)){nodes.set(k,new Element());}return nodes.get(k);};
    function listen(k,f){const old=events[k];events[k]=old?e=>{old(e);f(e);}:f;}
    doc={hidden:false,activeElement:null,querySelector:s=>s==='meta[name="floe-http-test"]'?(options.httpTest===undefined?null:{content:String(options.httpTest)}):{content:bundle},getElementById:el,createElement:tag=>new Element(tag),addEventListener:listen};
    function timer(f,ms,interval=false){timers.set(++number,{f,ms,interval});return number;}
    const win={devicePixelRatio:1,performance:{now:()=>clock},sessionStorage:{getItem(k){reads.push(k);return storage.get(k)||null;},setItem(k,v){if(options.noStorage){throw Error('denied');}storage.set(k,v);},removeItem(k){storage.delete(k);}},
        setTimeout:timer,clearTimeout:n=>timers.delete(n),setInterval:(f,ms)=>timer(f,ms,true),clearInterval:n=>timers.delete(n),
        requestAnimationFrame:f=>{rafs.set(++number,f);return number;},cancelAnimationFrame:n=>rafs.delete(n),addEventListener:listen,confirm:()=>confirm};
    const location={origin,protocol:options.http?'http:':'https:',pathname:'/server/'+id,hash:options.resume?'':options.hash===undefined?'#bootstrap='+secret:options.hash};
    const history={replaceState(a,b,path){assert.equal(path,location.pathname);location.hash='';}};
    function state(extra={}){return {type:'snapshot',view_id:id,connection_epoch:epoch,state_rev:'1',render_rev:'1',render_key:'1',dataset_revision:'1',worker_epoch:'1',
        bbox_dbu:['0','0','64','32'],pixels:[64,32],camera_um:['32','16','64'],font_px:14,max_depth:6,depth:'full',detail:'high',thin:'auto',labels:true,frames:true,mono:false,status:'idle',failure:null,
        capabilities:{query:false,clip:false,mode:false,labels:true},...extra};}
    class XHR {
        open(method,path){this.method=method;this.path=path;this.headers={};}setRequestHeader(k,v){this.headers[k]=v;}
        abort(){this.onabort();}
        send(body){assert.equal(location.hash,'','fragment removed before any request');const value=body?JSON.parse(body):null;
            const entry={method:this.method,path:this.path,body:value,headers:this.headers};requests.push(entry);let status=200,data;
            assert(this.path.startsWith(base));assert(!('X-Floe-Delegation-Key' in this.headers));
            if(this.path===base+'/exchange'){assert.equal(this.method,'POST');assert.deepEqual(value,{bootstrap:secret});data=auth;}
            else {assert.equal(this.headers['X-Floe-CSRF'],auth.csrf);status=code;
                if(this.path===base){data=this.method==='DELETE'?null:{launch_id:id,viewer_ready:true,render_transport:true,public_demo:!!options.demo,principal:{namespace:'teebox',subject:'<img onerror=bad>'}};}
                else if(this.path===base+'/view'){if(this.method==='POST'){opened=true;status=202;data={status:'opening',view_id:id};}else{data=opened?state():{type:'unopened',view_id:id};}}
                else if(this.path===base+'/palette'){assert.equal(this.method,'POST');assert.equal(value.kind,'page');data={state_rev:'1',render_key:'1',total:0,all_total:0,start:value.start,next:null,rows:[]};}
                else if(this.path===base+'/presets'||this.path.startsWith(base+'/minimap/')||this.path.startsWith(base+'/fill-slots/')){status=404;data={error:'unavailable'};}
                else if(this.path===base+'/cells'){assert.equal(this.method,'POST');assert.equal(value.view_id,id);data=({
                    sources:{sources:[{src:0,placements:1,name:'source 0'}]},
                    children:{cell:7,name:'TOP',insts:1,height:2,unit:0.001,bbox:[0,0,64,32],n:2,total:2,children:[{ci:1,members:4,leaf:false,name:'BLK'},{ci:2,members:1,leaf:true,name:'VIA'}]},
                    bbox:{insts:1,approx:false,bbox:[10,10,30,20]},insts:{n:1,more:false,visited:1,boxes:[[10,10,30,20]]}})[value.kind];
                    if(!data){throw Error('Unexpected cell question '+value.kind);}}
                else {throw Error('Unexpected path');}}
            entry.reply=(s=status,v=data)=>{this.status=s;this.responseText=v?JSON.stringify(v):'';this.onload();};entry.fail=()=>this.ontimeout();
            const h=holds.findIndex(h=>h.method===this.method&&this.path.endsWith(h.suffix));if(h>=0){holds.splice(h,1);deferred.push(entry);}else{entry.reply();}
        }
    }
    class WS {
        constructor(url,protocols){assert.equal(url,(options.http?'ws://10.0.0.10:8080':'wss://service.example.test')+base+'/stream');assert.deepEqual(protocols,['floe-server-v1','bundle.'+bundle,'csrf.'+auth.csrf]);this.sent=[];this.readyState=1;this.bufferedAmount=0;sockets.push(this);}
        send(text){this.sent.push(JSON.parse(text));}close(){this.readyState=3;}text(v){this.onmessage({data:JSON.stringify(v)});}binary(extra={}){this.onmessage({data:packet({view_id:id,...extra})});}
    }
    const panels=options.panels?{palette:require('./palette.js'),presets:require('./presets.js'),fillEditor:require('./fill-editor.js'),minimap:require('./minimap.js'),panes:require('./panes.js'),menubar:require('./menubar.js'),cells:require('./cells.js')}:{};
    const c=Client.bind({window:win,document:doc,location,history,protocol:P,viewer:require('./viewer.js'),XHR,WebSocket:WS,now:()=>clock,gestures:require('./gestures.js'),...panels,
        decode(h,d,done){const job=Decode.create({ImageData:class{constructor(data){this.data=data;}},setTimeout:win.setTimeout,clearTimeout:win.clearTimeout},h,d,done);
            return options.holdDecode?{start(){decoders.push({finish:()=>job.start(),fail:()=>done(null,Error('synthetic decode error'))});},cancel:()=>job.cancel()}:job;}});
    function hello(ws=sockets.at(-1),e=epoch){ws.onopen();ws.text({type:'hello',protocol:1,bundle,view_id:id,connection_epoch:e,frame_credit:1,capabilities:{view:true,index:false,review:false,export:false,query:false}});ws.text(state({connection_epoch:e}));}
    function fire(ms,interval){const item=[...timers].find(([,v])=>v.ms===ms&&(interval===undefined||v.interval===interval));assert(item,'timer '+ms);if(!item[1].interval){timers.delete(item[0]);}clock+=ms;item[1].f();}
    const names={in:'zoom-in',out:'zoom-out',x:'goto-x',y:'goto-y',width:'goto-width',go:'goto'};
    return {c,el:n=>el('server-'+(names[n]||n)),events,doc,win,location,requests,sockets,storage,reads,deferred,timers,decoders,hello,state,
        raf(){for(const [k,f] of [...rafs]){rafs.delete(k);f();}},fire,code(n){code=n;},confirm(v){confirm=v;},opened(v){opened=v;},
        defer(method,suffix){holds.push({method,suffix});}};
}
module.exports={environment};
if(require.main===module)(async()=>{
    const http=environment({http:true,httpTest:true,demo:true});await http.c.start();assert.equal(http.sockets.length,1);
    assert.equal(http.doc.getElementById('transport-warning').hidden,false);http.hello();http.sockets[0].binary();http.raf();
    assert.equal(http.el('empty').hidden,true);http.el('in').onclick();assert.equal(http.sockets[0].sent.at(-1).type,'view.set');
    http.sockets[0].onclose();http.fire(500);await tick();assert.equal(http.sockets.length,2);await http.el('leave').onclick();
    for(const options of [{http:true},{http:true,httpTest:false},{httpTest:true}]){const bad=environment(options);await bad.c.start();assert.equal(bad.requests.length,0);assert.equal(bad.sockets.length,0);assert.equal(bad.location.hash,'');}
    const demo=environment({demo:true,width:7680,height:4320});await demo.c.start();
    const px=demo.requests.find(r=>r.method==='POST'&&r.path.endsWith('/view')).body;
    assert(px.width<=2048&&px.height<=2048&&px.width*px.height<=2097152);demo.c.stop();
    const e=environment();await e.c.start();assert.equal(e.requests.filter(r=>r.method==='POST'&&r.path.endsWith('/view')).length,1);
    assert.equal(e.reads.length,0);assert(e.storage.has(key));assert.equal(e.el('user').textContent,'teebox / <img onerror=bad>');e.hello();const ws=e.sockets[0];
    ws.binary();assert.equal(ws.sent.length,0);e.raf();assert.equal(ws.sent[0].type,'frame.ack');assert(!('disposition' in ws.sent[0]));assert.deepEqual([...e.el('canvas').pixels.slice(0,4)],[1,2,3,255]);
    e.el('in').onclick();const command=ws.sent.at(-1);assert.deepEqual(command.body,{navigation:{kind:'zoom',factor:0.8,anchor:[0.5,0.5]}});
    e.el('out').onclick();assert.equal(ws.sent.length,2);ws.text({type:'accepted',seq:command.seq,state_rev:'2',render_rev:'2'});assert.equal(ws.sent.length,2);
    ws.text(e.state({state_rev:'2',render_rev:'2'}));e.fire(100);assert.equal(ws.sent.length,3);assert.equal(ws.sent.at(-1).base_state_rev,'2');
    ws.onclose();assert.equal(e.el('canvas').width,1);e.code(503);e.fire(500);await tick();e.code(200);e.fire(1000);await tick();assert.equal(e.sockets.length,2);
    e.hello(e.sockets[1],'9'.repeat(64));assert.equal(e.sockets[1].sent.length,0,'no edit replay');assert.equal(e.requests.filter(r=>r.path.endsWith('/exchange')).length,1);assert.equal(e.requests.filter(r=>r.method==='POST'&&r.path.endsWith('/view')).length,1);
    e.confirm(false);await e.el('leave').onclick();assert.equal(e.requests.filter(r=>r.method==='DELETE').length,0);
    e.confirm(true);await e.el('leave').onclick();assert.equal(e.requests.filter(r=>r.method==='DELETE').length,1);assert(!e.storage.has(key));assert.equal(e.storage.get('floe-session:'+origin),'OWNER');assert.equal(e.storage.get('floe-server-session:'+origin+':'+'b'.repeat(64)),'OTHER');

    for(const resume of [false,true]){const a=environment({resume});await a.c.start();a.hello();const sock=a.sockets[0];sock.binary();sock.text(a.state({state_rev:'2',render_rev:'2'}));a.raf();assert.equal(a.el('empty').hidden,false,'stale decode not presented');assert.equal(sock.sent.at(-1).type,'frame.ack');
        sock.onclose();a.code(401);a.fire(500);await tick();assert(!a.storage.has(key));assert.equal(a.el('in').disabled,true);a.c.stop();}
    // Unknown POST outcome: inspect current state, never submit it twice.
    const lost=environment();lost.defer('POST','/view');const beginning=lost.c.start();await tick();lost.deferred[0].fail();await beginning;lost.fire(500);await tick();assert.equal(lost.sockets.length,1);assert.equal(lost.requests.filter(r=>r.method==='POST'&&r.path.endsWith('/view')).length,1);lost.c.stop();
    const busy=environment();busy.defer('POST','/view');const waiting=busy.c.start();await tick();busy.opened(false);busy.deferred[0].reply(429,{});await waiting;busy.fire(500);await tick();assert.equal(busy.el('open').hidden,false);assert.equal(busy.sockets.length,0);busy.el('open').onclick();await tick();assert.equal(busy.sockets.length,1);busy.c.stop();
    // Bootstrap remains a single exchange even if the tab hides mid-request.
    const hide=environment();hide.defer('POST','/exchange');const pending=hide.c.start();hide.doc.hidden=true;hide.events.visibilitychange();hide.deferred[0].reply();await pending;assert.equal(hide.sockets.length,0);
    hide.doc.hidden=false;hide.events.visibilitychange();await tick();assert.equal(hide.sockets.length,1);assert.equal(hide.requests.filter(r=>r.path.endsWith('/exchange')).length,1);hide.c.stop();
    for(const hash of ['#invite='+secret,'#bootstrap=bad','#bootstrap='+secret+'&user=other']){const bad=environment({hash});await bad.c.start();assert.equal(bad.location.hash,'');assert.equal(bad.requests.length,0);}
    const failed=environment();failed.defer('POST','/exchange');const attempt=failed.c.start();failed.deferred[0].fail();await attempt;assert.equal(failed.requests.length,1);assert.equal(failed.timers.size,0);
    const nostore=environment({noStorage:true});await nostore.c.start();assert.equal(nostore.sockets.length,1);nostore.c.stop();
    const stale=environment();await stale.c.start();stale.hello();const old=stale.sockets[0].onmessage;stale.sockets[0].onclose();stale.fire(500);await tick();stale.hello(stale.sockets[1],'9'.repeat(64));old({data:packet({view_id:id})});assert.equal(stale.sockets[1].sent.length,0);stale.c.stop();
    const logout=environment();await logout.c.start();logout.hello();logout.code(503);await logout.el('leave').onclick();assert(!logout.storage.has(key));assert.equal(logout.requests.filter(r=>r.method==='DELETE').length,1);assert.equal(logout.el('canvas').width,1);assert.match(logout.el('status').textContent,/unconfirmed/);
    const nav=environment();await nav.c.start();nav.hello();nav.el('x').value='1.25';nav.el('y').value='-4';nav.el('width').value='20';nav.el('goto-form').onsubmit({preventDefault(){}});
    assert.deepEqual(nav.sockets[0].sent.at(-1).body.navigation,{kind:'goto',center_um:['1.25','-4'],width_um:'20'});nav.c.stop();
    const mouse=environment();await mouse.c.start();mouse.hello();mouse.sockets[0].binary();mouse.raf();
    const down={button:0,buttons:1,clientX:8,clientY:8,preventDefault(){}};
    mouse.el('viewport').listeners.mousedown(down);mouse.events.mousemove({...down,clientX:24});mouse.events.mouseup({...down,buttons:0,clientX:24});
    assert.deepEqual(mouse.sockets[0].sent.at(-1).body.navigation,{kind:'pan',x:-0.25,y:0,snap:false});mouse.c.stop();
    const renderFailure=environment();await renderFailure.c.start();renderFailure.hello();renderFailure.sockets[0].binary();renderFailure.raf();
    renderFailure.sockets[0].text(renderFailure.state({failure:'worker_failed',status:'failed'}));assert.equal(renderFailure.el('in').disabled,true);assert.match(renderFailure.el('frame-status').textContent,/Last image.*renderer failed/);renderFailure.c.stop();
    const handshake=environment();await handshake.c.start();handshake.fire(10000);assert.equal(handshake.sockets[0].readyState,3);handshake.fire(500);await tick();assert.equal(handshake.sockets.length,2);handshake.c.stop();
    const noReply=environment();await noReply.c.start();noReply.hello();noReply.el('in').onclick();
    noReply.fire(10000,false);assert.equal(noReply.sockets[0].readyState,3);noReply.fire(500);await tick();noReply.hello(noReply.sockets[1],'9'.repeat(64));assert.equal(noReply.sockets[1].sent.length,0);noReply.c.stop();
    // Only already-presented pixels can survive display-policy edits; stale
    // packets still cannot be newly decoded/presented and identity stays fixed.
    async function displayed(options={}){const h=environment(options);await h.c.start();h.hello();h.sockets[0].binary();if(options.holdDecode){h.decoders.shift().finish();}h.raf();return h;}
    function visiblePixels(h){const c=h.el('canvas'),out=new Uint8ClampedArray(64*32*4),x=parseInt(c.style.left)||0,y=parseInt(c.style.top)||0;
        for(let yy=0;yy<c.height;yy++){for(let xx=0;xx<c.width;xx++){const dx=xx+x,dy=yy+y;if(dx>=0&&dx<64&&dy>=0&&dy<32){out.set(c.pixels.subarray((yy*c.width+xx)*4,(yy*c.width+xx)*4+4),(dy*64+dx)*4);}}}return out;}
    const pointer=(x,y,button=0,buttons=1)=>({clientX:x,clientY:y,button,buttons,preventDefault(){}});
    const zoomBounds=['6.4','3.2','57.6','28.8'];
    for(const input of ['in','out','+','-','wheel']){
        const h=await displayed(),s=h.sockets[0],c=h.el('canvas'),pixels=c.pixels.slice(),resets=c.resets;
        if(input==='wheel'){h.el('viewport').listeners.wheel({...pointer(32,16,0,0),deltaY:-1,deltaMode:0});}
        else if(input==='+'||input==='-'){h.el('viewport').listeners.keydown({key:input,preventDefault(){}});}
        else{h.el(input).onclick();}
        const edit=s.sent.at(-1);assert.equal(edit.body.navigation.kind,'zoom');
        s.text({type:'accepted',seq:edit.seq,state_rev:'2'});
        s.text(h.state({state_rev:'2',render_rev:'2',bbox_dbu:zoomBounds,status:'rendering'}));
        assert.equal(h.el('empty').hidden,true,input);assert.equal(c.resets,resets,input+' cleared the canvas');assert.deepEqual(c.pixels,pixels);
        assert.match(h.el('frame-status').textContent,/Previous image.*waiting/);
        s.binary();h.raf();assert.equal(c.resets,resets,'stale packet replaced retained pixels');
        const blits=c.blits;s.binary({state_rev:'2',render_rev:'2',bbox_dbu:zoomBounds});h.raf();assert(c.blits>blits);
        assert.match(h.el('frame-status').textContent,/^Live/);h.c.stop();assert.equal(c.width,1);
    }
    for(const button of [0,1]){
        const h=await displayed(),s=h.sockets[0],c=h.el('canvas'),v=h.el('viewport'),mask=button===0?1:4;
        v.listeners.mousedown(pointer(8,8,button,mask));h.events.mousemove(pointer(21,8,button,mask));h.raf();
        assert.equal(c.style.left,'13px');const preview=visiblePixels(h);h.events.mouseup(pointer(21,8,button,0));
        const edit=s.sent.at(-1),resets=c.resets,blits=c.blits;
        assert.deepEqual(visiblePixels(h),preview,'release recentered preview');
        s.text(h.state({status:'rendering'}));assert.equal(c.resets,resets,'old snapshot erased pending pan');
        s.text({type:'accepted',seq:edit.seq,state_rev:'2'});
        const moved={state_rev:'2',render_rev:'2',bbox_dbu:['-13','0','51','32']};
        s.text(h.state({...moved,status:'rendering'}));assert.equal(c.resets,resets);assert.equal(c.blits,blits);
        assert.equal(h.el('empty').hidden,true);assert.match(h.el('frame-status').textContent,/Previous image/);
        s.binary(moved);h.raf();assert.deepEqual(c.lastBlit,[0,0]);assert.match(h.el('frame-status').textContent,/^Live/);h.c.stop();
    }
    for(const outcome of ['noop','reject']){
        const h=await displayed(),s=h.sockets[0],c=h.el('canvas'),original=visiblePixels(h);
        h.el('viewport').listeners.mousedown(pointer(8,8));h.events.mouseup(pointer(21,8,0,0));
        assert.notDeepEqual(visiblePixels(h),original);const edit=s.sent.at(-1);
        s.text(outcome==='noop'?{type:'accepted',seq:edit.seq,state_rev:'1'}:{type:'error',seq:edit.seq});
        assert.deepEqual(visiblePixels(h),original);assert.equal(h.el('empty').hidden,true);h.c.stop();
    }
    const snapped=await displayed(),sn=snapped.sockets[0];
    snapped.el('viewport').listeners.keydown({key:'ArrowLeft',preventDefault(){}});
    const snEdit=sn.sent.at(-1);assert.equal(snEdit.body.navigation.snap,true);
    sn.text({type:'accepted',seq:snEdit.seq,state_rev:'2'});sn.text(snapped.state({state_rev:'2',render_rev:'2',bbox_dbu:['-32','0','32','32']}));
    assert.equal(snapped.el('canvas').style.left,'32px');snapped.c.stop();
    for(const change of [{dataset_revision:'2'},{worker_epoch:'2'},{status:'closed'}]){
        const h=await displayed(),s=h.sockets[0];s.text(h.state({state_rev:'2',render_rev:'2',bbox_dbu:zoomBounds}));assert.equal(h.el('empty').hidden,true);
        s.text(h.state({state_rev:'3',...change}));assert.equal(h.el('canvas').width,1);assert.equal(h.el('empty').hidden,false);
        s.text(h.state({state_rev:'4'}));assert.equal(h.el('empty').hidden,false,'cleared image resurfaced');h.c.stop();
    }
    for(const lifecycle of ['hide','disconnect','logout']){
        const h=await displayed(),s=h.sockets[0];s.text(h.state({state_rev:'2',render_rev:'2',bbox_dbu:zoomBounds}));
        if(lifecycle==='hide'){h.doc.hidden=true;h.events.visibilitychange();}else if(lifecycle==='disconnect'){s.onclose();}else{await h.el('leave').onclick();}
        assert.equal(h.el('canvas').width,1);assert.equal(h.el('empty').hidden,false);h.c.stop();
    }
    for(const outward of [false,true]){
        const h=await displayed(),s=h.sockets[0],v=h.el('viewport');let prevented=false;
        v.listeners.contextmenu({preventDefault(){prevented=true;}});assert(prevented,'native image menu not suppressed');
        v.listeners.mousedown(pointer(32,8,2,2));h.events.mousemove(pointer(outward?16:48,24,2,2));h.raf();
        assert.equal(h.el('zoom-band').hidden,false);assert.equal(v.style.cursor,'crosshair');assert.equal(h.el('zoom-band').style.width,'16px');
        assert.match(h.el('zoom-band-hint').textContent,outward?/Zoom out/:/Zoom in/);assert.equal(s.sent.length,1,'band edited before release');
        h.events.mouseup(pointer(outward?16:48,24,2,0));
        assert.deepEqual(s.sent.at(-1).body.navigation,{kind:'band',start:[0.5,0.25],end:[outward?0.25:0.75,0.75],axes:[true,true],outward});
        assert.equal(h.el('zoom-band').hidden,true);h.c.stop();
    }
    for(const cancel of ['Escape','blur','snapshot']){
        const h=await displayed(),s=h.sockets[0],v=h.el('viewport');v.listeners.mousedown(pointer(8,8,2,2));h.events.mousemove(pointer(24,24,2,2));h.raf();
        if(cancel==='Escape'){
            v.listeners.keydown({key:'Escape',isComposing:true,preventDefault(){assert.fail('IME Escape intercepted');}});assert.equal(h.el('zoom-band').hidden,false);
            v.listeners.keydown({key:'Escape',preventDefault(){}});
        }else if(cancel==='blur'){h.events.blur();}else{s.text(h.state({state_rev:'2',render_rev:'2'}));}
        h.events.mouseup(pointer(24,24,2,0));assert.equal(s.sent.length,1,'cancelled band submitted');assert.equal(h.el('zoom-band').hidden,true);h.c.stop();
    }
    const letterbox=await displayed({width:128,height:96}),lb=letterbox.el('viewport');
    lb.listeners.mousedown(pointer(32,32,2,2));letterbox.events.mousemove(pointer(96,64,2,2));letterbox.raf();
    assert.equal(letterbox.el('zoom-band').style.top,'32px');assert.equal(letterbox.el('zoom-band').style.left,'32px');
    letterbox.events.mouseup(pointer(96,64,2,0));assert.deepEqual(letterbox.sockets[0].sent.at(-1).body.navigation,{kind:'band',start:[0.25,0.25],end:[0.75,0.75],axes:[true,true],outward:false});letterbox.c.stop();
    const unpainted=environment();await unpainted.c.start();unpainted.hello();unpainted.el('viewport').listeners.mousedown(pointer(8,8,2,2));unpainted.events.mouseup(pointer(24,24,2,0));assert.equal(unpainted.sockets[0].sent.length,0);unpainted.c.stop();
    const oldView=await displayed();oldView.sockets[0].text(oldView.state({state_rev:'2',render_rev:'2',bbox_dbu:zoomBounds}));
    oldView.el('viewport').listeners.mousedown(pointer(8,8,2,2));oldView.events.mouseup(pointer(24,24,2,0));assert.equal(oldView.sockets[0].sent.length,1,'band used stale display');oldView.c.stop();
    function cursor(h,busy){assert.equal(h.el('shell').getAttribute('data-busy'),String(busy));assert.equal(h.el('viewport').getAttribute('aria-busy'),String(busy));assert.equal(h.el('viewport').style.cursor,busy?'wait':'');}
    for(const button of [0,1,2]){
        const h=await displayed(),s=h.sockets[0],buttons=[1,4,2][button],port=h.el('viewport');
        port.listeners.mousedown(pointer(8,8,button,buttons));assert.equal(port.style.cursor,button===2?'crosshair':'grabbing');
        h.events.mouseup(pointer(24,24,button,0));cursor(h,true);
        s.text({type:'accepted',seq:s.sent.at(-1).seq,state_rev:'1'});cursor(h,false);h.c.stop();
    }
    for(const [field,value] of [['detail','medium'],['detail','low'],['detail','exact'],['thin','keep'],['thin','cull'],['depth','1'],['frames',false],['labels',false],['mono',true]]){
        const h=await displayed(),s=h.sockets[0],c=h.el('canvas'),pixels=c.pixels.slice(),resets=c.resets;
        if(typeof value==='boolean'){h.el(field).checked=value;}else{h.el(field).value=value;}h.el(field).onchange();
        const edit=s.sent.at(-1),changed={state_rev:'2',render_rev:'2',render_key:'2',[field]:value};
        assert.deepEqual(edit.body,{[field]:value});cursor(h,true);
        // An idle snapshot is not proof that its pixels reached this browser.
        s.text(h.state(changed));assert.equal(c.resets,resets,field+' cleared displayed pixels');assert.deepEqual(c.pixels,pixels);assert.equal(h.el('empty').hidden,true);
        assert.match(h.el('frame-status').textContent,/Previous image.*waiting/);cursor(h,true);
        s.text({type:'accepted',seq:edit.seq,state_rev:'2'});cursor(h,true);
        s.binary();h.raf();assert.equal(c.resets,resets,'stale policy frame was presented');cursor(h,true);
        s.binary(changed);cursor(h,true);h.raf();cursor(h,false);assert.match(h.el('frame-status').textContent,/^Live/);h.c.stop();cursor(h,false);
    }
    const startup=environment();cursor(startup,false);startup.defer('POST','/exchange');const starting=startup.c.start();cursor(startup,true);
    startup.deferred[0].reply();await starting;cursor(startup,true);startup.hello();cursor(startup,true);startup.sockets[0].binary();cursor(startup,true);startup.raf();cursor(startup,false);startup.c.stop();
    for(const outcome of ['noop','reject']){
        const h=await displayed(),s=h.sockets[0];h.el('detail').value='high';h.el('detail').onchange();cursor(h,true);
        s.text(outcome==='noop'?{type:'accepted',seq:s.sent.at(-1).seq,state_rev:'1'}:{type:'error',seq:s.sent.at(-1).seq});cursor(h,false);h.c.stop();
    }
    const phases=await displayed(),ps=phases.sockets[0];
    for(const status of ['rendering','cancelling','opening']){ps.text(phases.state({status}));cursor(phases,true);}
    ps.text(phases.state());cursor(phases,false);
    ps.binary({final:false,complete:false});phases.raf();cursor(phases,true);
    // A terminal but incomplete frame must not leave an eternal wait cursor.
    ps.binary({final:true,complete:false,labels_truncated:true});phases.raf();cursor(phases,false);phases.c.stop();
    const delayed=await displayed({holdDecode:true}),ds=delayed.sockets[0];cursor(delayed,false);
    ds.binary();cursor(delayed,true);delayed.decoders.shift().finish();cursor(delayed,true);delayed.raf();cursor(delayed,false);
    delayed.el('thin').value='keep';delayed.el('thin').onchange();cursor(delayed,true);
    ds.text({type:'accepted',seq:ds.sent.at(-1).seq,state_rev:'2'});ds.text(delayed.state({state_rev:'2',render_rev:'2',render_key:'2',thin:'keep'}));
    ds.binary({state_rev:'2',render_rev:'2',render_key:'2'});cursor(delayed,true);const superseded=delayed.decoders.shift();
    ds.text(delayed.state({state_rev:'3',render_rev:'3',render_key:'3',thin:'cull'}));const preserved=delayed.el('canvas').resets;
    superseded.finish();delayed.raf();cursor(delayed,true);assert.equal(delayed.el('canvas').resets,preserved);
    ds.binary({state_rev:'3',render_rev:'3',render_key:'3'});delayed.decoders.shift().finish();delayed.raf();cursor(delayed,false);delayed.c.stop();
    for(const lifecycle of ['hide','disconnect','logout','renderer-failed','closed','timeout','decode-failed']){
        const h=await displayed({holdDecode:true}),s=h.sockets[0];h.el('in').onclick();cursor(h,true);
        if(lifecycle==='hide'){h.doc.hidden=true;h.events.visibilitychange();}else if(lifecycle==='disconnect'){s.onclose();}
        else if(lifecycle==='logout'){await h.el('leave').onclick();}else if(lifecycle==='timeout'){h.fire(10000,false);}
        else if(lifecycle==='decode-failed'){s.binary();h.decoders.shift().fail();}
        else{s.text(h.state({status:lifecycle==='closed'?'closed':'failed',failure:lifecycle==='closed'?null:'worker_failed'}));}
        cursor(h,false);h.c.stop();
    }
    const failedPolicy=await displayed(),fp=failedPolicy.sockets[0],previous=failedPolicy.el('canvas').pixels.slice();
    failedPolicy.el('thin').value='keep';failedPolicy.el('thin').onchange();
    fp.text(failedPolicy.state({state_rev:'2',render_rev:'2',render_key:'2',thin:'keep',status:'failed',failure:'worker_failed'}));
    assert.deepEqual(failedPolicy.el('canvas').pixels,previous);assert.equal(failedPolicy.el('empty').hidden,true);
    assert.match(failedPolicy.el('frame-status').textContent,/Last image.*failed/);cursor(failedPolicy,false);failedPolicy.c.stop();
    const chain=await displayed(),cs=chain.sockets[0];chain.el('detail').value='medium';chain.el('detail').onchange();const first=cs.sent.at(-1);
    chain.el('thin').value='keep';chain.el('thin').onchange();cursor(chain,true);
    cs.text({type:'accepted',seq:first.seq,state_rev:'2'});const secondState={state_rev:'2',render_rev:'2',render_key:'2',detail:'medium'};
    cs.text(chain.state(secondState));cs.binary(secondState);chain.raf();cursor(chain,true);chain.fire(100);cursor(chain,true);
    cs.text({type:'accepted',seq:cs.sent.at(-1).seq,state_rev:'3'});const lastState={state_rev:'3',render_rev:'3',render_key:'3',detail:'medium',thin:'keep'};
    cs.text(chain.state({...lastState,status:'rendering'}));cs.binary(lastState);chain.raf();cursor(chain,true);cs.text(chain.state(lastState));cursor(chain,false);chain.c.stop();
    // Shared panels: the GTK-style menu bar and the layer pane bind through
    // the prefixed `el`, read only this session's routes, and edit over the
    // same serialized view.set path as the toolbar.
    const shell=environment({demo:true,panels:true});await shell.c.start();shell.hello();
    assert.deepEqual(shell.el('menubar').children.map(m=>m.children[0].textContent),['File','View','Cell']);
    shell.sockets[0].text(shell.state({fill_slots_key:'0'.repeat(40)}));await tick();
    const page=shell.requests.filter(r=>r.path.endsWith('/palette'));assert.equal(page.length,1);assert.equal(page[0].headers['X-Floe-CSRF'],'f'.repeat(64));
    assert.equal(shell.el('layers-count').textContent,'0 layers');assert.equal(shell.el('layers').textContent,'No layer rows.');
    assert.equal(shell.el('dstatus').textContent,'depth: full/6 · detail: high · thin:auto · frame:on');
    assert(shell.requests.every(r=>r.path.startsWith(base)&&!r.path.includes('/api/v1/views/')),'owner routes are never addressed');
    const view=shell.el('menubar').children[1];view.children[0].onclick({preventDefault(){}});assert.equal(view.children[1].hidden,false);
    const detail=view.children[1].children.find(n=>n.children[0]&&n.children[0].children[1]&&n.children[0].children[1].textContent==='Detail');
    const low=detail.children[1].children[0];assert.equal(low.children[1].textContent,'Low · 5 px');low.onclick({preventDefault(){}});
    assert.equal(shell.el('detail').value,'low');assert.deepEqual(shell.sockets[0].sent.at(-1).body,{detail:'low'});assert.equal(view.children[1].hidden,true);
    assert.equal(shell.el('minimap-panel').hidden,true,'no minimap projection in this snapshot');
    shell.el('tab-palette').onclick();assert.equal(shell.el('palette-page').hidden,false);assert.equal(shell.el('minimap-page').hidden,true);
    assert(shell.requests.some(r=>r.path.endsWith('/presets')),'presets load with the palette page');
    shell.c.stop();
    // The cell tree reads this session's cell route only; its root edit is
    // a view.set on the same serialized path; the Cell menu proxies it.
    const cellsShell=environment({demo:true,panels:true});await cellsShell.c.start();cellsShell.hello();
    assert.deepEqual(cellsShell.el('menubar').children.map(m=>m.children[0].textContent),['File','View','Cell']);
    cellsShell.sockets[0].text(cellsShell.state({dbu_um:'0.001',capabilities:{query:false,clip:false,mode:false,labels:true,cells:true,cell_root:true},root:null,root_name:''}));
    for(let i=0;i<6;i++){await tick();}
    const cellRows=()=>cellsShell.el('cells-tree').children.filter(n=>n.children.length).map(n=>n.children[1].textContent);
    assert.deepEqual(cellRows(),['TOP','BLK','VIA']);
    const asked=cellsShell.requests.filter(r=>r.path===base+'/cells').map(r=>r.body.kind);assert.deepEqual(asked,['sources','children']);
    const via=cellsShell.el('cells-tree').children.find(n=>n.children[1]&&n.children[1].textContent==='VIA');via.onclick({target:via.children[1]});
    for(let i=0;i<6;i++){await tick();}
    assert.equal(cellsShell.el('cells-root').disabled,false,'the root needs the extent of this frame');
    assert.equal(cellsShell.el('cells-build').hidden,true,'the demo offers no index build');
    cellsShell.el('cells-build').onclick();cellsShell.el('cells-build-run').onclick();
    assert.equal(cellsShell.el('cells-build-confirm').hidden,true,'the demo never confirms an index build');
    const sentBefore=cellsShell.sockets[0].sent.length;
    cellsShell.el('viewport').listeners.keydown({key:'t',code:'KeyT',ctrlKey:true,shiftKey:false,metaKey:false,altKey:false,repeat:false,isComposing:false,keyCode:84,preventDefault(){}});
    assert.equal(cellsShell.sockets[0].sent.slice(sentBefore).filter(m=>m.type==='view.set').length,0,'Ctrl+T is no shortcut');
    cellsShell.el('cells-root').onclick();
    const rootEdit=cellsShell.sockets[0].sent.slice(sentBefore).find(m=>m.type==='view.set');
    assert.deepEqual(rootEdit.body,{root:{src:0,cell:2}},'the root button roots the selected cell over view.set');
    assert(cellsShell.requests.every(r=>r.path.startsWith(base)&&!r.path.includes('/api/v1/views/')),'owner routes are never addressed');
    cellsShell.c.stop();
    const fs=require('node:fs'),path=require('node:path');
    assert.match(fs.readFileSync(path.join(__dirname,'server.html'),'utf8'),/<body id="server-shell" data-floe-viewer>/);
    assert.match(fs.readFileSync(path.join(__dirname,'viewer.css'),'utf8'),/\[data-floe-viewer\]\[data-busy="true"\][^{]*\[data-floe-viewer\]\[data-busy="true"\] \*\s*\{\s*cursor:\s*wait\s*!important;/);
    console.log('WEB SERVER DISPLAY POLICY: ALL OK (all controls retain pixels; waiting through queue/ACK/state/decode/presentation; failure/no-op/lifecycle resets)');
    console.log('WEB SERVER NAVIGATION: ALL OK (band in/out/cancel/letterbox, retained zoom/free pan, no-op/rejection, stale packet and policy/session isolation)');
    console.log('WEB SERVER SESSION: ALL OK (bootstrap, isolated storage, pixels/ACK, serialized edits, reconnect/no replay, open ambiguity, hidden exchange, revoke/logout)');
})().catch(e=>{console.error(e);process.exitCode=1;});
