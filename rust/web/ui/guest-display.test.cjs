'use strict';
const assert=require('node:assert/strict');
const P=require('./protocol.js');
const Q=require('./query.js');
const D=require('./guest-display.js');
function fixture(){
    const state={view_id:'a'.repeat(64),connection_epoch:'b'.repeat(64),dataset_revision:'1',worker_epoch:'2',state_rev:'3',render_rev:'3',render_key:'1',
        pixels:[200,160],bbox_dbu:['-100','-80','100','80'],rendering:false,failure:null};
    const foreground={...state,frame_id:'4',width:200,height:160,purpose:'foreground',query:true,
        query_scene:{generation:'1',round:'1',complete:true,summary_layers:'0'}};
    return {protocol:P,session:{mode:'explore'},hello:{view_id:state.view_id,connection_epoch:state.connection_epoch,mode:'explore',measure:true,query:true},state,foreground,
        margin:null,acked:{foreground:null,margin:null},connected:true,hidden:false,pending:false,
        canvasRect:{width:100,height:100,left:10,top:20},viewportRect:{width:100,height:100,left:10,top:20}};
}
{
    const o=fixture(),c=D.context(o);assert.equal(Q.scope(c,P),null,'before ACK');
    o.acked.foreground='4';const d=D.context(o);assert(Q.scope(d,P));assert.deepEqual(d.size,{pixels:[200,160],dpr:2,left:0,top:10});
    assert.deepEqual(Q.position(d,60,70),[.5,.5]);assert.equal(Q.position(d,60,25),null,'letterbox is not geometry');
    o.canvasRect={width:300,height:160,left:25,top:30};const e=D.context(o);
    assert.deepEqual(e.size,{pixels:[200,160],dpr:1,left:65,top:10});assert.deepEqual(Q.position(e,175,110),[.5,.5]);
    o.pending=true;assert.equal(Q.scope(D.context(o),P),null);o.pending=false;
    o.hidden=true;assert.equal(Q.scope(D.context(o),P,true),null);o.hidden=false;
    o.connected=false;assert.equal(Q.scope(D.context(o),P,true),null);
}
{
    const o=fixture();o.acked.foreground='4';o.state={...o.state,state_rev:'4',bbox_dbu:['-84','-80','116','80']};
    assert.equal(D.context(o).frame,null,'old foreground overlap cannot query strip');
    o.margin={...o.foreground,purpose:'margin',frame_id:'5',final:true,width:264,height:224,bbox_dbu:['-132','-112','132','112']};
    o.state.margin={frame_id:'5',origin_px:[48,32],crop_safe:false};
    let c=D.context(o);assert.equal(c.frame.frame_id,'5');assert.deepEqual(c.origin,[48,32]);assert.equal(Q.scope(c,P),null);
    o.acked.margin='5';c=D.context(o);assert(Q.scope(c,P));assert.equal(Q.scope(c,P).anchor.frame_id,'5');assert.equal(Q.scope(c,P).anchor.state_rev,'4');
    o.state.bbox_dbu=['-52','-80','148','80'];assert.equal(D.context(o).frame,null,'partial-overlap margin cannot query missing strip');
    o.state.bbox_dbu=['-83','-80','117','80'];assert.equal(D.context(o).frame,null,'non-phase pan needs new receipt');
}
{
    const o=fixture();o.acked.foreground='4';o.hello.query=false;assert.equal(Q.scope(D.context(o),P),null);assert(Q.scope(D.context(o),P,true),'manual deck ruler');
    o.session.mode=o.hello.mode='follow';assert.equal(D.context(o),null);assert.equal(Q.scope(D.context(o),P,true),null);
    o.session.mode=o.hello.mode='explore';o.hello.measure=false;assert.equal(D.context(o),null);
    o.hello.measure=true;o.hello.query='true';assert.equal(D.context(o),null);
    o.hello.query=true;o.hello.connection_epoch='c'.repeat(64);assert.equal(D.context(o),null);
}
{
    const o=fixture();o.acked.foreground='4';o.foreground.query=false;o.foreground.query_scene={generation:'1',round:'1',complete:false,summary_layers:'0'};
    assert.equal(Q.scope(D.context(o),P),null);assert(Q.scope(D.context(o),P,true),'cursor arithmetic does not promise complete geometry');
    o.state.failure='worker_failed';assert.equal(Q.scope(D.context(o),P,true),null);
    assert.throws(()=>D.size([200,160],{width:0,height:100,left:0,top:0},{left:0,top:0}));
}
console.log('GUEST DISPLAY SCOPE: ALL OK (ACK, phase/full margin, partial/stale/Follow, letterbox and DPR)');
