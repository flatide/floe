'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),P=require('./presets.js');
const tick=()=>new Promise(setImmediate);
function data(){
    const lines=name=>fs.readFileSync(__dirname+'/../../../floe/'+name,'utf8').split('\n').map(l=>l.trim()).filter(l=>l&&!l.startsWith('#')).map(l=>l.split(/\s+/));
    return {version:1,colors:lines('colornames.def').map(([name,color])=>({name,color:'#'+color.toLowerCase()})),
        fills:lines('fillpatterns.def').map(([name,...words])=>{const rows=words.map(w=>parseInt(w,16));return {name,rows,fill:['solid','clear','speckle'].includes(name)?{kind:name}:{kind:'pattern',rows}};})};
}
function harness(){
    const nodes=new Map(),reads=[],edits=[];let available=true,enabled=false;
    class Element{
        constructor(tag){this.tag=tag;this.children=[];this.style={};this.open=false;this.calls=[];this.attributes={};}
        set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}
        appendChild(c){this.children.push(c);}setAttribute(k,v){this.attributes[k]=v;}
        getContext(){const calls=this.calls;return {fillRect(x,y,w,h){calls.push([this.fillStyle,x,y,w,h]);}};}
    }
    const document={createElement:tag=>new Element(tag)};
    const el=id=>{if(!nodes.has(id)){nodes.set(id,new Element('div'));}return nodes.get(id);};
    const ui=P.bind({el,document,available:()=>available,enabled:()=>enabled,apply:b=>edits.push(b),
        http(method,path,body,missing,token){assert.equal(method,'GET');assert.equal(path,'/api/v1/palette/presets');assert.equal(body,undefined);return new Promise((resolve,reject)=>{const q={resolve,reject,token};reads.push(q);token.abort=()=>q.aborted=true;});}});
    return {el,ui,reads,edits,document,set available(v){available=v;ui.changed();},set enabled(v){enabled=v;ui.changed();},
        open(){el('palette-presets').open=true;el('palette-presets').ontoggle();},
        async reply(q=reads.at(-1),v=data()){q.resolve(v);await tick();},async fail(){reads.at(-1).reject(Error('unavailable'));await tick();}};
}
async function main(){
    const d=data();P.validate(d);assert.equal(d.colors.length,49);assert.equal(d.fills.length,20);
    assert.equal(d.colors[7].color,d.colors[8].color);assert.notEqual(d.colors[7].name,d.colors[8].name);
    for(const bad of [{}, {...d,version:2},{...d,colors:[]},{...d,colors:Array(257).fill(d.colors[0])},
        {...d,colors:[d.colors[0],d.colors[0]]},{...d,colors:[{name:'<bad>',color:'#000000'}]},
        {...d,colors:[{name:'safe',color:'url(https://wrong)'}]},
        {...d,fills:[{...d.fills[0],rows:[1]}]}, {...d,fills:[{...d.fills[0],fill:{kind:'pattern',rows:Array(16).fill(1)}}]}]) {assert.throws(()=>P.validate(bad));}
    const h=harness();assert.equal(h.reads.length,0);h.open();assert.equal(h.reads.length,1);await h.reply();
    assert.equal(h.edits.length,0);assert.equal(h.el('presets-colors').children.length,49);assert.equal(h.el('presets-fills').children.length,20);
    assert(h.el('presets-colors').children.every(b=>b.disabled));h.enabled=true;
    for(let i=0;i<49;i++){const b=h.el('presets-colors').children[i];assert(b.title.startsWith(d.colors[i].name));b.onclick();assert.deepEqual(h.edits.at(-1),{color:d.colors[i].color});}
    for(let i=0;i<20;i++){
        const b=h.el('presets-fills').children[i],canvas=b.children[0];assert.equal(canvas.width,16);assert.equal(canvas.height,16);
        const expected=[['#ffffff',0,0,16,16]];
        d.fills[i].rows.forEach((row,y)=>row.toString(2).padStart(16,'0').split('').forEach((bit,x)=>{if(bit==='1'){expected.push(['#000000',x,y,1,1]);}}));
        assert.deepEqual(canvas.calls,expected,'MSB-left preview '+d.fills[i].name);b.onclick();assert.deepEqual(h.edits.at(-1),{fill:d.fills[i].fill});
    }
    const count=h.edits.length;h.enabled=false;h.el('presets-colors').children[0].onclick();assert.equal(h.edits.length,count);
    h.available=false;h.available=true;assert.equal(h.reads.length,1,'immutable palette was fetched again');h.ui.stop();assert(h.el('presets-colors').children.every(b=>b.disabled));
    const f=harness();f.open();await f.fail();assert.equal(f.el('presets-retry').hidden,false);f.ui.changed();assert.equal(f.reads.length,1);
    f.el('presets-retry').onclick();await f.reply();assert.equal(f.el('presets-retry').hidden,true);
    const stale=harness();stale.open();const q=stale.reads[0];stale.available=false;assert(q.aborted);await stale.reply(q);assert.equal(stale.el('presets-colors').children.length,0);
    stale.available=true;assert.equal(stale.reads.length,2);await stale.reply();assert.equal(stale.el('presets-fills').children.length,20);
    const malformed=harness();malformed.open();await malformed.reply(undefined,{...d,fills:[{name:'last_bad',rows:[]} ]});assert.equal(malformed.el('presets-colors').children.length,0,'partial palette was displayed');
    console.log('WEB PRESETS: ALL OK (49 colors, 20 exact MSB-left previews, bounded immutable read, one-field writes, retry/cancel/stale)');
}
main().catch(e=>{console.error(e);process.exitCode=1;});
