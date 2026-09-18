'use strict';
const assert = require('node:assert/strict'), gestures = require('./gestures.js');
function target() {const listeners={};return {listeners,focus(){},addEventListener(k,fn){(listeners[k] ||= []).push(fn);},emit(k,v={}){for(const fn of listeners[k]||[]){fn(v);}}};}
const viewport=target(), win=target(), doc=target(), frames=new Map(), previews=[], pans=[], clicks=[];
let seq=0,stamp='1',ready=true,cursor=false,boxMode=false;
const g=gestures.bind({viewport,window:win,document:doc,stamp:()=>stamp,ready:()=>ready,
    dimensions:()=>({pixels:[800,600],dpr:2}),cursor:v=>{cursor=v;},
    preview:(p,paint)=>previews.push({p,paint}),pan:n=>pans.push(n),click:(...c)=>clicks.push(c),selectionMode:()=>boxMode,
    requestAnimationFrame:fn=>{frames.set(++seq,fn);return seq;},cancelAnimationFrame:id=>frames.delete(id)});
function event(x,y,button=0,buttons=1){return {clientX:x,clientY:y,button,buttons,preventDefault(){}};}
viewport.emit('mousedown',event(100,100));win.emit('mousemove',event(103,102));win.emit('mouseup',event(103,102,0,0));
assert.equal(pans.length,0);assert.equal(cursor,false);assert.equal(frames.size,0);
assert.deepEqual(clicks,[[103,102,false]]);
viewport.emit('mousedown',event(100,100));win.emit('mouseup',{...event(100,100,0,0),detail:2});
assert.deepEqual(clicks.at(-1),[100,100,true]);
for(const modifier of ['ctrlKey','metaKey','altKey','shiftKey']){
    for(const phase of ['press','release']){
        const down=event(100,100),up=event(100,100,0,0);(phase==='press'?down:up)[modifier]=true;
        viewport.emit('mousedown',down);win.emit('mouseup',up);assert.equal(clicks.length,2,modifier+' '+phase);
    }
}
viewport.emit('mousedown',event(100,100,1,4));win.emit('mouseup',event(100,100,1,0));assert.equal(clicks.length,2);
viewport.emit('mousedown',event(100,100,0,3));win.emit('mouseup',event(100,100,0,0));assert.equal(clicks.length,2);
viewport.emit('mousedown',event(100,100));win.emit('mouseup',event(100,100,0,4));assert.equal(clicks.length,2);
viewport.emit('mousedown',event(100,100));viewport.emit('mousedown',event(100,100,1,5));
win.emit('mouseup',event(100,100,1,1));win.emit('mouseup',event(100,100,0,0));assert.equal(clicks.length,2);
viewport.emit('mousedown',event(100,100));win.emit('mousemove',event(101,101,0,3));
win.emit('mouseup',event(101,101,0,0));assert.equal(clicks.length,2);
// Returning to the origin after crossing the pan threshold is NOT a click.
viewport.emit('mousedown',event(100,100));win.emit('mousemove',event(110,110));win.emit('mouseup',event(100,100,0,0));
assert.equal(clicks.length,2);assert.equal(pans.length,0);
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
    else if(kind==='buttons'){win.emit('mousemove',event(170,180,0,2));}
    else if(kind==='hidden'){doc.hidden=true;doc.emit('visibilitychange');doc.hidden=false;}
    else {win.emit(kind);}
    assert.equal(g.active(),false,kind);assert.equal(frames.size,0,kind);
    win.emit('mouseup',event(170,180));assert.equal(pans.length,1,kind);
    assert.equal(clicks.length,2,kind);
}
ready=false;viewport.emit('mousedown',event(0,0));assert.equal(g.active(),false);
ready=true;viewport.emit('mousedown',event(0,0,1,4));win.emit('mouseup',event(20,20,0,0));assert(g.active());
win.emit('mousemove',event(1000,-1000,1,4));win.emit('mouseup',event(1000,-1000,1,0));
assert.deepEqual(pans.at(-1),{kind:'pan',x:-1,y:-1,snap:false});
boxMode=true;
for(const modifiers of [{ctrlKey:true},{metaKey:true},{shiftKey:true},{ctrlKey:true,shiftKey:true},{}]){
    viewport.emit('mousedown',{...event(100,100),...modifiers});win.emit('mouseup',{...event(100,100,0,0),...modifiers});
    assert.deepEqual(clicks.at(-1),[100,100,false,{ctrlKey:!!modifiers.ctrlKey,metaKey:!!modifiers.metaKey,shiftKey:!!modifiers.shiftKey}]);
}
const boxClicks=clicks.length, boxPans=pans.length;
viewport.emit('mousedown',{...event(100,100),ctrlKey:true});win.emit('mousemove',event(120,110));win.emit('mouseup',event(120,110,0,0));
assert.equal(pans.length,boxPans+1);assert.equal(clicks.length,boxClicks,'box-mode drag became a corner');
viewport.emit('mousedown',event(100,100));win.emit('mousemove',event(110,110));win.emit('mouseup',event(100,100,0,0));
assert.equal(clicks.length,boxClicks,'returned drag selected a box');
for(const modifiers of [{altKey:true}]){
    viewport.emit('mousedown',{...event(100,100),...modifiers});win.emit('mouseup',{...event(100,100,0,0),...modifiers});assert.equal(clicks.length,boxClicks);
}
viewport.emit('mousedown',event(100,100));viewport.emit('mousedown',event(100,100,1,5));win.emit('mouseup',event(100,100,0,0));assert.equal(clicks.length,boxClicks);
viewport.emit('mousedown',event(100,100));boxMode=false;win.emit('mouseup',event(100,100,0,0));assert.equal(clicks.length,boxClicks,'mode changed during click');
viewport.emit('mousedown',event(100,100));boxMode=true;win.emit('mouseup',event(100,100,0,0));assert.equal(clicks.length,boxClicks);
console.log('WEB GESTURES: ALL OK (rAF, release-only pan, clicks/modifiers/chords, opt-in box mode, jitter, bounds, lost capture/stale/hidden)');
{
    const v=target(),w=target(),d=target(),picks=[],moves=[];
    const g=gestures.bind({viewport:v,window:w,document:d,stamp:()=>1,ready:()=>true,objectClicks:true,selectionMode:()=>false,
        dimensions:()=>({pixels:[100,80],dpr:1}),cursor(){},preview(){},pan:n=>moves.push(n),click:(...c)=>picks.push(c),
        requestAnimationFrame:()=>1,cancelAnimationFrame(){}});
    for(const m of [{},{shiftKey:true},{ctrlKey:true},{metaKey:true}]){
        v.emit('mousedown',{...event(30,20),...m});w.emit('mouseup',{...event(30,20,0,0),...m});
        assert.equal(picks.length,m.shiftKey?2:m.ctrlKey?3:m.metaKey?4:1);
    }
    for(const key of ['shiftKey','ctrlKey','metaKey'])for(const press of [true,false]){
        v.emit('mousedown',{...event(30,20),[key]:press});w.emit('mouseup',{...event(30,20,0,0),[key]:!press});assert.equal(picks.length,4);
    }
    v.emit('mousedown',{...event(30,20),ctrlKey:true});w.emit('mousemove',{...event(50,20),ctrlKey:true});w.emit('mouseup',{...event(50,20,0,0),ctrlKey:true});
    assert.equal(picks.length,4);assert.equal(moves.length,0);assert(!g.active(),'modifier drag must retain its old no-pan meaning');
    v.emit('mousedown',event(30,20));w.emit('blur');w.emit('mouseup',event(30,20,0,0));assert.equal(picks.length,4);
    assert.deepEqual(picks[0],[30,20,false]);assert.equal(picks[2][3].ctrlKey,true);
}
console.log('WEB OBJECT GESTURES: ALL OK (opt-in modifiers, unchanged plain click, release consistency, drag/cancel isolation)');
{
    const v=target(),w=target(),d=target(),bands=[],boxes=[],warnings=[],frames=new Map();
    let id=0,stamp=1,ready=true;
    v.getBoundingClientRect=()=>({left:10,top:20});
    const g=gestures.bind({viewport:v,window:w,document:d,ready:()=>true,bandReady:()=>ready,stamp:()=>stamp,
        dimensions:()=>({pixels:[800,600],dpr:2,left:0,top:0}),cursor(){},preview(){assert.fail('band moved pan pixels');},
        pan(){assert.fail('band became pan');},click(){assert.fail('band became pick');},band:n=>bands.push(n),bandPreview:b=>boxes.push(b),notice:n=>warnings.push(n),
        requestAnimationFrame:f=>{frames.set(++id,f);return id;},cancelAnimationFrame:i=>frames.delete(i)});
    const down=(x,y)=>v.emit('mousedown',event(x,y,2,2));
    const move=(x,y)=>w.emit('mousemove',event(x,y,2,2));
    const up=(x,y)=>w.emit('mouseup',event(x,y,2,0));
    down(110,120);move(210,121);assert(g.bandActive());assert.equal(bands.length,0);assert.equal(frames.size,1);
    const draw=[...frames.values()][0];frames.clear();draw();assert.equal(boxes.at(-1).outward,false);
    up(210,121);assert.deepEqual(bands.at(-1),{kind:'band',start:[.25,1/3],end:[.5,1/3+1/300],axes:[true,false],outward:false});
    assert(!g.active());assert.equal(boxes.at(-1),null);
    down(210,120);move(110,120);up(213,120);assert.equal(bands.length,1);assert.equal(warnings.at(-1),'Zoom band cancelled');
    down(210,120);move(110,120);up(215,160);assert.deepEqual(bands.at(-1).axes,[false,true]);assert.equal(bands.at(-1).outward,true);
    down(210,120);move(110,120);up(310,160);assert.equal(bands.at(-1).outward,false,'tie must zoom in');
    down(110,120);up(114.9,124.9);assert.equal(bands.length,3,'subthreshold click navigated');
    down(110,120);up(115,120);assert.equal(bands.length,4,'5px horizontal band was swallowed by pan jitter');
    for(const kind of ['blur','resize','pagehide','hidden','stale','not-displayed','buttons','chord']){
        down(110,120);move(160,150);
        if(kind==='hidden'){d.hidden=true;d.emit('visibilitychange');d.hidden=false;}
        else if(kind==='stale'){stamp++;move(170,160);}
        else if(kind==='not-displayed'){ready=false;move(170,160);ready=true;}
        else if(kind==='buttons'){w.emit('mousemove',event(170,160,2,1));}
        else if(kind==='chord'){v.emit('mousedown',event(170,160,0,3));}
        else{w.emit(kind);}
        assert(!g.active(),kind);assert.equal(frames.size,0,kind);assert.equal(boxes.at(-1),null,kind);
        up(170,160);assert.equal(bands.length,4,kind);
    }
    let blocked=false;v.emit('contextmenu',{preventDefault(){blocked=true;}});assert(blocked);
}
console.log('WEB BAND GESTURES: ALL OK (release-only, dominant direction/tie/wobble, 5px axes, DPI, no pick/pan, stale/cancel/hidden/chords)');

