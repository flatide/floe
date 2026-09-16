'use strict';
const assert=require('node:assert/strict');
const ui='./';
const P=require(ui+'protocol.js'),Q=require(ui+'query.js'),T=require('./guest-tools.js');
const html=require('node:fs').readFileSync(require('node:path').join(__dirname,'guest.html'),'utf8');
const ids=new Set([...html.matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
function setup(){
    const state={view_id:'a'.repeat(64),connection_epoch:'b'.repeat(64),dataset_revision:'1',worker_epoch:'2',state_rev:'3',render_rev:'3',render_key:'1',
        pixels:[200,160],bbox_dbu:['-100','-80','100','80'],status:'idle',capabilities:{query:true}};
    const frame={...state,frame_id:'4',width:200,height:160,purpose:'foreground',query:true,query_scene:{generation:'1',round:'1',complete:true,summary_layers:'0'}};
    let c={id:state.view_id,state,frame,acked:true,connected:true,pending:false,hidden:false,origin:[0,0],rect:{left:0,top:0},size:{pixels:[200,160],dpr:1,left:0,top:0}};
    const nodes=new Map(),sent=[],timers=new Map(),rafs=new Map();let seq='0',time=1000,n=0,t;
    const ctx={clearRect(){},save(){},restore(){},rect(){},clip(){},beginPath(){},moveTo(){},lineTo(){},closePath(){},setLineDash(){},stroke(){},fill(){},fillRect(){},fillText(){},setTransform(){},measureText(){return {width:20};}};
    const el=id=>{assert(ids.has(id),'missing real guest HTML element: '+id);if(!nodes.has(id)){const e={textContent:'',checked:false,disabled:false,hidden:false,style:{},getContext:()=>ctx,setAttribute(){}};
        Object.defineProperty(e,'innerHTML',{set(){throw Error('Never HTML');}});nodes.set(id,e);}return nodes.get(id);};
    const win={setTimeout(f,ms){timers.set(++n,{f,at:time+ms});return n;},clearTimeout(i){timers.delete(i);},requestAnimationFrame(f){rafs.set(++n,f);return n;},cancelAnimationFrame(i){rafs.delete(i);}};
    t=T.bind({window:win,document:{getElementById:el},protocol:P,query:Q,wire:require('./guest-query-wire.js'),inspect:require(ui+'inspect.js'),measure:require(ui+'measure.js'),rulers:require(ui+'rulers.js'),
        now:()=>time,context:()=>c,send:m=>{m.seq=seq=P.next(seq);sent.push(m);return seq;},modeChanged(){if(t){t.changed();}}});
    function tick(ms=100){const end=time+ms;while(true){const item=[...timers].sort((a,b)=>a[1].at-b[1].at)[0];if(!item||item[1].at>end){break;}time=item[1].at;timers.delete(item[0]);item[1].f();}time=end;}
    function queryReply(hit,request=sent.filter(m=>m.type==='explore.query').at(-1)){
        t.receive({type:'query.accepted',seq:request.seq,view_id:request.view_id,connection_epoch:request.connection_epoch,query_id:request.seq});
        t.receive({type:'query.result',seq:request.seq,view_id:request.view_id,connection_epoch:request.connection_epoch,query_id:request.seq,anchor:request.body.anchor,
            status:'ok',hit,scene:{generation:'1',round:'1',complete:true,summary_layers:'0'},requested_summary_layers:'0'});
    }
    function measureReply(point,request=sent.filter(m=>m.type==='explore.measure').at(-1)){
        const start=request.body.start_dbu;
        t.receive({type:'measure.result',seq:request.seq,view_id:request.view_id,connection_epoch:request.connection_epoch,anchor:request.body.anchor,
            point_dbu:point,snap:request.body.snap_query===null?null:'vertex',segment:start?{endpoints_dbu:[start,point],delta_um:['10','0'],distance_um:'10'}:null});
    }
    t.changed();return {t,el,sent,tick,queryReply,measureReply,timers,rafs,get c(){return c;},set c(v){c=v;}};
}
const shape={kind:'pick',count:'1',index:'0',pair:[7,0],layer_name:'<img> layer',cell_name:'private cell',area_dbu2:'100',bbox_dbu:['0','0','10','10'],
    points_dbu:[['0','0'],['10','0'],['10','10'],['0','10']],points_truncated:false};
{
    const h=setup();assert(!h.el('guest-inspect').hidden);h.c.acked=false;assert.equal(h.t.click(100,80),false);assert.equal(h.sent.length,0);
    h.c.acked=true;assert(h.t.click(100,80));h.queryReply(shape);assert.match(h.el('pick-details').textContent,/<img> layer/);
    h.tick();h.el('snap-probe').checked=true;h.t.move(100,80);const probe=h.sent.at(-1);assert.equal(probe.type,'explore.query');
    assert(h.t.key('r'));assert(h.t.active());assert(h.sent.some(m=>m.type==='explore.query.cancel'&&m.kind==='snap'));
    h.queryReply({kind:'snap',snap:'vertex',point_dbu:['0','0']},probe);assert.equal(h.el('snap-status').textContent,'','retired probe cannot become ruler snap');
    h.tick();h.t.click(100,80);assert.equal(h.sent.at(-1).body.operation.kind,'snap');h.queryReply({kind:'snap',snap:'vertex',point_dbu:['0','0']});
    assert.equal(h.sent.at(-1).type,'explore.measure');h.measureReply(['0','0']);
    const before=h.sent.length;h.el('snap-probe').onchange();assert.equal(h.sent.length,before,'probe UI cannot cancel active ruler');
    h.tick();h.t.click(110,80);h.queryReply({kind:'snap',snap:'vertex',point_dbu:['10','0']});h.measureReply(['10','0']);
    assert.match(h.el('ruler-count').textContent,/1 rulers/);assert.match(h.el('ruler-details').textContent,/10.0000/);
    assert(h.t.key('k'));assert.match(h.el('ruler-count').textContent,/0 rulers/);
    h.c=null;h.t.changed();assert(h.el('guest-inspect').hidden);assert.equal(h.el('pick-details').textContent,'');assert.equal(h.el('ruler-details').textContent,'');
    assert.equal(h.timers.size,0);assert.equal(h.rafs.size,0);assert(!h.t.active());
}
{
    const h=setup();h.t.click(100,80);h.queryReply(shape);h.tick();
    h.t.click(120,80,{shiftKey:true});h.queryReply({...shape,bbox_dbu:['20','0','30','10'],
        points_dbu:[['20','0'],['30','0'],['30','10'],['20','10']]});
    assert.equal(h.t.selection().length,2);h.t.key('r');const request=h.sent.at(-1);
    assert.equal(request.type,'explore.measure_selection');assert.deepEqual(request.body.boxes_dbu,[shape.bbox_dbu,['20','0','30','10']]);
    h.t.receive({type:'measure_selection.result',seq:request.seq,view_id:request.view_id,connection_epoch:request.connection_epoch,anchor:request.body.anchor,
        segments:[{endpoints_dbu:[['10','5'],['20','5']],delta_um:['10','0'],distance_um:'10'}]});
    assert.match(h.el('ruler-status').textContent,/1 selected bbox gaps/);assert.match(h.el('ruler-count').textContent,/1 rulers/);
    h.t.reset();assert.equal(h.timers.size,0);assert.equal(h.rafs.size,0);
}
{
    const h=setup();h.c.state.capabilities.query=false;h.c.frame.query=false;h.c.frame.query_scene={generation:null,round:null,complete:false,summary_layers:'0'};h.t.changed();
    assert(h.t.key('r'));assert(h.el('ruler-snap').disabled);assert(!h.el('ruler-snap').checked);h.t.click(20,40);
    assert.equal(h.sent.at(-1).type,'explore.measure');assert.equal(h.sent.at(-1).body.snap_query,null);h.measureReply(['-80','40']);
    h.c.pending=true;const count=h.sent.length;h.t.click(30,40);assert.equal(h.sent.length,count,'no coordinates against pending display edits');
    h.c=null;h.t.reset();assert.equal(h.timers.size,0);assert.equal(h.rafs.size,0);
}
console.log('GUEST TOOLS: ALL OK (real inspection/ruler modules, mapped native replies, snap owner, no displayed receipt, jobdeck cursor-only, teardown)');
