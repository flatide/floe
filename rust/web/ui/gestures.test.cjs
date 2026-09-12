'use strict';
const assert = require('node:assert/strict'), gestures = require('./gestures.js');
function target() {const listeners={};return {listeners,focus(){},addEventListener(k,fn){(listeners[k] ||= []).push(fn);},emit(k,v={}){for(const fn of listeners[k]||[]){fn(v);}}};}
const viewport=target(), win=target(), doc=target(), frames=new Map(), previews=[], pans=[];
let seq=0,stamp='1',ready=true,cursor=false;
const g=gestures.bind({viewport,window:win,document:doc,stamp:()=>stamp,ready:()=>ready,
    dimensions:()=>({pixels:[800,600],dpr:2}),cursor:v=>{cursor=v;},
    preview:(p,paint)=>previews.push({p,paint}),pan:n=>pans.push(n),
    requestAnimationFrame:fn=>{frames.set(++seq,fn);return seq;},cancelAnimationFrame:id=>frames.delete(id)});
function event(x,y,button=0,buttons=1){return {clientX:x,clientY:y,button,buttons,preventDefault(){}};}
viewport.emit('mousedown',event(100,100));win.emit('mousemove',event(103,102));win.emit('mouseup',event(103,102));
assert.equal(pans.length,0);assert.equal(cursor,false);assert.equal(frames.size,0);
viewport.emit('mousedown',event(100,100));
for(let i=1;i<=100;i++){win.emit('mousemove',event(100+i,100+i/2));}
assert.equal(frames.size,1);assert.equal(pans.length,0);
const fn=[...frames.values()][0];frames.clear();fn();
assert.deepEqual(previews.at(-1),{p:[-200,-100],paint:true});
win.emit('mouseup',event(200,150));assert.equal(pans.length,1);
assert.deepEqual(pans[0],{kind:'pan',x:-0.25,y:1/6,snap:false});assert.equal(g.active(),false);
for(const kind of ['blur','resize','pagehide','stale','buttons','hidden']){
    viewport.emit('mousedown',event(100,100));win.emit('mousemove',event(150,140));
    if(kind==='stale'){stamp='2';win.emit('mousemove',event(170,180));}
    else if(kind==='buttons'){win.emit('mousemove',event(170,180,0,0));}
    else if(kind==='hidden'){doc.hidden=true;doc.emit('visibilitychange');doc.hidden=false;}
    else {win.emit(kind);}
    assert.equal(g.active(),false,kind);assert.equal(frames.size,0,kind);
    win.emit('mouseup',event(170,180));assert.equal(pans.length,1,kind);
}
ready=false;viewport.emit('mousedown',event(0,0));assert.equal(g.active(),false);
ready=true;viewport.emit('mousedown',event(0,0,1,4));win.emit('mouseup',event(20,20,0,0));assert(g.active());
win.emit('mousemove',event(1000,-1000,1,4));win.emit('mouseup',event(1000,-1000,1,0));
assert.deepEqual(pans.at(-1),{kind:'pan',x:-1,y:-1,snap:false});
console.log('WEB GESTURES: ALL OK (rAF pacing, one release input, jitter, bounds, lost capture/stale/hidden)');
