'use strict';
const assert=require('node:assert/strict'),P=require('./protocol.js'),Client=require('./server.js'),Decode=require('./image-decode.js');
const packet=require('./guest.test.cjs').packet;
const id='a'.repeat(64),epoch='c'.repeat(64),secret='d'.repeat(64),bundle='test-bundle';
const auth={launch_id:id,csrf:'f'.repeat(64),bundle,protocol:'floe-server-v1',viewer_ready:true,render_transport:true};
const origin='https://service.example.test',base='/api/v1/server/sessions/'+id,key='floe-server-session:'+origin+':'+id;
const tick=()=>new Promise(r=>setImmediate(r));
function environment(options={}) {
    const id=options.id||'a'.repeat(64),auth={launch_id:id,csrf:'f'.repeat(64),bundle,protocol:'floe-server-v1',viewer_ready:true,render_transport:true};
    const base='/api/v1/server/sessions/'+id,key='floe-server-session:'+origin+':'+id;
    const nodes=new Map(),events={},requests=[],sockets=[],timers=new Map(),rafs=new Map(),reads=[],deferred=[],holds=[];
    const storage=new Map([['floe-session:'+origin,'OWNER'],['floe-server-session:'+origin+':'+'b'.repeat(64),'OTHER']]);
    if(options.resume){storage.set(key,JSON.stringify(auth));}
    let number=0,clock=10000,opened=!!options.resume,code=200,confirm=true,doc;
    class Element {
        constructor(){this.value='';this.checked=false;this.hidden=false;this.disabled=false;this.style={};this.listeners={};this.width=this.height=1;}
        getContext(){const self=this;return {fillRect(){self.pixels=new Uint8ClampedArray(self.width*self.height*4);},putImageData(i){self.pixels=i.data.slice();},
            drawImage(i,x=0,y=0){if(x===0&&y===0){self.pixels=i.pixels.slice();}self.blits=(self.blits||0)+1;}};}
        getBoundingClientRect(){return {width:options.width||64,height:options.height||32,left:0,top:0};}focus(){doc.activeElement=this;}
        addEventListener(k,f){this.listeners[k]=f;}
    }
    const el=k=>{if(!nodes.has(k)){nodes.set(k,new Element());}return nodes.get(k);};
    function listen(k,f){const old=events[k];events[k]=old?e=>{old(e);f(e);}:f;}
    doc={hidden:false,activeElement:null,querySelector:()=>({content:bundle}),getElementById:el,createElement:()=>new Element(),addEventListener:listen};
    function timer(f,ms,interval=false){timers.set(++number,{f,ms,interval});return number;}
    const win={devicePixelRatio:1,performance:{now:()=>clock},sessionStorage:{getItem(k){reads.push(k);return storage.get(k)||null;},setItem(k,v){if(options.noStorage){throw Error('denied');}storage.set(k,v);},removeItem(k){storage.delete(k);}},
        setTimeout:timer,clearTimeout:n=>timers.delete(n),setInterval:(f,ms)=>timer(f,ms,true),clearInterval:n=>timers.delete(n),
        requestAnimationFrame:f=>{rafs.set(++number,f);return number;},cancelAnimationFrame:n=>rafs.delete(n),addEventListener:listen,confirm:()=>confirm};
    const location={origin,protocol:'https:',pathname:'/server/'+id,hash:options.resume?'':options.hash===undefined?'#bootstrap='+secret:options.hash};
    const history={replaceState(a,b,path){assert.equal(path,location.pathname);location.hash='';}};
    function state(extra={}){return {type:'snapshot',view_id:id,connection_epoch:epoch,state_rev:'1',render_rev:'1',render_key:'1',dataset_revision:'1',worker_epoch:'1',
        bbox_dbu:['0','0','64','32'],pixels:[64,32],depth:'full',detail:'high',thin:'auto',labels:true,frames:true,mono:false,status:'idle',failure:null,
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
                else {throw Error('Unexpected path');}}
            entry.reply=(s=status,v=data)=>{this.status=s;this.responseText=v?JSON.stringify(v):'';this.onload();};entry.fail=()=>this.ontimeout();
            const h=holds.findIndex(h=>h.method===this.method&&this.path.endsWith(h.suffix));if(h>=0){holds.splice(h,1);deferred.push(entry);}else{entry.reply();}
        }
    }
    class WS {
        constructor(url,protocols){assert.equal(url,'wss://service.example.test'+base+'/stream');assert.deepEqual(protocols,['floe-server-v1','bundle.'+bundle,'csrf.'+auth.csrf]);this.sent=[];this.readyState=1;this.bufferedAmount=0;sockets.push(this);}
        send(text){this.sent.push(JSON.parse(text));}close(){this.readyState=3;}text(v){this.onmessage({data:JSON.stringify(v)});}binary(extra={}){this.onmessage({data:packet({view_id:id,...extra})});}
    }
    const c=Client.bind({window:win,document:doc,location,history,protocol:P,XHR,WebSocket:WS,now:()=>clock,gestures:require('./gestures.js'),
        decode(h,d,done){return Decode.create({ImageData:class{constructor(data){this.data=data;}},setTimeout:win.setTimeout,clearTimeout:win.clearTimeout},h,d,done);}});
    function hello(ws=sockets.at(-1),e=epoch){ws.onopen();ws.text({type:'hello',protocol:1,bundle,view_id:id,connection_epoch:e,frame_credit:1,capabilities:{view:true,index:false,review:false,export:false,query:false}});ws.text(state({connection_epoch:e}));}
    function fire(ms,interval){const item=[...timers].find(([,v])=>v.ms===ms&&(interval===undefined||v.interval===interval));assert(item,'timer '+ms);if(!item[1].interval){timers.delete(item[0]);}clock+=ms;item[1].f();}
    return {c,el:n=>el('server-'+n),events,doc,win,location,requests,sockets,storage,reads,deferred,timers,hello,state,
        raf(){for(const [k,f] of [...rafs]){rafs.delete(k);f();}},fire,code(n){code=n;},confirm(v){confirm=v;},opened(v){opened=v;},
        defer(method,suffix){holds.push({method,suffix});}};
}
module.exports={environment};
if(require.main===module)(async()=>{
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
    const nav=environment();await nav.c.start();nav.hello();nav.el('x').value='1.25';nav.el('y').value='-4';nav.el('width').value='20';nav.el('go').onclick();
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
    console.log('WEB SERVER SESSION: ALL OK (bootstrap, isolated storage, pixels/ACK, serialized edits, reconnect/no replay, open ambiguity, hidden exchange, revoke/logout)');
})().catch(e=>{console.error(e);process.exitCode=1;});