// Release races use a deterministic clock, not sleeps or a permissive ready
// mock that changes the existing cancellation/navigation contract.
function releaseHarness(button) {
    const v=target(),w=target(),d=target(),timers=new Map(),frames=new Map(),sent=[],clicks=[];
    let clock=0,id=0,stamp=1,ready=true,bandReady=true;
    const mask=button===0?1:button===1?4:2;
    w.setTimeout=(fn,ms)=>{const n=++id;timers.set(n,{fn,at:clock+ms});return n;};
    w.clearTimeout=n=>timers.delete(n);
    v.getBoundingClientRect=()=>({left:0,top:0});
    const g=gestures.bind({viewport:v,window:w,document:d,ready:()=>ready,bandReady:()=>bandReady,stamp:()=>stamp,
        dimensions:()=>({pixels:[800,600],dpr:2}),cursor(){},preview(){},bandPreview(){},
        pan:n=>sent.push(n),band:n=>sent.push(n),click:(...a)=>clicks.push(a),
        requestAnimationFrame:fn=>{const n=++id;frames.set(n,fn);return n;},cancelAnimationFrame:n=>frames.delete(n)});
    const mouse=(type,x,y,buttons=type==='mouseup'?0:mask,b=button)=>{
        (type==='mousedown'?v:w).emit(type,event(x,y,b,buttons));
    };
    const advance=ms=>{clock+=ms;for(const [n,t] of [...timers])if(t.at<=clock){timers.delete(n);t.fn();}};
    const raf=()=>{for(const [n,f] of [...frames]){frames.delete(n);f();}};
    return {w,d,g,timers,frames,sent,clicks,mouse,advance,raf,
        stale(){stamp++;},notReady(){ready=false;},notDisplayed(){bandReady=false;}};
}
let releaseCases=0;
for(const button of [0,1,2]) {
    for(const paintFirst of [false,true]) for(const zeroMove of [false,true]) {
        const h=releaseHarness(button);
        h.mouse('mousedown',100,100);h.mouse('mousemove',150,130);
        if(paintFirst)h.raf();
        if(zeroMove){h.mouse('mousemove',900,900,0);h.advance(99);}
        assert(h.g.active());assert.equal(h.sent.length,0,'no submission before release');
        h.mouse('mouseup',160,140);
        assert.equal(h.sent.length,1,'moving release must submit exactly once');
        if(button===2){assert.deepEqual(h.sent[0],{kind:'band',start:[.25,1/3],end:[.4,1/3+80/600],axes:[true,true],outward:false});}
        else{assert.deepEqual(h.sent[0],{kind:'pan',x:-.15,y:80/600,snap:false});}
        assert(!h.g.active());assert.equal(h.timers.size,0);assert.equal(h.frames.size,0);
        h.mouse('mouseup',160,140);h.mouse('mousemove',170,150,0);h.advance(200);h.raf();
        assert.equal(h.sent.length,1);assert.equal(h.clicks.length,0);releaseCases++;
    }
    for(const reason of ['timeout','blur','resize','pagehide','hidden','escape','stale','not-ready',...(button===2?['not-displayed']:[])]) {
        const h=releaseHarness(button);
        h.mouse('mousedown',100,100);h.mouse('mousemove',150,130);h.mouse('mousemove',151,131,0);
        if(reason==='timeout'){
            h.advance(99);h.mouse('mousemove',500,500,0);h.advance(1);
            assert(!h.g.active(),'zero-button moves must not extend the deadline');
        }else if(reason==='hidden'){h.d.hidden=true;h.d.emit('visibilitychange');}
        else if(reason==='escape'){h.g.cancel();} // Owner and guest Escape call this API.
        else if(reason==='stale'){h.stale();}
        else if(reason==='not-ready'){h.notReady();}
        else if(reason==='not-displayed'){h.notDisplayed();}
        else{h.w.emit(reason);}
        h.mouse('mouseup',160,140);h.advance(200);h.raf();
        assert.equal(h.sent.length,0,reason);assert.equal(h.clicks.length,0,reason);
        assert(!h.g.active());assert.equal(h.timers.size,0);assert.equal(h.frames.size,0);releaseCases++;
    }
    const resume=releaseHarness(button);
    resume.mouse('mousedown',100,100);resume.mouse('mousemove',150,130);resume.mouse('mousemove',151,131,0);
    resume.mouse('mousemove',160,140);resume.advance(200);assert(resume.g.active());
    resume.mouse('mouseup',170,150);assert.equal(resume.sent.length,1);releaseCases++;
    const restart=releaseHarness(button);
    restart.mouse('mousedown',100,100);restart.mouse('mousemove',150,130);restart.mouse('mousemove',151,131,0);
    restart.mouse('mousedown',300,300);restart.advance(200);assert(restart.g.active());
    restart.mouse('mouseup',320,310);assert.equal(restart.sent.length,1);
    if(button!==2)assert.deepEqual(restart.sent[0],{kind:'pan',x:-.05,y:20/600,snap:false});
    releaseCases++;
    const wrong=releaseHarness(button);
    wrong.mouse('mousedown',100,100);wrong.mouse('mousemove',150,130);wrong.mouse('mousemove',151,131,0);
    wrong.mouse('mouseup',160,140,0,(button+1)%3);assert(wrong.g.active());
    wrong.advance(100);wrong.mouse('mouseup',160,140);assert.equal(wrong.sent.length,0);releaseCases++;
}
console.log('WEB RELEASE ORDER: ALL OK ('+releaseCases+' cases; left/middle pan + right band, immediate/painted, lost input/cancel, no delayed navigation)');
