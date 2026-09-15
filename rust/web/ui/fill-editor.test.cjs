'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),F=require('./fill-editor.js');
function presets(){return fs.readFileSync(__dirname+'/../../../floe/fillpatterns.def','utf8').split('\n').map(l=>l.trim()).filter(l=>l&&!l.startsWith('#')).map(l=>{const [name,...words]=l.split(/\s+/);return {name,rows:words.map(w=>parseInt(w,16))};});}
const ev=(v={})=>Object.assign({button:0,buttons:1,detail:0,clientX:1,clientY:1,preventDefault(){},stopPropagation(){}},v);
function harness(pointer=false){
    const nodes=new Map(),writes=[],events={},wevents={};
    let context={id:'a'.repeat(64),epoch:'b'.repeat(64),rev:'12',slotKey:'c'.repeat(40),ready:true,idle:true,fillEdit:true};
    class Element{
        constructor(){this.children=[];this.dataset={};this.disabled=false;this.hidden=false;this.attributes={};}
        appendChild(c){this.children.push(c);}setAttribute(k,v){this.attributes[k]=v;}
        set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}
        focus(){document.activeElement=this;if(this.onfocus){this.onfocus();}}
        getBoundingClientRect(){return {left:0,top:0,width:256,height:256};}
        setPointerCapture(id){this.capture=id;}
    }
    const document={createElement:()=>new Element(),addEventListener:(k,f)=>events[k]=f};
    const window={addEventListener:(k,f)=>wevents[k]=f};if(pointer){window.PointerEvent=function(){};}
    const el=id=>{if(!nodes.has(id)){nodes.set(id,new Element());}return nodes.get(id);};
    const ui=F.bind({el,document,window,context:()=>context,preview:(canvas,bits)=>canvas.bits=bits,changed(){},
        submit(c,body,done){const w={context:c,body,done,cancelled:false};writes.push(w);return ()=>{w.cancelled=true;return false;};}});
    return {ui,el,document,writes,events,wevents,open:(value=Array(16).fill(0),base=presets()[15].rows)=>ui.open('brick',value,base),
        get state(){return context;},change(p){context=Object.assign({},context,p);ui.changed();},
        bits:()=>el('fill-slot-preview').bits,apply:()=>el('fill-slot-editor').onsubmit(ev()),click:i=>el('fill-slot-grid').children[i].onclick(ev()),
        cancel:()=>el('fill-slot-cancel').onclick()};
}
for(const p of presets()){
    const initial=p.rows.slice(),d=F.draft(initial,p.rows);initial.fill(0);
    assert.deepEqual(d.rows(),p.rows);const copy=d.rows();copy.fill(0);assert.deepEqual(d.rows(),p.rows);
    d.operation('invert');assert.deepEqual(d.rows(),p.rows.map(n=>n^65535));d.operation('reset');assert.deepEqual(d.rows(),p.rows);
    d.press(0,0);const ink=!(p.rows[0]&0x8000);d.move(15,15);d.move(0,0);d.move(-1,0);d.move(16,15);d.release();d.move(1,0);
    const expected=p.rows.slice();expected[0]=ink?expected[0]|0x8000:expected[0]&~0x8000;expected[15]=ink?expected[15]|1:expected[15]&~1;assert.deepEqual(d.rows(),expected);
    d.operation('clear');assert.deepEqual(d.rows(),Array(16).fill(0));d.operation('solid');assert.deepEqual(d.rows(),Array(16).fill(65535));
    d.operation('reset');d.press(-1,0);d.move(0,0);assert.deepEqual(d.rows(),p.rows);assert.throws(()=>d.operation('unknown'));assert.deepEqual(d.rows(),p.rows);
}
for(const bad of [[],Array(15).fill(0),Array(16).fill(-1),Array(16).fill(65536),Array(16).fill(0.5),null]){assert.throws(()=>F.draft(bad,Array(16).fill(0)));}
if(process.env.FLOE_BITMAP_SLOT_ORACLE){
    const cases=JSON.parse(fs.readFileSync(process.env.FLOE_BITMAP_SLOT_ORACLE,'utf8')),base=new Map(presets().map(p=>[p.name,p.rows]));assert.equal(cases.length,324);
    for(const c of cases){
        const original=c.initial.slots.find(p=>p.name===c.name).rows,d=F.draft(original,base.get(c.name));let applied=false;
        for(const step of c.events){if(Array.isArray(step)){d[step[0]](...step.slice(1));}else if(step>=10){d.operation(['clear','solid','invert','reset'][step-10]);}else{applied=step===-5;}}
        assert.deepEqual(applied?d.rows():original,c.after.slots.find(p=>p.name===c.name).rows);
    }
    console.log('GTK WEB BITMAP DRAFT: ALL OK (324 actual GTK event traces)');
}
const h=harness();assert(!h.ui.open('solid',Array(16).fill(0),Array(16).fill(0)));h.change({fillEdit:false});assert(!h.open());h.change({fillEdit:true});assert(h.open());
assert.equal(h.writes.length,0);assert.equal(h.el('fill-slot-grid').children.length,256);assert.equal(h.el('fill-slot-grid').children.filter(b=>b.tabIndex===0).length,1);
h.click(0);assert.equal(h.bits()[0],0x8000);h.el('fill-slot-grid').children[0].onclick(ev({detail:1}));assert.equal(h.bits()[0],0x8000,'pointer click toggled twice');
h.el('fill-slot-grid').onkeydown(ev({key:'ArrowRight'}));assert.equal(h.document.activeElement,h.el('fill-slot-grid').children[1]);h.click(1);assert.equal(h.bits()[0],0xc000);
h.el('fill-slot-grid').onkeydown(ev({key:'End',ctrlKey:true}));assert.equal(h.document.activeElement,h.el('fill-slot-grid').children[255]);
h.el('fill-slot-clear').onclick();h.el('fill-slot-reset').onclick();assert.deepEqual(h.bits(),presets()[15].rows);h.cancel();assert(!h.ui.active());assert.equal(h.writes.length,0);assert.equal(h.document.activeElement,h.el('fill-slot-open'));
assert(h.open());h.el('fill-slot-grid').onmousedown(ev());h.events.mousemove(ev({clientX:255,clientY:255}));h.events.mousemove(ev());h.events.mousemove(ev({clientX:-1}));h.events.mouseup();h.events.mousemove(ev({clientX:33}));
assert.equal(h.bits()[0],0x8000);assert.equal(h.bits()[15],1);h.apply();h.apply();assert.equal(h.writes.length,1);assert(h.el('fill-slot-apply').disabled);assert.equal(h.writes[0].context.rev,'12');
h.change({rev:'13',slotKey:'d'.repeat(40),idle:false});assert(h.ui.active(),'own pending result discarded on snapshot');h.writes[0].done(null);assert(!h.ui.active());assert.match(h.el('fill-slot-status').textContent,/applied/);
h.change({idle:true});h.open();h.click(0);h.change({rev:'14'});assert(!h.ui.active());h.apply();assert.equal(h.writes.length,1,'stale draft submitted');
h.open();h.apply();const pending=h.writes.at(-1);h.change({ready:false});assert(pending.cancelled);assert(!h.ui.active());pending.done(null);assert.match(h.el('fill-slot-status').textContent,/not replayed/);h.change({ready:true,epoch:'e'.repeat(64)});assert.equal(h.writes.length,2);
h.open();h.apply();h.writes.at(-1).done('stale_state');assert(!h.ui.active());assert.equal(h.writes.length,3);h.ui.changed();assert.equal(h.writes.length,3);
h.open();h.el('fill-slot-grid').onkeydown(ev({key:'Escape',isComposing:true}));assert(h.ui.active());h.el('fill-slot-grid').onkeydown(ev({key:'Escape'}));assert(!h.ui.active());
const p=harness(true);p.open();const grid=p.el('fill-slot-grid');grid.onpointerdown(ev({pointerId:7}));grid.onpointermove(ev({pointerId:8,clientX:255,clientY:255}));assert.equal(p.bits()[15],0);
grid.onpointermove(ev({pointerId:7,clientX:255,clientY:255}));grid.onpointercancel(ev({pointerId:7}));grid.onpointermove(ev({pointerId:7,clientX:33}));assert.equal(p.bits()[0],0x8000);assert.equal(p.bits()[15],1);
grid.onpointerdown(ev({pointerId:9}));p.wevents.blur();grid.onpointermove(ev({pointerId:9,clientX:33}));assert.equal(p.bits()[0],0);p.cancel();assert.equal(p.writes.length,0);
p.open();grid.setPointerCapture=()=>{throw Error('pointer no longer active');};grid.onpointerdown(ev({pointerId:10}));
grid.onpointermove(ev({pointerId:10,clientX:33}));assert.equal(p.bits()[0],0x8000,'failed capture left a live stroke');
assert.match(p.el('fill-slot-status').textContent,/individual clicks/);p.cancel();assert.equal(p.writes.length,0);
console.log('WEB FILL EDITOR: ALL OK (MSB/drag/reset/copy, pointer+mouse+keyboard, explicit apply, CAS/lifecycle, no save/replay)');
