'use strict';
const assert=require('node:assert/strict'), P=require('./protocol.js'), S=require('./panel-state.js');
const calls=[],timers=new Map(),applied=[],messages=[];let clock=0;
function http(method,path,body,missing,token){return new Promise((resolve,reject)=>{
    const r={method,path,body,token,resolve,reject,done:false};calls.push(r);
    token.abort=()=>{r.done=true;reject(new Error('aborted'));};
});}
const saver=S.bind({http,protocol:P,apply:async s=>applied.push(s),status:s=>messages.push(s),
    setTimeout:fn=>{timers.set(++clock,fn);return clock;},clearTimeout:id=>timers.delete(id)});
const scope={path:'/api/v1/drc/d/views/v/panel',revision:'drc-rev',view:'v'};
function request(method){const r=calls.find(r=>!r.done&&r.method===method);assert(r,'missing '+method);return r;}
function response(r,body,rev='1',ctx=scope){r.done=true;r.token.abort=null;r.resolve({view_id:ctx.view,revision:ctx.revision,state:{panel_rev:rev,body}});}
async function tick(){for(let i=0;i<10;i++)await Promise.resolve();}
async function flush(){for(const [id,fn] of [...timers]){timers.delete(id);fn();}await tick();}
(async()=>{
    let opening=saver.attach(scope);response(request('GET'),null);await opening;
    assert(saver.ready());assert.deepEqual(applied,[null]);
    const a={selected:'9007199254740993',markers:true};saver.change(a);a.markers=false;
    await flush();let save=request('POST');assert.equal(save.body.body.markers,true);assert.equal(save.body.base_panel_rev,'1');
    for(let i=0;i<200;i++)saver.change({selected:String(i),markers:false});
    await flush();assert.equal(calls.filter(r=>r.method==='POST').length,1,'active save did not bound queue');
    response(save,{markers:true,selected:'9007199254740993'},'2');await tick();await flush();
    save=request('POST');assert.equal(save.body.body.selected,'199');assert.equal(save.body.base_panel_rev,'2');
    response(save,save.body.body,'3');await tick();
    saver.change({selected:'199',markers:false});await flush();assert.equal(calls.filter(r=>r.method==='POST').length,2,'identical state was resent');
    // Lost reply/conflict: keep local UI but disable blind writes until an
    // explicit server-state read. Neither active nor latest is replayed.
    saver.change({selected:'200'});await flush();save=request('POST');
    saver.change({selected:'201'});save.done=true;save.reject(new Error('conflict'));await tick();await flush();
    assert(!saver.ready());const count=calls.length;saver.change({selected:'202'});await flush();assert.equal(calls.length,count);
    assert(messages.at(-1).includes('Reload review'));
    opening=saver.attach(scope);response(request('GET'),{selected:'newer-tab'},'4');await opening;
    assert.deepEqual(applied.at(-1),{selected:'newer-tab'});
    saver.change({selected:'203'});await flush();const old=request('POST');
    const next={...scope,path:'/api/v1/drc/d/views/new/panel',view:'new'};
    opening=saver.attach(next);assert(old.token.cancelled);response(old,{selected:'203'},'5');await tick();
    response(request('GET'),null,'1',next);await opening;assert.equal(applied.at(-1),null);
    saver.change({selected:'0'});await flush();save=request('POST');assert.equal(save.body.base_panel_rev,'1');
    saver.close();assert(save.token.cancelled);assert.equal(timers.size,0);assert(!saver.ready());
    opening=saver.attach(scope);const failedRead=request('GET');failedRead.done=true;failedRead.reject(new Error('unavailable'));await opening;
    assert(!saver.ready());assert(messages.at(-1).includes('Reload review'));
    opening=saver.attach(scope);response(request('GET'),null,'7');await opening;
    const beforeOversize=calls.length;saver.change({text:'x'.repeat(4097)});await flush();assert.equal(calls.length,beforeOversize);
    saver.change({selected:'old-revision'});await flush();save=request('POST');response(save,save.body.body,'6');await tick();
    assert(!saver.ready(),'decreasing response revision was accepted');
    saver.close();
    console.log('WEB PANEL STATE: ALL OK (bounded/latest saves, u64, immutable snapshots, CAS revision, conflict/timeout, old-context cancellation)');
})().catch(e=>{console.error(e);process.exitCode=1;});
