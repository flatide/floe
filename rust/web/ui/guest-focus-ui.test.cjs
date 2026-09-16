'use strict';
const assert=require('node:assert/strict');
const {environment,tick,packet,click,measureReply,exactScene,view,epoch}=require('./guest.test.cjs');
const measurements=e=>e.requests.filter(r=>r.body&&r.body.body&&r.body.body.kind==='measurements');
const edits=w=>w.sent.filter(r=>r.type==='explore.set');
const posts=e=>e.requests.filter(r=>r.path.endsWith('/drc/panel')&&r.method==='POST');
async function settle(){for(let i=0;i<20;i++){await tick();}}
async function selected(){const e=environment('explore',undefined,'records');await e.c.start();e.hello();const w=e.sockets[0];w.text(e.state());await settle();
    e.el('gd-errors').children[0].onclick({});await settle();return {e,w};}
function accept(w,q,rev='2'){w.text({type:'accepted',seq:q.seq,view_id:view,connection_epoch:epoch,state_rev:rev,render_rev:rev});}
function snapshot(e,w,rev='2'){w.text(e.state({state_rev:rev,render_rev:rev,camera_um:['22','10','8'],bbox_dbu:['18','8','26','12']}));}
async function go(e,w){e.el('gd-go').onclick();await settle();assert.equal(edits(w).at(-1).body.navigation.kind,'goto');return edits(w).at(-1);}
function key(e,k){e.el('guest-viewport').listeners.keydown({key:k,preventDefault(){}});}
(async()=>{
    for(const stateFirst of [false,true]){
        const {e,w}=await selected(),q=await go(e,w),before=posts(e).length;
        if(stateFirst){snapshot(e,w);}else{accept(w,q);}await settle();
        assert.equal(measurements(e).length,0,'one half of receipt never creates CD');assert.equal(posts(e).length,before);
        if(stateFirst){accept(w,q);}else{snapshot(e,w);}await settle();
        assert.equal(measurements(e).length,1);assert.match(e.el('gd-cd-title').textContent,/global 1/);assert.equal(e.el('gd-cd-values').children.length,2);
        assert.equal(posts(e).at(-1).body.body.cd.target.error,'0');
        w.binary(packet({...exactScene,frame_id:'2',state_rev:'2',render_rev:'2',bbox_dbu:['18','8','26','12']}));e.raf();e.raf();
        const base=e.el('guest-canvas'),acks=w.sent.filter(v=>v.type==='frame.ack').length;
        e.el('gd-box').onclick();e.raf();click(e,10,10);e.raf();const blits=base.blits;e.mouse('mousemove',20,20);e.raf();
        assert.equal(base.blits,blits,'overlay-only changes never recompose base pixels');assert.equal(w.sent.filter(v=>v.type==='frame.ack').length,acks);
        key(e,'Escape');key(e,'Escape');e.raf();
        // The shared ruler controller paints guest CD without owner-only IDs.
        const ruler=e.el('ruler-canvas'),strokes=ruler.strokes||0;e.el('gd-markers').checked=false;e.el('gd-markers').onchange();await settle();e.raf();e.raf();assert.equal(ruler.strokes||0,strokes);
        e.el('gd-markers').checked=true;e.el('gd-markers').onchange();await settle();e.raf();e.raf();assert((ruler.strokes||0)>strokes);
        // Real manual ruler after CD is popped before the older CD group.
        e.advance();e.el('ruler-mode').onclick();e.el('ruler-snap').checked=false;e.el('ruler-snap').onchange();click(e,16,16);let m=w.sent.at(-1);assert.equal(m.type,'explore.measure');measureReply(w,m,['20','10']);
        e.advance();click(e,32,16);m=w.sent.at(-1);measureReply(w,m,['22','10']);e.el('ruler-mode').onclick();
        key(e,'k');await settle();assert.equal(e.el('gd-cd-values').children.length,2,'newer manual ruler is deleted first');
        key(e,'k');await settle();assert.equal(e.el('gd-cd-values').children.length,1);key(e,'K');await settle();assert.equal(e.el('gd-cd-values').children.length,0);
        key(e,'Escape');await settle();assert.equal(posts(e).at(-1).body.body.jump_active,false);w.onclose();assert.equal(e.rafs.size,0);assert(![...e.timers.values()].some(t=>t.ms===8000));
    }
    for(const reason of ['reject','timeout','cancel','new-input','skipped','disconnect']){
        const {e,w}=await selected(),q=await go(e,w);
        if(reason==='reject'){w.text({type:'error',seq:q.seq,code:'stale_state'});}
        if(reason==='timeout'){e.timer(8000);}
        if(reason==='cancel'){e.el('gd-end-jump').onclick();}
        if(reason==='new-input'){e.el('guest-in').onclick();}
        if(reason==='skipped'){snapshot(e,w,'3');}
        if(reason==='disconnect'){w.onclose();}else if(reason!=='reject'){accept(w,q);if(reason!=='skipped'){snapshot(e,w);}}
        await settle();assert.equal(measurements(e).length,0,reason);assert.equal(edits(w).filter(v=>v.body.navigation.kind==='goto').length,1,'never replay '+reason);
        assert(![...e.timers.values()].some(t=>t.ms===8000));if(reason!=='disconnect'){w.onclose();}assert.equal(e.rafs.size,0);
    }
    // A rate-limited move that was never sent is removed by explicit cancel.
    const {e,w}=await selected();e.el('guest-in').onclick();accept(w,edits(w).at(-1));snapshot(e,w);await settle();
    const count=edits(w).length;e.el('gd-go').onclick();await settle();assert.equal(edits(w).length,count);
    e.el('gd-end-jump').onclick();await settle();e.advance(65);await settle();assert.equal(edits(w).length,count);assert.equal(measurements(e).length,0);w.onclose();
    console.log('GUEST FOCUS UI: ALL OK (real WS ACK/state order, CD/history/marker overlay, cancellation/timeout/reject/supersede, unsent queue removal, no replay, cleanup)');
})().catch(e=>{console.error(e);process.exitCode=1;});
