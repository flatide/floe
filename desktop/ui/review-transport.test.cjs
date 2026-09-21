'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const script=fs.readFileSync(__dirname+'/review-transport.js','utf8');
const key='floe.desktop.review.qa';
function page(storage=new Map()){
    class XHR {
        open(method,url){this.method=method;this.url=url;}
        send(body){this.body=body;}
        complete(status){this.status=status;this.onload({type:'load'});}
    }
    const window={XMLHttpRequest:XHR};
    vm.runInNewContext(script,{window,URL,WeakMap,Event:class Event{constructor(type){this.type=type;}},
        location:new URL('http://127.0.0.1:12345/'),sessionStorage:{
            getItem(k){assert.equal(k,key);return storage.get(k)||null;},
            setItem(k,v){assert.equal(k,key);storage.set(k,v);}}});
    function send(path,method='POST'){
        const x=new XHR(),seen=[];
        x.open(method,path);x.onload=()=>seen.push('load');x.onerror=()=>seen.push('error');
        const body=new Proxy({}, {get(){throw Error('Body must not be inspected');}});
        x.send(body);assert.equal(x.body,body);return {x,seen};
    }
    return {window,send,storage,wire:window.__floeReviewTransport};
}
const first=page();first.wire.arm('notes');
const lost=first.send('/api/v1/drc/review/notes');lost.x.complete(200);
assert.deepEqual(lost.seen,['error']);assert.equal(first.wire.counts().notes,1);
assert.equal(first.wire.counts().dropped,'notes');assert(!first.wire.counts().failed);
// A new document sees the prior count before any app script can replay a save.
const again=page(first.storage);assert.equal(again.wire.counts().notes,1);
const resolved=again.send('/api/v1/drc/review/notes');resolved.x.complete(200);
assert.deepEqual(resolved.seen,['load']);assert.equal(again.wire.counts().notes,2);
assert.throws(()=>again.wire.arm('notes'));
assert.throws(()=>again.send('/api/v1/drc/review/notes'));assert(again.wire.counts().failed);
for(const path of ['https://outside.invalid/api/v1/drc/review/notes','/api/v1/drc/review/notes/read','/api/v1/bootstrap']){
    const h=page(),r=h.send(path);r.x.complete(200);assert.deepEqual(r.seen,['load']);assert.equal(h.wire.counts().notes,0);
}
const read=page(),r=read.send('/api/v1/drc/review/notes','GET');r.x.complete(200);assert.equal(read.wire.counts().notes,0);
for(const status of [400,403,409,500]){
    const h=page();h.wire.arm('waives');const r=h.send('/api/v1/drc/review/waives');r.x.complete(status);
    assert.deepEqual(r.seen,['load']);assert(h.wire.counts().failed,'native errors must not become a passing lost ACK');
}
for(const raw of ['broken','{}','{"notes":0,"waives":0,"drop":"","dropped":"","failed":false,"secret":1}']){
    assert(page(new Map([[key,raw]])).wire.counts().failed);
}
const failing=vm.runInNewContext('('+fs.readFileSync(__dirname+'/review-probe.js','utf8')+')',{
    window:{__floeReviewTransport:{counts:()=>({failed:true})}},document:{getElementById:()=>({disabled:false})},location:{hash:''}});
assert.equal(failing('note-start'),'review-failed');
const early=vm.runInNewContext('('+fs.readFileSync(__dirname+'/review-probe.js','utf8')+')',{
    window:{},document:{getElementById:()=>null},location:{hash:''}});
assert.equal(early('note-start'),'wait','about:blank before the document-start script is not a QA failure');
// Read-back must release the note snapshot before requesting a waive snapshot.
const nodes=new Map(),actions=[];
function el(id){if(!nodes.has(id))nodes.set(id,{textContent:'',value:'',hidden:false,disabled:false,
    click(){actions.push(id);}});return nodes.get(id);}
el('notes-owner').textContent=el('waives-owner').textContent='Reviewer: native-recovery-test';
el('notes-editor').hidden=el('waives-editor').hidden=true;
el('drc-errors').children=[{getAttribute:()=> 'Error 1, global 1',click:()=>actions.push('select')},{}];
el('notes-read').click=()=>{actions.push('notes-read');el('notes-editor').hidden=false;el('notes-text').value='Synthetic native recovery — 한글';};
el('notes-discard').click=()=>{actions.push('notes-discard');el('notes-editor').hidden=true;el('transfer-export').disabled=true;};
el('waives-read').click=()=>{actions.push('waives-read');el('waives-editor').hidden=false;el('waives-target').textContent='1 already waived · 0 reserved statuses';};
el('waives-discard').click=()=>{actions.push('waives-discard');el('waives-editor').hidden=true;};
const probe=vm.runInNewContext('('+fs.readFileSync(__dirname+'/review-probe.js','utf8')+')',{
    window:{__floeReviewTransport:{counts:()=>({notes:2,waives:2,failed:false})}},
    document:{getElementById:el},location:{hash:''}});
for(let i=0;i<3;i++)assert.equal(probe('read-back'),'wait');
assert.equal(probe('read-back'),'review-wait-read');assert.deepEqual(actions,['select','notes-read','notes-discard']);
el('transfer-export').disabled=false;
assert.equal(probe('read-back'),'wait');assert.equal(probe('read-back'),'wait');assert.equal(probe('read-back'),'review-ok');
assert.deepEqual(actions,['select','notes-read','notes-discard','waives-read','waives-discard']);
console.log('DESKTOP REVIEW TRANSPORT: OK (narrow ACK loss; startup counters; no body/auth inspection; error fail-closed)');
