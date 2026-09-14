'use strict';
// UI effects are driven by explicit HTTP completion and view ACKs. Matching and
// original-visibility storage are separately tested through the native service.
const assert=require('node:assert/strict'),D=require('./drc.js'),P=require('./protocol.js'),F=require('./test-focus.cjs');
const nodes=new Map(),requests=[],moves=[],saves=[],chosen=new Map();
let serial=0,selectionRev='1',saved=null,hold=false,held,corrupt=null,reason='ready',restoreCount=0;
let context={id:'view',source:'source',connected:true,pending:false,state:{state_rev:'1',connection_epoch:'epoch1',status:'idle',
    layers_isolated:false,dbu_um:'1',bbox_dbu:['0','0','20','20'],pixels:[200,200]}};
class Element {
    constructor(){this.children=[];this.style={};this.value='';this.checked=false;this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}
    appendChild(v){this.children.push(v);return v;}setAttribute(k,v){this[k]=v;}focus(){}scrollIntoView(){}
    getContext(){return new Proxy({}, {get:()=>()=>{}});}
}
const el=id=>{if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
el('drc-waived').value='all';el('drc-markers').checked=true;
const row=i=>({check:'0',local:String(i),global:String(i+1),kind:'p',status:0,bbox_um:['1','1','4','3'],points:'4'});
function groups(){return {revision:'r',view_id:context.id,state:{selection_rev:selectionRev,total:String([...chosen.values()].reduce((n,x)=>n+x.length,0)),limit:5000,
    rules:[...chosen].map(([check,errors])=>({check,errors}))}};}
function http(method,path,envelope,missing,token){
    const q=envelope&&envelope.body;requests.push({path,q,envelope,token});
    if(path.endsWith('/selection')){
        if(method==='POST'){assert.equal(envelope.base_selection_rev,selectionRev);selectionRev=P.next(selectionRev);
            if(q.kind==='clear_all')chosen.clear();else if(q.kind==='clear_rule'||!q.errors.length)chosen.delete(q.check);else chosen.set(q.check,q.errors);}
        return Promise.resolve(groups());
    }
    if(!q)return Promise.resolve({drc:{id:'drc',revision:'r',source_id:'source',phase:'ready',title:'isolation',metadata:{checks:'1',errors:'2'}}});
    if(q.kind==='rules')return Promise.resolve({rows:[{check:'0',name:'MASK',errors:'2',waived:'0'}],next:null});
    if(q.kind==='rule')return Promise.resolve({name:'MASK',errors:'2',waived:'0',description:'mask'});
    if(q.kind==='list')return Promise.resolve({rows:[row(0),row(1)],next:null,scanned:'2',bbox_um:q.in_view?context.state.bbox_dbu:null,selection_rev:q.selection_rev});
    if(q.kind==='filtered_step')return Promise.resolve({hit:row(q.after==='0'?1:0),next:null,scanned:'1',bbox_um:null,selection_rev:q.selection_rev});
    if(q.kind==='comparison')return Promise.resolve({...row(Number(q.error)),comparison:null});
    if(q.kind==='geometry')return Promise.resolve({...row(Number(q.error)),precision:'1',points_dbu:[['1','1'],['4','1'],['4','3'],['1','3']].slice(0,q.limit),start:'0',total:'4',next:q.limit<4?String(q.limit):null});
    if(q.kind==='measurements')return Promise.resolve({...row(Number(q.error)),segments:[{endpoints_um:[['1','1'],['4','1']],distance_um:'3',offset:false}]});
    if(q.kind==='records')return Promise.resolve({rows:q.errors.map(i=>row(Number(i)))});
    if(q.kind==='focus'){
        assert.equal(q.isolate,true);assert(!('layers' in q));
        let v=F.reply(q,{navigation:{kind:'goto',center_um:['2.5','2'],width_um:'10'},layer_isolation:{status:reason,matched:reason==='ready'?'5000':'0'}});
        if(corrupt)v=corrupt(v);
        if(hold)return new Promise(resolve=>{held={token,resolve:()=>resolve(v)};token.abort=()=>{};});
        return Promise.resolve(v);
    }
    throw new Error('Unexpected request '+JSON.stringify(q));
}
function pending(kind,done){
    const change={kind,done,sent:false,cancelled:false};moves.push(change);context={...context,pending:true};
    return ()=>{if(change.sent||change.cancelled)return false;change.cancelled=true;context={...context,pending:false};done('Request cancelled');return true;};
}
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},window:{requestAnimationFrame:()=>++serial,cancelAnimationFrame(){}},
    protocol:P,rulers:require('./rulers.js'),groups:require('./drc-groups.js'),http,context:()=>context,
    navigate:(n,token,done)=>{assert.equal(token,'a'.repeat(64));return pending('focus',done);},
    restoreLayers:done=>{restoreCount++;return pending('restore',done);},resize(){},
    stateStore:{bind:o=>{let ready=false;return {attach:async()=>{ready=false;await o.apply(saved);ready=true;},change:v=>{if(ready){saved=JSON.parse(JSON.stringify(v));saves.push(saved);}},close(){ready=false;}};}}});
