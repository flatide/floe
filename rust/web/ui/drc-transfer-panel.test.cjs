'use strict';
// Actual DRC + both editors + transfer controller, including reader revision
// handoff. Only transport/filesystem are synthetic; no selection is required.
const assert=require('node:assert/strict'),fs=require('node:fs'),D=require('./drc.js'),N=require('./drc-notes.js'),W=require('./drc-waives.js'),T=require('./drc-transfer.js'),P=require('./protocol.js');
const FN=require('./drc-notes.test.cjs'),FW=require('./drc-waives.test.cjs'),FT=require('./drc-transfer.test.cjs'),clone=v=>JSON.parse(JSON.stringify(v)),C=clone(FN.context);
const ids=new Set([...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
const nodes=new Map(),calls=[],timers=new Map(),saved=new Map(),transfers={notes:FT.catalog(),waives:FT.catalog()};let serial=0,waiveController;
class Element {constructor(){this.children=[];this.style={};this.value='';this.files=[];this.checked=false;this.hidden=false;this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}set innerHTML(_){throw Error('HTML injection');}
    appendChild(v){this.children.push(v);return v;}setAttribute(k,v){this[k]=v;}focus(){}scrollIntoView(){}getContext(){return new Proxy({}, {get:()=>()=>{}});}}
const el=id=>{assert(ids.has(id),id);if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
el('drc-waived').value='all';el('drc-markers').checked=true;
const view={id:C.view_id,source:'source',connected:true,pending:false,state:{connection_epoch:'8'.repeat(64),state_rev:'1',status:'idle',layers_isolated:false,bbox_dbu:['0','0','10','10'],pixels:[200,200],dbu_um:'1'}};
const cat={drc:{id:C.drc_id,revision:C.revision,title:'synthetic.ice',phase:'ready',source_id:'source',metadata:{checks:'1',errors:'4',format:'ice'}},notes:FN.catalog(),waives:FW.catalog()};
async function http(method,path,body){
    calls.push({method,path,body});if(path==='/api/v1/drc')return clone(cat);
    const match=path.match(/^\/api\/v1\/drc\/review\/(notes|waives)(.*)$/);
    if(match){const k=match[1],suffix=match[2],m=transfers[k];
        if(!suffix){if(method==='GET')return clone(cat[k]);assert(body.approve&&body.confirm_legacy);assert.equal(body.seq,P.next(cat[k].operations.last_seq));
            if(k==='waives')assert(waiveController.suspended(),'whole import did not pause reads before saving');
            const op=(k==='notes'?FN:FW).op(body.seq,'succeeded',{context:clone(body.context),review_rev:P.next(cat[k].review_rev)});cat[k]=(k==='notes'?FN:FW).catalog([op]);
            if(k==='waives')cat.drc.revision=op.reader_revision;return clone(op);
        }
        if(suffix==='/revoke'){m.upload=null;return null;}
        if(suffix==='/transfer'){
            if(method==='GET')return clone(m);
            assert.equal(body.seq,P.next(m.operations.last_seq));const op={kind:'drc_review_transfer',seq:body.seq,phase:'succeeded',context:clone(body.context),action:body.action};
            if(body.action==='import'){m.upload={token:'e'.repeat(64),context:clone(body.context),bytes:body.bytes,received:'0'};op.upload={token:m.upload.token,bytes:body.bytes,received:'0',expires_in_ms:'600000'};}
            else if(body.action==='prepare'){assert.equal(m.upload.received,m.upload.bytes);m.upload=null;op.preview=FT.preview(k);op.preview.reviewer=cat[k].reviewer;op.preview.context=clone(body.context);op.preview.review_rev=cat[k].review_rev;}
            else throw Error('Unexpected transfer action');
            m.operations={last_seq:op.seq,active:null,history:m.operations.history.concat([op])};return clone(op);
        }
        throw Error('Unexpected review request '+suffix);
    }
    if(path.endsWith('/selection'))return {revision:cat.drc.revision,view_id:view.id,state:{selection_rev:'1',total:'0',limit:5000,rules:[]}};
    if(body&&body.body&&body.body.kind==='rules')return {rows:[],next:null};
    throw Error('Unexpected request '+method+' '+path);
}
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},window:{requestAnimationFrame:fn=>{timers.set(++serial,{fn});return serial;},cancelAnimationFrame:id=>timers.delete(id)},
    protocol:P,rulers:require('./rulers.js'),groups:require('./drc-groups.js'),notes:N,waives:Object.assign({},W,{bind(o){waiveController=W.bind(o);return waiveController;}}),transfers:T,http,context:()=>view,
    transferChunk:async(k,req,offset,blob)=>{const m=transfers[k];assert.equal(offset,Number(m.upload.received));assert(blob.size<=1048576);m.upload.received=String(offset+blob.size);
        const op={kind:'drc_review_transfer',seq:req.seq,phase:'succeeded',context:clone(req.context),action:'chunk',upload:{token:m.upload.token,bytes:m.upload.bytes,received:m.upload.received,expires_in_ms:'600000'}};
        m.operations={last_seq:op.seq,active:null,history:m.operations.history.concat([op])};return clone(op);},transferDownload(){throw Error('No download requested');},
    session:()=>'f'.repeat(64),loadNotePending:()=>saved.get('notes')||null,saveNotePending:v=>saved.set('notes',v),loadWaivePending:()=>saved.get('waives')||null,saveWaivePending:v=>saved.set('waives',v),
    now:()=>100,setTimeout:(fn,ms)=>{timers.set(++serial,{fn,ms});return serial;},clearTimeout:id=>timers.delete(id),
    navigate(){throw Error('import moved layout');},restoreLayers(){throw Error('import changed layers');},resize(){},stateStore:{bind:o=>({attach:()=>o.apply(null),change(){},close(){}})}});
const flush=async()=>{for(let i=0;i<200;i++)await Promise.resolve();};
let completed=false;
process.once('beforeExit',()=>{if(!completed){console.error('Transfer panel test exited with unresolved async work');process.exitCode=1;}});
(async()=>{
    await panel.init();await flush();assert(el('notes-read').disabled,'fixture must have no selection');assert(el('waives-read').disabled);assert(!el('transfer-panel').hidden);
    const before=clone(view);
    for(const k of ['notes','waives']){
        if(k==='waives'){el('transfer-kind').value=k;await el('transfer-kind').onchange();}
        el('transfer-file').files=[{size:17,slice(a,b){return {size:b-a};}}];el('transfer-file').onchange();assert(!el('transfer-import').disabled,k);
        await el('transfer-import').onclick();await flush();assert(!el('transfer-review').hidden,k);assert.equal(cat[k].operations.history.length,0);
        el('transfer-run').checked=el('transfer-consent').checked=true;el('transfer-consent').onchange();assert(!el('transfer-approve').disabled,k);
        await el('transfer-approve').onclick();await flush();assert.equal(cat[k].operations.history.length,1,k);assert.equal(saved.get(k),null,k);
        assert(el('transfer-review').hidden);assert(!el(k+'-status').textContent.includes('UNKNOWN'));
    }
    assert(!waiveController.suspended(),'matching new reader revision did not resume queries');assert.notEqual(cat.drc.revision,C.revision);
    assert.deepEqual(view,before,'whole review import mutated the layout view');panel.stop(true);assert.equal(timers.size,0);
    console.log('WEB DRC TRANSFER PANEL: ALL OK (real DRC/editors, no selection, consent, native save delegation, waive pause/revision resume, no layout mutation)');
})().then(()=>{completed=true;},e=>{completed=true;console.error(e);process.exitCode=1;});
