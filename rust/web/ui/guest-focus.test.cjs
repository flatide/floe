'use strict';
const assert=require('node:assert/strict'),F=require('./guest-focus.js');
const P=require('./protocol.js');
function setup(){let c={view_id:'view',epoch:'epoch',mode:'explore',state_rev:'9007199254740993',otherInput:false};const events=[],timers=new Map();let timerId=0;
    const f=F.bind({protocol:P,context:()=>c,setTimeout(fn,ms){assert.equal(ms,8000);timers.set(++timerId,fn);return timerId;},clearTimeout(id){timers.delete(id);}});
    return {f,events,timers,begin(){return f.begin(c,(...a)=>events.push(a));},state(extra){c=Object.assign({},c,extra);f.changed();}};}
let e=setup(),t=e.begin();t.sent('7');assert.equal(e.f.accepted('8','9007199254740994'),false);assert.equal(e.events.length,0);
e.f.accepted('7','9007199254740994');assert.equal(e.events.length,0,'ACK alone is insufficient');
e.state({state_rev:'9007199254740994'});assert.equal(e.events[0][0],null);assert.equal(e.timers.size,0);e.f.changed();assert.equal(e.events.length,1);
e=setup();t=e.begin();t.sent('1');e.state({state_rev:'9007199254740994'});assert.equal(e.events.length,0,'snapshot alone is insufficient');
e.f.accepted('1','9007199254740994');assert.equal(e.events[0][0],null,'both arrival orders work');
for(const event of ['skip','input','epoch','follow','timeout','cancel','reset','reject']){
    e=setup();t=e.begin();t.sent('1');
    if(event==='skip'){e.state({state_rev:'9007199254740995'});e.f.accepted('1','9007199254740994');}
    if(event==='input'){e.state({otherInput:true});}
    if(event==='epoch'){e.state({epoch:'new'});}
    if(event==='follow'){e.state({mode:'follow'});}
    if(event==='timeout'){[...e.timers.values()][0]();}
    if(event==='cancel'){t.cancel();}
    if(event==='reset'){e.f.reset();}
    if(event==='reject'){assert(e.f.rejected('1'));}
    assert.equal(e.events.length,1,event);assert.equal(typeof e.events[0][0],'string',event);assert.equal(e.timers.size,0,event);
    e.state({state_rev:'9007199254740994',otherInput:false,mode:'explore',epoch:'epoch'});e.f.accepted('1','9007199254740994');assert.equal(e.events.length,1,'late response stays retired');
}
e=setup();t=e.begin();assert.throws(()=>e.begin());t.sent('1');assert.throws(()=>t.sent('2'));t.cancel();
console.log('GUEST FOCUS RECEIPT: ALL OK (exact ACK+snapshot both orders, u64, no replay, skip/input/reject/cancel/timeout/identity)');
