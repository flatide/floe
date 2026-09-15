'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),P=require('./presets.js'),F=require('./fill-editor.js');
const tick=()=>new Promise(setImmediate);
function data(){
    const lines=name=>fs.readFileSync(__dirname+'/../../../floe/'+name,'utf8').split('\n').map(l=>l.trim()).filter(l=>l&&!l.startsWith('#')).map(l=>l.split(/\s+/));
    return {version:1,colors:lines('colornames.def').map(([name,color])=>({name,color:'#'+color.toLowerCase()})),
        fills:lines('fillpatterns.def').map(([name,...words])=>{const rows=words.map(w=>parseInt(w,16));return {name,rows,fill:['solid','clear','speckle'].includes(name)?{kind:name}:{kind:'pattern',rows}};})};
}
function harness(){
    const nodes=new Map(),reads=[],edits=[],slotEdits=[];let available=true,enabled=false;
    let state={id:'a'.repeat(64),epoch:'b'.repeat(64),rev:'1',slotKey:'c'.repeat(40),ready:true,idle:true,fillEdit:true};
    class Element{
        constructor(tag){this.tag=tag;this.children=[];this.style={};this.dataset={};this.open=false;this.calls=[];this.attributes={};}
        set width(v){this._width=v;this.calls=[];}get width(){return this._width;}
        set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}
        appendChild(c){this.children.push(c);if(c.tag==='option'&&!this.value){this.value=c.value;}}setAttribute(k,v){this.attributes[k]=v;}
        focus(){document.activeElement=this;if(this.onfocus){this.onfocus();}}
        getBoundingClientRect(){return {left:0,top:0,width:256,height:256};}
        getContext(){const calls=this.calls;return {fillRect(x,y,w,h){calls.push([this.fillStyle,x,y,w,h]);}};}
    }
    const document={createElement:tag=>new Element(tag),addEventListener(){}};
    const el=id=>{if(!nodes.has(id)){nodes.set(id,new Element('div'));}return nodes.get(id);};
    const ui=P.bind({el,document,window:{addEventListener(){}},context:()=>state,slotEditor:F,editSlot:(context,body,done)=>slotEdits.push({context,body,done}),available:()=>available,enabled:()=>enabled,apply:b=>edits.push(b),
        http(method,path,body,missing,token){assert.equal(method,'GET');assert(path==='/api/v1/palette/presets'||path.includes('/fill-slots/'));assert.equal(body,undefined);return new Promise((resolve,reject)=>{const q={resolve,reject,token,path,state:Object.assign({},state)};reads.push(q);token.abort=()=>q.aborted=true;});}});
    function session(q=reads.at(-1)){return {version:1,view_id:q.state.id,fill_slots_key:q.state.slotKey,editable:true,fills:data().fills.map(p=>({name:p.name,rows:p.rows}))};}
    return {el,ui,reads,edits,slotEdits,document,session,set available(v){available=v;ui.changed();},set enabled(v){enabled=v;ui.changed();},
        change(p){state=Object.assign({},state,p);ui.changed();},
        open(){el('palette-presets').open=true;el('palette-presets').ontoggle();},
        async ready(){await this.reply();await this.reply();},
        async reply(q=reads.at(-1),v){q.resolve(v||(q.path==='/api/v1/palette/presets'?data():session(q)));await tick();},async fail(){reads.at(-1).reject(Error('unavailable'));await tick();}};
}
async function main(){
    const d=data();P.validate(d);assert.equal(d.colors.length,49);assert.equal(d.fills.length,20);
    assert.equal(d.colors[7].color,d.colors[8].color);assert.notEqual(d.colors[7].name,d.colors[8].name);
    for(const bad of [{}, {...d,version:2},{...d,colors:[]},{...d,colors:Array(257).fill(d.colors[0])},
        {...d,colors:[d.colors[0],d.colors[0]]},{...d,colors:[{name:'<bad>',color:'#000000'}]},
        {...d,colors:[{name:'safe',color:'url(https://wrong)'}]},
        {...d,fills:[{...d.fills[0],rows:[1]}]}, {...d,fills:[{...d.fills[0],fill:{kind:'pattern',rows:Array(16).fill(1)}}]}]) {assert.throws(()=>P.validate(bad));}
    const h=harness();assert.equal(h.reads.length,0);h.open();assert.equal(h.reads.length,1);await h.ready();
    assert.equal(h.edits.length,0);assert.equal(h.el('presets-colors').children.length,49);assert.equal(h.el('presets-fills').children.length,20);
    assert(h.el('presets-colors').children.every(b=>b.disabled));h.enabled=true;
    for(let i=0;i<49;i++){const b=h.el('presets-colors').children[i];assert(b.title.startsWith(d.colors[i].name));b.onclick();assert.deepEqual(h.edits.at(-1),{color:d.colors[i].color});}
    for(let i=0;i<20;i++){
        const b=h.el('presets-fills').children[i],canvas=b.children[0];assert.equal(canvas.width,16);assert.equal(canvas.height,16);
        const expected=[['#ffffff',0,0,16,16]];
        d.fills[i].rows.forEach((row,y)=>row.toString(2).padStart(16,'0').split('').forEach((bit,x)=>{if(bit==='1'){expected.push(['#000000',x,y,1,1]);}}));
        assert.deepEqual(canvas.calls,expected,'MSB-left preview '+d.fills[i].name);b.onclick();assert.deepEqual(h.edits.at(-1),{fill_slot:d.fills[i].name});
    }
    const count=h.edits.length;h.enabled=false;h.el('presets-colors').children[0].onclick();assert.equal(h.edits.length,count);
    h.change({rev:'2'});assert.equal(h.reads.length,2,'pan reread unchanged slots');
    h.available=false;h.available=true;assert.equal(h.reads.length,3);assert.equal(h.reads.filter(q=>q.path==='/api/v1/palette/presets').length,1,'immutable palette was fetched again');await h.reply();
    h.change({slotKey:'d'.repeat(40),rev:'3'});assert(h.el('presets-fills').hidden);const live=h.session();live.fills[15].rows=Array(16).fill(1);await h.reply(undefined,live);
    assert(!h.el('presets-fills').hidden);assert.equal(h.el('presets-fills').children[15].children[0].calls.length,17);
    assert(!h.el('fill-slot-open').disabled,'unused slot editor depends on selection');h.el('fill-slot-choice').value='brick';h.el('fill-slot-open').onclick();
    assert.equal(h.el('fill-slot-editor').hidden,false);assert.equal(h.edits.length,count);h.el('fill-slot-reset').onclick();h.el('fill-slot-editor').onsubmit({preventDefault(){}});
    assert.equal(h.slotEdits.length,1);assert.deepEqual(h.slotEdits[0].body,{name:'brick',rows:d.fills[15].rows});h.slotEdits[0].done(null);
    h.enabled=true;const fillButton=h.el('presets-fills').children[15];let prevented=false;fillButton.oncontextmenu({preventDefault(){prevented=true;}});assert(prevented);assert(!h.el('fill-slot-editor').hidden);assert.equal(h.edits.length,count);
    h.change({rev:'4'});assert(h.el('fill-slot-editor').hidden);assert.equal(h.reads.length,4,'revision alone reread slot table');
    h.change({fillEdit:false});assert(h.el('fill-slot-tools').hidden);h.el('fill-slot-open').onclick();assert(h.el('fill-slot-editor').hidden);
    h.ui.stop();assert(h.el('presets-colors').children.every(b=>b.disabled));
    const f=harness();f.open();await f.fail();assert.equal(f.el('presets-retry').hidden,false);f.ui.changed();assert.equal(f.reads.length,1);
    f.el('presets-retry').onclick();await f.ready();assert.equal(f.el('presets-retry').hidden,true);
    const stale=harness();stale.open();const q=stale.reads[0];stale.available=false;assert(q.aborted);await stale.reply(q);assert.equal(stale.el('presets-colors').children.length,0);
    stale.available=true;assert.equal(stale.reads.length,2);await stale.ready();assert.equal(stale.el('presets-fills').children.length,20);
    const malformed=harness();malformed.open();await malformed.reply(undefined,{...d,fills:[{name:'last_bad',rows:[]} ]});assert.equal(malformed.el('presets-colors').children.length,0,'partial palette was displayed');
    const badLive=harness();badLive.open();await badLive.reply();const bad=badLive.session();bad.fills[19].rows=Array(16).fill(1);await badLive.reply(undefined,bad);assert(badLive.el('presets-fills').hidden);assert(!badLive.el('presets-retry').hidden);const reads=badLive.reads.length;badLive.change({rev:'2'});assert.equal(badLive.reads.length,reads,'failed read looped');
    badLive.el('presets-retry').onclick();await badLive.reply();assert(!badLive.el('presets-fills').hidden);
    const replaced=harness();replaced.open();await replaced.reply();const old=replaced.reads.at(-1);replaced.change({epoch:'e'.repeat(64)});assert(old.aborted);await replaced.reply(old);assert(replaced.el('presets-fills').hidden);await replaced.reply();assert(!replaced.el('presets-fills').hidden);
    console.log('WEB PRESETS: ALL OK (49 colors, 20 live previews, reference writes, immutable/live caches, unused editor, retry/cancel/stale)');
}
main().catch(e=>{console.error(e);process.exitCode=1;});
