'use strict';
const assert=require('node:assert/strict'),zlib=require('node:zlib');
const P=require('./protocol.js');
const {environment,tick,packet,click,exactScene,view,epoch,id}=require('./guest.test.cjs');
const red=[255,0,0,255],blue=[0,0,255,255];
const edits=w=>w.sent.filter(v=>v.type==='explore.set');
const acks=w=>w.sent.filter(v=>v.type==='frame.ack');
const pixel=e=>[...e.el('guest-canvas').pixels.slice(0,4)];
function failed(e,w,extra={}){w.text(e.state({failure:'worker_failed',...extra}));}
function assertFailed(e,displayed){
    assert.match(e.el('guest-status').textContent,/Render failed: worker_failed/);
    assert.match(e.el('guest-status').textContent,/new Explore invitation/);
    assert.match(e.el('guest-frame-status').textContent,displayed?/Last image.*failed/:/No displayed frame.*failed/);
    for(const name of ['fit','in','out','depth','detail','thin','frames','labels','mono','x','y','width','go']){
        assert(e.el('guest-'+name).disabled,name+' disabled after failure');
    }
    assert(!e.el('guest-leave').disabled);assert(e.el('gl-all').disabled);assert(e.el('gl-none').disabled);
    assert.equal(e.el('guest-empty').hidden,displayed);
}
function credentialsUnchanged(e){
    assert(e.storage.has('floe-guest-session:http://127.0.0.1:1234:'+id));
    assert.equal(e.storage.get('floe-session:http://127.0.0.1:1234'),'OWNER');
    assert.equal(e.storage.get('floe-default-pending:OWNER'),'PRIVATE');
    assert.equal(e.requests.filter(v=>v.path.endsWith('/exchange')).length,1);
    assert(!e.requests.some(v=>v.method==='DELETE'));
}
async function ready(options={},grant=false){const e=environment('explore',undefined,grant,options);await e.c.start();e.hello();const w=e.sockets[0];w.text(e.state());await tick();return {e,w};}
function accept(w,q,rev='1'){w.text({type:'accepted',seq:q.seq,view_id:view,connection_epoch:epoch,state_rev:rev,render_rev:rev});}
// Real PNG container/deflate; only the browser's Image onload scheduling is fake.
function pngPacket(extra={}){
    function chunk(name,data){const kind=Buffer.from(name),body=Buffer.concat([kind,data]);let crc=0xffffffff;
        for(const b of body){crc^=b;for(let k=0;k<8;k++){crc=(crc>>>1)^((crc&1)?0xedb88320:0);}}
        const size=Buffer.alloc(4),tail=Buffer.alloc(4);size.writeUInt32BE(data.length);tail.writeUInt32BE((crc^0xffffffff)>>>0);return Buffer.concat([size,body,tail]);}
    const h={...P.packet(packet()).header,...extra,format:'png'},ihdr=Buffer.alloc(13);ihdr.writeUInt32BE(h.width);ihdr.writeUInt32BE(h.height,4);ihdr[8]=8;ihdr[9]=6;
    const scan=Buffer.alloc((h.width*4+1)*h.height);
    for(let y=0;y<h.height;y++)for(let x=0;x<h.width;x++){scan.set(blue,y*(h.width*4+1)+1+x*4);}
    const bytes=Buffer.concat([Buffer.from([137,80,78,71,13,10,26,10]),chunk('IHDR',ihdr),chunk('IDAT',zlib.deflateSync(scan)),chunk('IEND',Buffer.alloc(0))]);
    h.payload_length=String(bytes.length);const text=Buffer.from(JSON.stringify(h)),out=new Uint8Array(4+text.length+bytes.length);new DataView(out.buffer).setUint32(0,text.length,true);out.set(text,4);out.set(bytes,4+text.length);return out.buffer;
}
(async()=>{
    for(const existing of ['none','foreground','margin']){
        const {e,w}=await ready();
        if(existing==='foreground'){w.binary(packet({},red));e.raf();}
        if(existing==='margin'){w.text(e.state({margin:{frame_id:'1',origin_px:[16,16],crop_safe:true}}));
            w.binary(packet({purpose:'margin',width:96,height:64,bbox_dbu:['-16','-16','80','48']},red));e.raf();}
        const before=existing==='none'?null:pixel(e);
        // Failure has no revision bump. A decoded image awaiting RAF is stale
        // even though its entire identity/geometry stamp still matches.
        w.binary(packet({frame_id:'2'},blue));failed(e,w);e.raf();
        assert.equal(acks(w).at(-1).disposition,'discarded',existing+' pending RAF');
        assertFailed(e,existing!=='none');if(before){assert.deepEqual(pixel(e),before);}
        const count=acks(w).length;
        w.binary(packet({frame_id:'3'},blue));e.raf();
        assert.equal(acks(w).length,count+1);assert.equal(acks(w).at(-1).disposition,'discarded');
        assertFailed(e,existing!=='none');if(before){assert.deepEqual(pixel(e),before);}credentialsUnchanged(e);
        await e.el('guest-leave').onclick();assert.equal(e.el('guest-canvas').width,1);assert.equal(e.sockets.length,1);assert.equal(e.timers.size,0);assert.equal(e.rafs.size,0);
    }
    for(const event of ['onload','onerror']){
        const images=[],urls=new Set(),revoked=[];
        const imageEnv={Image:class{constructor(){this.naturalWidth=this.width=64;this.naturalHeight=this.height=32;this.pixels=new Uint8ClampedArray(64*32*4);for(let i=0;i<this.pixels.length;i+=4){this.pixels.set(blue,i);}images.push(this);}},
            Blob:class{},URL:{createObjectURL(){const u='blob:synthetic-'+images.length;urls.add(u);return u;},revokeObjectURL(u){assert(urls.delete(u));revoked.push(u);}}};
        const {e,w}=await ready({imageEnv});w.binary(packet({},red));e.raf();w.binary(pngPacket({frame_id:'2'}));
        assert.equal(images.length,1);assert.equal(urls.size,1);const late=images[0][event];assert.equal(typeof late,'function');
        failed(e,w);assert.equal(urls.size,0,'failure releases pending PNG immediately');assert.equal(revoked.length,1);
        assert.equal(images[0].src,'');assert.equal(images[0].onload,null);assert.equal(images[0].onerror,null);
        assert(![...e.timers.values()].some(t=>t.ms===5000));
        assert.equal(acks(w).filter(v=>v.frame_id==='2').length,1);assert.equal(acks(w).at(-1).disposition,'discarded');
        late();e.raf();e.advance(6000);assert.deepEqual(pixel(e),red);assertFailed(e,true);
        assert.equal(acks(w).filter(v=>v.frame_id==='2').length,1,'late callback cannot ACK twice');credentialsUnchanged(e);e.c.stop();
    }
    for(const reply of ['accepted','error','accepted-later-revision']){
        const {e,w}=await ready();w.binary(packet(exactScene,red));e.raf();e.el('guest-in').onclick();const q=edits(w).at(-1);
        e.el('guest-out').onclick();assert.equal(edits(w).length,1);failed(e,w);
        if(reply==='error'){w.text({type:'error',seq:q.seq,code:'worker_failed'});}else{accept(w,q,reply==='accepted'?'1':'2');}
        if(reply==='accepted-later-revision'){failed(e,w,{state_rev:'2',render_rev:'2'});}
        e.advance(1000);assert.equal(w.readyState,1,'late edit reply is consumed, not a protocol error');assert.equal(edits(w).length,1,'queued edit is not replayed');
        e.el('guest-in').onclick();e.el('gl-none').onclick();e.el('guest-viewport').listeners.keydown({key:'ArrowRight',preventDefault(){}});
        e.el('guest-viewport').rect={left:0,top:0,width:128,height:64};e.events.resize();e.advance(1000);
        assert.equal(edits(w).length,1);assertFailed(e,true);credentialsUnchanged(e);e.c.stop();
    }
    // An unsent cadence timer must be cancelled as well as the visible buttons.
    {const {e,w}=await ready();e.el('guest-in').onclick();accept(w,edits(w).at(-1));e.el('guest-out').onclick();
        assert([...e.timers.values()].some(t=>t.ms===65));failed(e,w);assert(![...e.timers.values()].some(t=>t.ms===65));e.advance(100);assert.equal(edits(w).length,1);e.c.stop();}
    for(const button of [0,2]){
        const {e,w}=await ready();w.binary(packet(exactScene,red));e.raf();e.mouse('mousedown',8,8,button);e.mouse('mousemove',28,20,button);e.raf();failed(e,w);e.raf();
        if(button===2){assert(e.el('zoom-band').hidden);}assert.notEqual(e.el('guest-viewport').style.cursor,'grabbing');
        e.mouse('mouseup',28,20,button);assert.equal(edits(w).length,0);assert.deepEqual(pixel(e),red);assertFailed(e,true);e.c.stop();
    }
    // A DRC jump receipt must not turn a failed navigation into a successful CD
    // operation. Already granted read-only DRC listing remains available.
    {const {e,w}=await ready({},'records');for(let i=0;i<12;i++){await tick();}e.el('gd-errors').children[0].onclick({});for(let i=0;i<12;i++){await tick();}
        e.el('gd-go').onclick();for(let i=0;i<12;i++){await tick();}const q=edits(w).at(-1);assert(q);
        failed(e,w);accept(w,q);for(let i=0;i<12;i++){await tick();}
        assert(!e.el('gd-panel').hidden);assert.equal(e.requests.filter(v=>v.body&&v.body.body&&v.body.body.kind==='measurements').length,0);
        assert(![...e.timers.values()].some(t=>t.ms===8000));credentialsUnchanged(e);e.c.stop();}
    // Failure while the DRC focus *read* is pending must not submit navigation
    // or persist a successful jump after that delayed read returns.
    {const {e,w}=await ready({},'records');for(let i=0;i<12;i++){await tick();}e.el('gd-errors').children[0].onclick({});for(let i=0;i<12;i++){await tick();}
        e.defer('POST','/drc/read');e.el('gd-go').onclick();await tick();assert.equal(e.delayed.length,1);
        assert.equal(e.delayed[0].body.body.kind,'focus');const posts=e.requests.filter(v=>v.method==='POST'&&v.path.endsWith('/drc/panel')).length;
        failed(e,w);e.delayed[0].reply();for(let i=0;i<12;i++){await tick();}
        assert.equal(edits(w).length,0);assert.equal(e.requests.filter(v=>v.method==='POST'&&v.path.endsWith('/drc/panel')).length,posts);
        assert(!e.el('gd-panel').hidden);assertFailed(e,false);credentialsUnchanged(e);e.c.stop();}
    {const {e,w}=await ready();w.binary(packet(exactScene,red));e.raf();click(e,16,8);const q=w.sent.at(-1);assert.equal(q.type,'explore.query');
        failed(e,w);w.text({type:'query.accepted',seq:q.seq,view_id:view,connection_epoch:epoch,query_id:q.seq});
        w.text({type:'query.result',seq:q.seq,view_id:view,connection_epoch:epoch,query_id:q.seq,anchor:q.body.anchor,status:'ok',
            hit:{kind:'pick',count:'1',index:'0',pair:[7,0],layer_name:'late geometry',cell_name:'synthetic',area_dbu2:'100',bbox_dbu:['0','0','10','10'],
                points_dbu:[['0','0'],['10','0'],['10','10'],['0','10']],points_truncated:false},scene:exactScene.query_scene,requested_summary_layers:'0'});
        e.raf();assert.equal(w.readyState,1);assert.equal(e.el('pick-details').textContent,'');assert(e.el('query-canvas').hidden);
        const requests=w.sent.length;click(e,16,8);assert.equal(w.sent.length,requests);assertFailed(e,true);credentialsUnchanged(e);e.c.stop();}
    // Hiding/transport loss may reconnect this same failed controller, not
    // create a new worker. Old connection callbacks have no display/ACK rights.
    for(const transition of ['hidden','disconnected']){
        const {e,w}=await ready();w.binary(packet({},red));e.raf();w.binary(packet({frame_id:'2'},blue));const oldMessage=w.onmessage;
        failed(e,w);const oldAcks=acks(w).length;
        if(transition==='hidden'){e.doc.hidden=true;e.events.visibilitychange();e.doc.hidden=false;e.events.visibilitychange();}
        else{w.onclose();e.timer(500);}await tick();assert.equal(e.sockets.length,2);
        const next=e.sockets[1],connection='9'.repeat(64);e.hello(next,connection);failed(e,next,{connection_epoch:connection});
        oldMessage({data:packet({frame_id:'2'},blue)});e.raf();e.advance(1000);
        assert.equal(acks(w).length,oldAcks);assert.equal(next.sent.length,0);assert.equal(e.el('guest-canvas').width,64);
        assertFailed(e,false);credentialsUnchanged(e);assert.equal(edits(next).length,0);e.c.stop();
    }
    // Follow's allowlisted state has no failure field; ordinary owner updates
    // must keep working without a new shared diagnostic/permission contract.
    {const e=environment('follow');await e.c.start();e.hello();const w=e.sockets[0],s=e.state();delete s.failure;w.text(s);
        w.binary(packet({},red));e.raf();w.binary(packet({frame_id:'2'},blue));e.raf();assert.deepEqual(pixel(e),blue);
        assert.equal(acks(w).at(-1).disposition,'displayed');assert.match(e.el('guest-frame-status').textContent,/Complete/);assert.equal(edits(w).length,0);e.c.stop();}
    console.log('GUEST FAILURE: ALL OK (retained/empty/margin, raw/PNG late discard, one ACK, URL cleanup, no edit/resize replay, gestures/DRC receipt and delayed focus/query, reconnect isolation, explicit leave, Follow unchanged)');
})().catch(e=>{console.error(e);process.exitCode=1;});
