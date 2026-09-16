'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),UI=require('./guest-layers.js'),P=require('./protocol.js');
const tick=()=>new Promise(r=>setImmediate(r));
function environment(mode='explore'){
    const nodes=new Map(),calls=[],edits=[];let c={view_id:'view',epoch:'epoch',state_rev:'1',render_key:'1',mode,pending:false},hold=null,fault=false;
    class Element{constructor(){this.children=[];this.style={};this.disabled=false;this.checked=false;this.attrs={};}
        set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}set innerHTML(_){throw Error('unsafe HTML');}
        appendChild(v){this.children.push(v);}setAttribute(k,v){this.attrs[k]=v;}}
    for(const m of fs.readFileSync(__dirname+'/guest.html','utf8').matchAll(/id="([^"]+)"/g)){nodes.set(m[1],new Element());}
    function el(k){assert(nodes.has(k),k);return nodes.get(k);}
    const row=(pair,name,more={})=>({pair,name,aliases:[],color:'#ff0000',fill:{kind:'solid'},width:1,visible:true,head:false,synthetic:false,closed:false,children:0,parent:null,all_visible:false,mixed:false,...more});
    async function http(method,path,body,t){assert.equal(method,'POST');assert.equal(path,'/layers');calls.push(body);const camera={...c};
        if(hold){const h=hold;hold=null;await new Promise((resolve,reject)=>{h.resolve=resolve;h.reject=reject;t.abort=()=>reject(Error('cancelled'));});}
        else{await tick();}if(fault){fault=false;throw Error('reply lost');}
        const folded=body.body.fold.closed!==body.body.fold.exceptions.some(p=>p[0]===3&&p[1]===1);
        const rows=[row([3,1],'<script>AUTHORIZED NAME</script>',{head:true,children:1,closed:folded,all_visible:false,mixed:true})];
        if(!folded){rows.push(row([3,2],'child',{parent:[3,1],visible:false}));}
        rows.push(row([4,0],'LEVEL 4',{head:true,synthetic:true,all_visible:true}));
        return {view_id:camera.view_id,data:{state_rev:camera.state_rev,render_key:camera.render_key,start:0,total:rows.length,all_total:3,next:null,rows}};
    }
    const ui=UI.bind({el,document:{createElement:()=>new Element()},protocol:P,context:()=>c,http,edit(body,context){if(c.pending||context.state_rev!==c.state_rev){return false;}edits.push(body);c={...c,pending:true};ui.changed();return true;}});
    async function settle(){for(let i=0;i<20&&el('gl-next').disabled;i++){await tick();}await tick();}
    return {ui,el,calls,edits,settle,get:()=>c,set(v){c=v;ui.changed();},hold(){hold={};return hold;},fail(){fault=true;},box(n){return el('gl-rows').children[n].children.at(-1).children[0];}};
}
(async()=>{
    const e=environment();assert.equal(e.calls.length,0);e.ui.changed();await e.settle();assert.equal(e.calls.length,1);
    assert.equal(e.el('gl-rows').children.length,3);assert.match(e.el('gl-rows').children[0].children.at(-1).children[1].textContent,/<script>/);
    const name=n=>e.el('gl-rows').children[n].children.at(-1).children[1];
    e.ui.highlight([[3,1],[999,0]]);assert.equal(name(0).className,'guest-layer-picked');assert.equal(name(1).className,'');
    assert.equal(e.calls.length,1,'highlight only existing authorized rows, with no extra reads');assert.equal(e.el('gl-rows').children.length,3);
    e.set({...e.get(),state_rev:'2'});await tick();assert.equal(e.calls.length,1,'camera-only pan does not rescan palette');
    assert.equal(name(0).className,'guest-layer-picked','highlight survives local rerender');e.ui.highlight([]);assert.equal(name(0).className,'');
    e.el('gl-collapse').onclick();await e.settle();assert.equal(e.el('gl-rows').children.length,2);assert(e.box(0).indeterminate);
    e.box(0).checked=true;e.box(0).onchange();assert.deepEqual(e.edits[0],{layer_visibility:{pair:[3,1],group:true,visible:true}});
    assert(e.box(0).disabled);e.set({...e.get(),state_rev:'3',render_key:'2',pending:false});await e.settle();assert.equal(e.edits.length,1);
    e.el('gl-expand').onclick();await e.settle();e.box(0).checked=false;e.box(0).onchange();assert.deepEqual(e.edits[1],{layer_visibility:{pair:[3,1],group:false,visible:false}});
    e.set({...e.get(),pending:false});await e.settle();assert.equal(e.edits.length,2,'rejected/no-op edit is not replayed');
    e.el('gl-all').onclick();assert.deepEqual(e.edits[2],{layers:{mode:'all'}});e.set({...e.get(),pending:false});await e.settle();
    const delay=e.hold();e.el('gl-reload').onclick();e.set({...e.get(),state_rev:'4'});const conflict=Error('HTTP 409');conflict.status=409;delay.reject(conflict);await e.settle();assert.equal(e.calls.at(-1).state_rev,'4');assert.equal(e.edits.length,3);
    e.fail();e.el('gl-reload').onclick();await tick();await tick();assert.match(e.el('gl-status').textContent,/unavailable/);assert(e.el('gl-all').disabled);
    e.el('gl-reload').onclick();await e.settle();assert.equal(e.edits.length,3);
    const stale=e.hold();e.el('gl-reload').onclick();e.set(null);stale.resolve();await tick();assert(e.el('gl-panel').hidden);assert.equal(e.el('gl-rows').children.length,0);
    const f=environment('follow');f.ui.changed();await f.settle();assert(f.el('gl-all').disabled);assert(f.box(0).disabled);f.el('gl-all').onclick();f.box(0).onchange();assert.equal(f.edits.length,0);f.el('gl-collapse').onclick();await f.settle();assert.equal(f.el('gl-rows').children.length,2);
    console.log('WEB GUEST LAYERS: ALL OK (scoped-only HTTP, plain labels, private folds, group/leaf edits, no pan reread, follow denial, stale/revoke, no mutation replay)');
})().catch(e=>{console.error(e);process.exitCode=1;});
