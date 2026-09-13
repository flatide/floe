'use strict';
const assert=require('node:assert/strict'), P=require('./protocol.js'), Q=require('./query.js'), M=require('./measure.js'), R=require('./rulers.js');
function harness() {
    const state={view_id:'a'.repeat(64),connection_epoch:'b'.repeat(64),dataset_revision:'1',worker_epoch:'2',state_rev:'3',render_rev:'3',render_key:'1',
        pixels:[200,160],bbox_dbu:['-100','-80','100','80'],status:'idle',capabilities:{query:true}};
    const c={id:state.view_id,state,frame:{...state,frame_id:'4',width:200,height:160,purpose:'foreground',query:true,query_scene:{generation:'1',round:'1',complete:true,summary_layers:'0'}},
        acked:true,connected:true,pending:false,hidden:false,origin:[0,0],rect:{left:10,top:20},size:{pixels:[200,160],dpr:2,left:.25,top:.125}};
    let time=0,id=0,seq='9007199254740992',paints=0;
    const nodes=new Map(),timers=new Map(),frames=new Map(),sent=[],lines=[];
    const ctx={clearRect(){},save(){},restore(){},beginPath(){},rect(){},clip(){},setTransform(){},setLineDash(){},moveTo(){},lineTo(){},stroke(){},closePath(){},fill(){},fillRect(){},fillText(){},measureText(s){return {width:s.length*6};}};
    function el(id){if(!nodes.has(id)){const n={textContent:'',checked:false,disabled:false,hidden:false,style:{},width:1,height:1,setAttribute(k,v){this[k]=v;},getContext:()=>ctx};
        Object.defineProperty(n,'innerHTML',{set(){throw new Error('HTML injection');}});nodes.set(id,n);}return nodes.get(id);}
    const m=M.bind({document:{getElementById:el},window:{requestAnimationFrame:fn=>{frames.set(++id,fn);return id;},cancelAnimationFrame:id=>frames.delete(id)},
        protocol:P,query:Q,rulers:{paint(...args){paints++;lines.push(args[1]);R.paint(...args);}},context:()=>c,modeChanged(){},now:()=>time,
        send:v=>{v.seq=seq=P.next(seq);sent.push(JSON.parse(JSON.stringify(v)));return seq;},setTimeout:(fn,ms)=>{timers.set(++id,{fn,at:time+ms});return id;},clearTimeout:id=>timers.delete(id)});
    function tick(ms=80){const end=time+ms;while(true){const t=[...timers].sort((a,b)=>a[1].at-b[1].at)[0];if(!t||t[1].at>end)break;time=t[1].at;timers.delete(t[0]);t[1].fn();}time=end;}
    function paint(){m.paint({bbox:[-100,-80,100,80],step:[1,1],origin:[0,0]},c.size);const fs=[...frames.values()];frames.clear();fs.forEach(fn=>fn());}
    function request(type='view.measure'){return sent.filter(m=>m.type===type).at(-1);}
    function reply(p,seg=null,t=request(),extra={}){const v={type:'measure.result',seq:t.seq,view_id:t.view_id,connection_epoch:t.connection_epoch,anchor:t.body.anchor,point_dbu:p,snap:null,segment:seg,...extra};m.receive(v);paint();return v;}
    function snap(hit,status='ok',t=request('view.query')) {m.receive({type:'query.accepted',seq:t.seq,view_id:t.view_id,connection_epoch:t.connection_epoch,query_id:'9007199254741000'});
        m.receive({type:'query.result',seq:t.seq,view_id:t.view_id,connection_epoch:t.connection_epoch,query_id:'9007199254741000',anchor:t.body.anchor,status,hit,
            scene:{generation:'1',round:'1',complete:true,summary_layers:status==='scene_summary'?'1':'0'},requested_summary_layers:status==='scene_summary'?'1':'0'});}
    m.changed();paint();return {m,c,el,sent,timers,frames,lines,tick,paint,request,reply,snap,get paints(){return paints;}};
}
const segment=(a,b,d='0.005',delta=['0.003','0.004'])=>({endpoints_dbu:[a,b],distance_um:d,delta_um:delta});
{
    const h=harness();h.m.key('r');h.m.key('m');h.m.click(60,60);h.reply(['1e-100','0']);
    assert.match(h.el('ruler-details').textContent,/1e-100/);h.tick();h.m.click(61,60);assert.deepEqual(h.request().body.start_dbu,['1e-100','0']);
    h.reply(['1','0'],segment(['1e-100','0'],['1','0'],'1e100',['1e100','0']));assert.match(h.el('ruler-details').textContent,/e\+100/);h.m.stop();
}
{
    const h=harness();assert(h.m.key('r'));assert(h.el('ruler-snap').checked);assert(h.m.key('m'));
    h.m.click(60.25,60.125);assert.deepEqual(h.request().body.position,[.5,.5]);assert.equal(h.request().body.start_dbu,null);h.reply(['0','0']);
    assert.equal(h.el('ruler-canvas').hidden,false,'first point marker missing');
    h.tick();h.m.move(61.75,58.125,{shiftKey:true});h.paint();assert.equal(h.el('ruler-canvas').hidden,false,'hover erased the accepted preview');assert(h.request().body.free_angle);assert.deepEqual(h.request().body.start_dbu,['0','0']);
    h.reply(['3','4'],segment(['0','0'],['3','4']));assert.match(h.el('ruler-details').textContent,/0.0050 µm/);assert.equal(h.el('ruler-count').textContent,'0 rulers');
    h.tick();h.m.click(61.75,58.125,{shiftKey:true});const final=h.request();h.m.move(80,70);assert.equal(h.request(),final,'hover replaced a clicked endpoint');
    h.reply(['3','4'],segment(['0','0'],['3','4']));assert.equal(h.el('ruler-count').textContent,'1 rulers');assert.equal(h.el('ruler-canvas').style.width,'100px');assert(h.paints>0);
    assert(h.m.key('Escape'));assert(!h.m.active());assert(h.m.key('Escape'));assert.equal(h.el('ruler-count').textContent,'0 rulers');
    h.m.stop();assert.equal(h.timers.size,0);assert.equal(h.frames.size,0);assert.equal(h.el('ruler-canvas').width,1);
}
{
    const h=harness();h.m.key('r');h.m.click(60.25,60.125);assert.equal(h.request('view.query').body.radius_px,20);assert.equal(h.request(),undefined);
    h.snap({kind:'snap',snap:'vertex',point_dbu:['0','0']});assert.equal(h.request().body.snap_query,'9007199254741000');h.reply(['0','0'],null,undefined,{snap:'vertex'});
    h.tick();h.m.click(61,58);h.snap(null,'scene_summary');assert.match(h.el('ruler-status').textContent,/summaries/);assert.equal(h.request().body.start_dbu,null,'refusal became cursor fallback');
    h.tick();h.m.click(61,58);h.snap(null);assert.deepEqual(h.request().body.start_dbu,['0','0']);h.reply(['3','4'],segment(['0','0'],['3','0'],'0.003',['0.003','0']));
    assert.equal(h.el('ruler-count').textContent,'1 rulers');h.m.key('k');assert.equal(h.el('ruler-count').textContent,'0 rulers');h.m.stop();
}
{
    const h=harness();h.m.key('r');h.m.key('m');h.m.move(50,50);const old=h.request();
    for(let n=0;n<500;n++){h.m.move(51+n/1000,50);}assert.equal(h.sent.length,1);assert(h.timers.size<=1);h.tick();assert.equal(h.sent.length,2);
    h.reply(['99','99'],null,old);assert.equal(h.el('ruler-count').textContent,'0 rulers');h.tick(8000);assert.match(h.el('ruler-status').textContent,/timed out/);
    h.m.click(60,60);const t=h.request();h.c.pending=true;h.m.changed();h.reply(['0','0'],null,t);assert.match(h.el('ruler-status').textContent,/discarded/);
    h.c.pending=false;h.m.changed();h.tick();h.m.click(60,60);h.reply(['0','0'],null,undefined,{anchor:{}});assert.match(h.el('ruler-status').textContent,/Invalid/);
    h.m.stop();assert.equal(h.timers.size,0);assert.equal(h.frames.size,0);
}
{
    const h=harness();h.c.state.capabilities.query=false;h.c.frame.query=false;h.c.frame.query_scene={generation:null,round:null,complete:false,summary_layers:'1'};
    assert(h.m.key('r'));assert(!h.el('ruler-snap').checked);assert(h.el('ruler-snap').disabled);h.m.click(60,60);h.reply(['0','0']);
    assert(h.m.key('Escape'),'pending first point outranks exiting the mode');assert(h.m.active());assert(h.m.key('Escape'));assert(!h.m.active());
    h.m.key('r');h.m.click(60,60);h.tick();h.reply(['0','0']);h.tick();h.m.click(61,60);h.reply(['3','0'],segment(['0','0'],['3','0'],'0.003',['0.003','0']));
    h.c.state.render_key='2';h.c.frame.render_key='2';h.m.changed();assert.equal(h.el('ruler-count').textContent,'1 rulers','style change erased annotation');
    h.c.state.worker_epoch='3';h.m.changed();assert.equal(h.el('ruler-count').textContent,'0 rulers');assert(!h.m.active());h.m.stop();
}
{
    const h=harness();h.m.key('r');h.m.key('m');
    for(let n=0;n<257;n++){h.tick();h.m.click(60,60);h.reply(['0','0']);h.tick();h.m.click(61,60);h.reply(['3','0'],segment(['0','0'],['3','0'],'0.003',['0.003','0']));}
    assert.equal(h.el('ruler-count').textContent,'256 rulers');assert.match(h.el('ruler-status').textContent,/256 rulers/);h.m.key('K');assert.equal(h.el('ruler-count').textContent,'0 rulers');h.m.stop();
}
console.log('WEB MANUAL RULERS: ALL OK (two points, snap refs/refusal, Shift, preview, bounded hover, frame lifecycle, cap, Escape/undo/clear, cleanup)');
