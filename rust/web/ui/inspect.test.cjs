'use strict';
const assert=require('node:assert/strict'), P=require('./protocol.js'), Q=require('./query.js'), I=require('./inspect.js');
function harness() {
    const state={view_id:'a'.repeat(64),connection_epoch:'b'.repeat(64),dataset_revision:'1',worker_epoch:'2',state_rev:'3',render_rev:'3',render_key:'1',
        pixels:[200,160],bbox_dbu:['-100','-80','100','80'],status:'idle',capabilities:{query:true}};
    const frame={...state,frame_id:'4',width:200,height:160,purpose:'foreground',query:true,query_scene:{generation:'1',round:'1',complete:true,summary_layers:'0'}};
    const c={id:state.view_id,state,frame,acked:true,connected:true,pending:false,hidden:false,origin:[0,0],rect:{left:10,top:20},
        size:{pixels:[200,160],dpr:2,left:0,top:0}};
    let seq='0',time=0,task=0,layerPairs=[];
    const timers=new Map(),frames=new Map(),sent=[],paths=[],nodes=new Map();
    let points=[],dash=[],closed=false;
    const ctx={clearRect(){paths.length=0;},save(){},restore(){},rect(){},clip(){},beginPath(){points=[];closed=false;},
        moveTo(x,y){points.push([x,y]);},lineTo(x,y){points.push([x,y]);},setLineDash(v){dash=v;},closePath(){closed=true;},
        stroke(){paths.push({points:points.slice(),closed,dash,width:this.lineWidth,color:this.strokeStyle});}};
    function el(id){if(!nodes.has(id)){const n={textContent:'',disabled:false,checked:false,hidden:false,style:{},width:1,height:1,getContext:()=>ctx};
        Object.defineProperty(n,'innerHTML',{set(){throw new Error('HTML injection');}});nodes.set(id,n);}return nodes.get(id);}
    const win={requestAnimationFrame:fn=>{frames.set(++task,fn);return task;},cancelAnimationFrame:id=>frames.delete(id)};
    const i=I.bind({document:{getElementById:el},window:win,protocol:P,query:Q,context:()=>c,send:m=>{m.seq=seq=P.next(seq);sent.push(JSON.parse(JSON.stringify(m)));return seq;},
        layers:p=>{layerPairs=p;},now:()=>time,setTimeout:(fn,ms)=>{timers.set(++task,{fn,at:time+ms});return task;},clearTimeout:id=>timers.delete(id)});
    function tick(ms=80){const end=time+ms;while(true){const t=[...timers].sort((a,b)=>a[1].at-b[1].at)[0];if(!t||t[1].at>end)break;time=t[1].at;timers.delete(t[0]);t[1].fn();}time=end;}
    function paint(){const p={bbox:c.state.bbox_dbu.map(Number),step:[1,1],origin:[0,0],dbu:.001};i.paint(p,c.size);for(const fn of frames.values())fn();frames.clear();}
    function reply(hit,status='ok',request=sent.filter(m=>m.type==='view.query').at(-1)){
        i.receive({type:'query.accepted',seq:request.seq,view_id:request.view_id,connection_epoch:request.connection_epoch,query_id:'9007199254740993'});
        const v={type:'query.result',seq:request.seq,view_id:request.view_id,connection_epoch:request.connection_epoch,query_id:'9007199254740993',anchor:request.body.anchor,
            status,hit,scene:{generation:'3',round:'1',complete:true,summary_layers:status==='scene_summary'?'1':'0'},requested_summary_layers:status==='scene_summary'?'1':'0'};
        i.receive(v);paint();return v;
    }
    i.changed();paint();return {i,c,el,sent,paths,timers,frames,tick,paint,reply,get pairs(){return layerPairs;},request:()=>sent.filter(m=>m.type==='view.query').at(-1)};
}
function shape(n=0,truncated=false){return {kind:'pick',count:'2',index:String(n%2),pair:[7+n,0],layer_name:'<img onerror=bad> L'+n,cell_name:'한글 <script>'+n,
    area_dbu2:String(100+n),bbox_dbu:['0','0','10','10'],points_dbu:[['0','0'],['10','0'],['10','10'],['0','10']],points_truncated:truncated};}
{
    const h=harness();assert(h.i.click(62,58));assert.deepEqual(h.request().body.position,[.52,.475]);assert.equal(h.request().body.radius_px,6);
    h.reply(shape());assert.match(h.el('pick-details').textContent,/<img onerror=bad>/);assert.match(h.el('pick-details').textContent,/100 DBU²/);
    assert.deepEqual(h.pairs,[[7,0]]);assert(h.paths.some(p=>p.closed&&p.width===2));assert.equal(h.el('query-canvas').style.width,'100px');
    h.i.move(61,60);assert.equal(h.frames.size,0,'probe-off hover repainted the selection');
    h.tick();h.i.click(62,58);assert.equal(h.request().body.operation.nth,'1');h.reply(shape(1));
    h.tick();h.el('pick-prev').onclick();assert.equal(h.request().body.operation.nth,'0');h.reply(shape());
    h.tick();h.i.click(62,58,{shiftKey:true});assert.equal(h.request().body.operation.nth,'0');h.reply(shape(1));assert.deepEqual(h.pairs,[[7,0],[8,0]]);
    h.tick();h.i.click(62,58,{ctrlKey:true});h.reply(shape());assert.deepEqual(h.pairs,[[8,0]]);
    h.tick();h.i.click(62,58,{metaKey:true});h.reply(shape());assert.deepEqual(h.pairs,[[8,0],[7,0]]);
    h.tick();h.i.click(62,58);h.reply(null,'scene_summary');assert.deepEqual(h.pairs,[[8,0],[7,0]]);assert.match(h.el('pick-status').textContent,/summaries/);
    h.tick();h.i.click(62,58);h.reply(null);assert.deepEqual(h.pairs,[]);assert.match(h.el('pick-status').textContent,/No shape/);
    h.i.stop();assert.equal(h.timers.size,0);assert.equal(h.frames.size,0);
}
{
    const h=harness();h.i.click(62,58);h.reply(shape(0,true));
    assert.match(h.el('pick-details').textContent,/truncated/);assert(h.paths.some(p=>p.closed&&p.dash.length>0));
    assert(!h.paths.some(p=>p.closed&&p.dash.length===0),'truncated prefix was closed as a polygon');
    h.tick();h.i.click(62,58);const old=h.request();h.c.pending=true;h.i.changed();h.reply(shape(1), 'ok', old);assert.deepEqual(h.pairs,[[7,0]]);
    h.c.pending=false;h.c.state.state_rev='4';h.c.state.render_rev='4';h.i.changed();assert.deepEqual(h.pairs,[[7,0]],'pan forgot accepted annotation');
    h.el('pick-next').onclick();assert.equal(h.request(),old,'stale cycle was submitted');
    h.c.state.render_key='2';h.i.changed();assert.deepEqual(h.pairs,[],'style/visibility revision retained old geometry');
    h.i.stop();assert.equal(h.timers.size,0);
}
{
    const h=harness();assert(h.i.key('m'));h.i.move(60,60);assert.equal(h.request().body.operation.kind,'snap');assert.equal(h.request().body.radius_px,20);
    h.reply({kind:'snap',snap:'vertex',point_dbu:['0','0']});assert.match(h.el('snap-status').textContent,/vertex/);
    assert(h.paths.some(p=>p.points[0][0]===90&&p.points[1][0]===110),'crosshair must be 10 CSS px at DPR2');
    h.tick();h.i.move(62,60);const prev=h.request();h.i.move(63,60);h.i.move(NaN,NaN);assert.equal(h.sent.at(-1).type,'view.query.cancel');
    h.reply({kind:'snap',snap:'edge',point_dbu:['10','0']},'ok',prev);assert.equal(h.el('snap-status').textContent,'');
    h.tick();h.i.move(63,60);h.c.hidden=true;h.i.changed();h.paint();assert.equal(h.el('query-canvas').hidden,true);
    h.c.hidden=false;h.i.changed();assert(h.i.key('m'));h.i.move(61,60);assert.equal(h.sent.at(-1).type,'view.query.cancel');
    h.i.stop();assert.equal(h.timers.size,0);
}
{
    const h=harness();for(let n=0;n<65;n++){h.tick();h.i.click(62,58,{shiftKey:true});h.reply(shape(n));}
    assert.equal(h.pairs.length,64);assert.match(h.el('pick-status').textContent,/64 selected shapes/);
    assert(h.i.key('Escape'));assert.equal(h.pairs.length,0);assert(!h.i.key('Escape'));assert(!h.i.key('r'),'manual ruler is not silently faked');
    h.tick();h.i.click(62,58);h.reply(shape());h.c.connected=false;h.i.changed();assert.deepEqual(h.pairs,[]);assert(h.el('snap-probe').disabled);
    h.c.connected=true;h.c.state.capabilities.query=false;h.i.changed();assert.match(h.el('query-availability').textContent,/Jobdeck/);assert(!h.i.click(62,58));
    h.i.stop();assert.equal(h.timers.size,0);assert.equal(h.frames.size,0);
}
console.log('WEB INSPECT: ALL OK (click/cycle/multiselect, plain text, bounded/open outlines, DPR snap, stale/style/disconnect, cancel, no invented ruler, cleanup)');