async function tick(){for(let i=0;i<90;i++)await Promise.resolve();}
const count=kind=>requests.filter(r=>r.q&&r.q.kind===kind).length;
async function ack(change=moves.at(-1),error=null){
    change.sent=true;context={...context,pending:false,state:{...context.state,
        state_rev:error?context.state.state_rev:P.next(context.state.state_rev),
        layers_isolated:error?context.state.layers_isolated:change.kind==='restore'?false:reason==='ready'?true:context.state.layers_isolated}};
    change.done(error);panel.contextChanged();await tick();
}
async function jump(i=0){el('drc-errors').children[i].ondblclick();await tick();}
(async()=>{
    await panel.init();await tick();assert(el('drc-restore-layers').disabled);
    el('drc-in-view').checked=true;el('drc-in-view').onchange();await tick();
    await jump();assert.equal(moves.length,1);assert.equal(count('measurements'),0);assert(el('drc-in-view').checked);
    assert(!context.state.layers_isolated);assert.equal(saved.cd,null);
    await ack();assert(!el('drc-in-view').checked);assert.equal(count('measurements'),1);assert(!el('drc-restore-layers').disabled);
    assert.match(el('drc-layer-status').textContent,/Layers isolated/);assert.match(el('drc-layer-status').textContent,/5000 layer pairs/);
    assert.equal(saved.selected.error,'0');assert(saved.jump_active);assert.equal(saved.cd.target.error,'0');
    assert(!('layers' in saved)&&!('isolated_from' in saved),'browser stored the server visibility backup');
    el('drc-errors').children[0].onclick({shiftKey:true});await tick();assert.equal(chosen.size,1);
    assert(panel.key('Escape'));await tick();assert.equal(restoreCount,0);assert.equal(saved.cd.remaining,0);
    assert(panel.key('Escape'));await tick();assert.equal(chosen.size,0);assert.equal(restoreCount,0);
    assert(panel.key('Escape'));await tick();assert.equal(restoreCount,1);assert(saved.focus_visible,'unapproved restore cleared focus');
    assert(el('drc-restore-layers').disabled);await ack();assert(!saved.focus_visible&&!saved.jump_active);assert.equal(saved.selected.error,'0');
    assert.equal(saved.cd,null);assert(el('drc-restore-layers').disabled);
    const moved=moves.length;panel.key('.');await tick();assert.equal(saved.selected.error,'1');assert.equal(moves.length,moved,'comma/period re-entered jump mode after restore');
    await jump(1);await ack();const cd=saved.cd;
    el('drc-restore-layers').onclick();await tick();assert.deepEqual(saved.cd,cd);
    await ack(moves.at(-1),'stale_state');assert(context.state.layers_isolated);assert.deepEqual(saved.cd,cd);assert(saved.focus_visible);
    el('drc-restore-layers').onclick();await ack();assert.equal(saved.cd,null);assert(!saved.focus_visible);
    // A restore accepted after a newer click may change server visibility but
    // must not clear the newer selection/focus. An unsent one is cancellable.
    await jump();await ack();el('drc-restore-layers').onclick();const lateRestore=moves.at(-1);lateRestore.sent=true;
    el('drc-errors').children[1].onclick();await tick();await ack(lateRestore);assert.equal(saved.selected.error,'1');assert(saved.focus_visible);
    // No-match/unsupported metadata retains the authoritative visibility; it
    // is reported as unchanged, never silently converted to an empty set.
    for(reason of ['no_metadata','no_rule','no_source_layers','no_match','unsupported_deck']){
        await jump();await ack();assert(!context.state.layers_isolated);assert.match(el('drc-layer-status').textContent,/Layers unchanged/);
    }
    reason='ready';await jump();await ack();const prior=moves.length;
    await el('drc-reload').onclick();await tick();assert.equal(moves.length,prior,'review restore navigated again');assert(!el('drc-restore-layers').disabled);
    // A late HTTP focus or a cancelled queued token cannot resurrect a jump.
    panel.key('K');panel.key('Escape');await ack();hold=true;await jump();const old=held;panel.key('Escape');old.resolve();await tick();hold=false;
    assert(old.token.cancelled);assert.equal(moves.length,prior+1);
    await jump();const unsent=moves.at(-1);panel.key('Escape');await tick();assert(unsent.cancelled);assert(!context.pending);
    // The same view revision on a replacement connection is still a different
    // input context: do not auto-apply an old HTTP preparation after reconnect.
    hold=true;await jump();const reconnect=held;context={...context,state:{...context.state,connection_epoch:'epoch2'}};
    reconnect.resolve();await tick();hold=false;assert.equal(moves.at(-1),unsent);
    for(corrupt of [v=>({...v,prepared_token:'bad'}),v=>({...v,check:'1'}),v=>({...v,layer_isolation:{status:'ready',matched:'0'}}),
        v=>({...v,layer_isolation:{status:'no_match',matched:'3'}}),v=>({...v,layer_isolation:{status:'ready',matched:'01'}}),
        v=>({...v,navigation:{width_um:'0'}})]){
        const n=moves.length;await jump();assert.equal(moves.length,n);assert.match(el('drc-message').textContent,/Invalid|counter/);
    }
    corrupt=null;await jump();const sent=moves.at(-1);sent.sent=true;panel.key('Escape');await ack(sent);assert(!saved.focus_visible);assert.equal(saved.cd,null,'late ACK resurrected CD');
    // Already isolated, with the previous rulers dismissed: Escape must still
    // cancel a new jump before it waits for an in-flight edit to finish.
    await jump();const isolatedSent=moves.at(-1);isolatedSent.sent=true;panel.key('Escape');await ack(isolatedSent);
    assert.equal(saved.cd,null,'isolated pending jump recreated rulers after Escape');
    panel.key('Escape');await ack();assert(!context.state.layers_isolated);
    await jump();await ack();panel.key('K');await jump();const isolatedQueued=moves.at(-1);panel.key('Escape');await tick();
    assert(isolatedQueued.cancelled);assert.equal(moves.at(-1).kind,'restore','queued cancellation did not release restoration');await ack();
    assert(!context.state.layers_isolated);assert.equal(saved.cd,null);
    const activeReads=count('focus');context={...context,connected:false};panel.contextChanged();el('drc-restore-layers').onclick();assert.equal(count('focus'),activeReads);
    assert(el('drc-restore-layers').disabled);panel.stop();
    console.log('WEB DRC ISOLATION UI: ALL OK (ACK-only effects, Restore/Escape, cursor/CD/groups, no-match/reload, stale/cancel/reconnect/invalid DTO)');
})().catch(e=>{console.error(e);process.exitCode=1;});
