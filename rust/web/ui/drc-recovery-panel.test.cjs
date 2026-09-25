'use strict';
// Real DRC controller and recovery module with an initially failed reader.
const assert=require('node:assert/strict'),fs=require('node:fs');
const D=require('./drc.js'),R=require('./drc-recovery.js'),P=require('./protocol.js');
const clone=v=>JSON.parse(JSON.stringify(v)),id=c=>c.repeat(64),nodes=new Map(),calls=[],timers=new Map(),raf=new Map();let serial=0;
const ids=new Set([...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
class Element{constructor(){this.children=[];this.style={};this.value='';this.checked=false;this.hidden=false;this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}set innerHTML(_){throw Error('HTML injection');}
    appendChild(v){this.children.push(v);return v;}setAttribute(k,v){this[k]=v;}focus(){}scrollIntoView(){}getContext(){return new Proxy({},{get:()=>()=>{}});}}
const el=n=>{assert(ids.has(n),n);if(!nodes.has(n))nodes.set(n,new Element());return nodes.get(n);};
const c={drc_id:id('a'),revision:id('b'),view_id:id('c')};
const view={id:c.view_id,source:'source',connected:true,pending:false,state:{connection_epoch:id('8'),state_rev:'1',status:'idle',layers_isolated:false,bbox_dbu:['0','0','10','10'],pixels:[200,200],dbu_um:'1'}};
const cat={drc:{id:c.drc_id,revision:c.revision,source_id:'source',title:'synthetic.tray',phase:'error',metadata:null,error:'drc_changed_or_corrupt'},review_grant:{available:false,reviewer:'synthetic',notes_editable:true,waives_editable:true},
    notes:{...require('./drc-notes.test.cjs').catalog(),detached:false,reviewer:'synthetic',binding_id:id('d')}};
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},window:{requestAnimationFrame:fn=>{raf.set(++serial,fn);return serial;},cancelAnimationFrame:n=>raf.delete(n)},
    protocol:P,rulers:require('./rulers.js'),groups:require('./drc-groups.js'),notes:require('./drc-notes.js'),loadNotePending:()=>null,saveNotePending:()=>{},recovery:R,context:()=>view,session:()=>id('f'),loadRecoveryPending:()=>null,saveRecoveryPending:()=>{},now:()=>100,
    setTimeout:(fn,ms)=>{timers.set(++serial,{fn,ms});return serial;},clearTimeout:n=>timers.delete(n),navigate(){throw Error('unexpected navigation');},restoreLayers(){},resize(){},
    stateStore:{bind:()=>({attach:async()=>{},change(){},close(){}})},http:async(method,path,body)=>{calls.push({method,path,body});
        if(path==='/api/v1/drc')return clone(cat);
        if(path.endsWith('/recovery')&&method==='GET')return {kind:'review_recovery',review_kind:'drc_note',available:true,reviewer:'synthetic',binding_id:id('d'),operations:{last_seq:'0',active:null,history:[]}};
        if(path.endsWith('/recovery/prepare'))return {kind:'review_recovery',review_kind:'drc_note',phase:'prepared',context:clone(c),token:id('e'),name:'.synthetic.notes.fe',bytes:'13',scope:'exact_staging_link_only',reviewer:'synthetic',review_rev:'0',expires_in_ms:'30000'};
        throw Error('Unexpected request: '+path);}});
async function tick(){for(let i=0;i<80;i++)await Promise.resolve();}
(async()=>{try{
    await panel.init();await tick();assert(!el('recovery-panel').hidden);assert(!el('recovery-prepare').disabled);
    await el('recovery-prepare').onclick();assert.deepEqual(calls.at(-1).body,{context:c});assert(!el('recovery-preview').hidden);
    assert(el('recovery-approve').disabled);assert.equal(calls.filter(r=>r.path.endsWith('/recovery')&&r.method==='POST').length,0);
    view.connected=false;panel.contextChanged();assert(el('recovery-preview').hidden);assert(el('recovery-prepare').disabled);
    view.connected=true;panel.contextChanged();await panel.refresh();await tick();assert(!el('recovery-prepare').disabled);
    cat.notes.detached=true;cat.notes.available=false;cat.review_grant.available=true;await panel.refresh();assert(el('recovery-panel').hidden);
    console.log('WEB REVIEW RECOVERY PANEL: ALL OK (failed metadata entry, trusted registration, no ready-reader dependency, disconnect/grant fencing)');
}finally{panel.stop();assert.equal(timers.size,0);assert.equal(raf.size,0);}})().catch(e=>{console.error(e);process.exitCode=1;});
