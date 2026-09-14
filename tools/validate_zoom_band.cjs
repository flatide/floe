'use strict';
// Development oracle bridge; runs the production gesture classifier, not a copy.
const fs=require('node:fs'),G=require('../rust/web/ui/gestures.js');
const cases=JSON.parse(fs.readFileSync(0,'utf8'));
function target(){const handlers={};return {addEventListener(k,f){(handlers[k] ||= []).push(f);},emit(k,e){for(const f of handlers[k]||[]){f(e);}},focus(){}};}
for(const c of cases){
    const v=target(),w=target(),d=target(),calls=[];
    v.getBoundingClientRect=()=>({left:17.25,top:9.5});
    G.bind({viewport:v,window:w,document:d,ready:()=>true,bandReady:()=>true,stamp:()=>1,
        dimensions:()=>({pixels:c.pixels,dpr:c.dpr}),cursor(){},preview(){},bandPreview(){},
        band:n=>calls.push(n),requestAnimationFrame:()=>1,cancelAnimationFrame(){}});
    const event=(p,buttons)=>({clientX:p[0]+17.25,clientY:p[1]+9.5,button:2,buttons,preventDefault(){}});
    v.emit('mousedown',event(c.start,2));
    for(const p of c.moves){w.emit('mousemove',event(p,2));}
    w.emit('mouseup',event(c.moves.at(-1),0));
    if(calls.length>1){throw new Error('Repeated navigation');}c.navigation=calls[0]||null;
}
process.stdout.write(JSON.stringify(cases));
