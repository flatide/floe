'use strict';
const assert=require('node:assert/strict'),Sharing=require('./sharing.js');
const id='a'.repeat(64),invite='b'.repeat(64),requests=[],nodes=new Map();let current={view_id:'c'.repeat(64),state_rev:'1',source_id:'source'},fail=false,entries=[],catalog=null;
class Element{
    constructor(){this.attrs={};this.children=[];this.listeners={};this.value='';this.checked=false;this.hidden=false;this.disabled=false;this.textContent='';}
    setAttribute(k,v){this.attrs[k]=v;}getAttribute(k){return this.attrs[k]||null;}removeAttribute(k){delete this.attrs[k];}
    appendChild(c){this.children.push(c);}contains(n){return this===n||this.children.some(c=>c.contains(n));}
    addEventListener(k,f){this.listeners[k]=f;}focus(){doc.activeElement=this;}querySelectorAll(){return [];}
}
const el=k=>{if(!nodes.has(k)){nodes.set(k,new Element());}return nodes.get(k);};
const doc={activeElement:null,contains:()=>true,addEventListener(){},createElement:()=>new Element()};
async function http(method,path,body){requests.push({method,path,body});if(path==='/api/v1/drc'){return {drc:catalog};}if(method==='GET'){return {shares:entries};}if(method==='DELETE'){entries=[];return null;}
    entries=[{share_id:id,mode:body.mode,...body.drc&&{drc:body.drc}}];if(fail){throw Error('timeout');}return {share_id:id,invite,mode:body.mode,invite_seconds:120,session_seconds:1800,...body.drc&&{drc:{id:body.drc.id,revision:body.drc.revision}}};}
const c=Sharing.bind({el,document:doc,http,origin:'http://127.0.0.1:1234',context:()=>current});
const tick=()=>new Promise(r=>setImmediate(r));
(async()=>{
    c.init(false);el('share-open').onclick();assert.equal(requests.length,0);assert(el('share-open').hidden);
    c.init(true);el('share-open').onclick();await tick();assert.equal(requests.length,2);assert.equal(requests[0].method,'GET');assert.equal(el('share-create').disabled,true);
    await el('share-create').onclick();assert.equal(requests.length,2);el('share-consent').checked=true;el('share-consent').onchange();
    current={...current,state_rev:'2'};c.changed();await el('share-create').onclick();assert.equal(requests.length,2);assert(!el('share-consent').checked);
    el('share-cancel').onclick();el('share-open').onclick();await tick();el('share-mode').value='explore';el('share-consent').checked=true;el('share-consent').onchange();await el('share-create').onclick();await tick();
    const post=requests.find(r=>r.method==='POST');assert.deepEqual(post.body,{view_id:current.view_id,base_state_rev:'2',mode:'explore',approve:true});assert(!('drc' in post.body));
    assert.equal(el('share-link').value,'http://127.0.0.1:1234/guest/'+id+'#invite='+invite);assert.equal(el('share-result').hidden,false);assert(!el('share-consent').checked);
    el('share-cancel').onclick();assert.equal(el('share-link').value,'');assert(!el('share-visit').attrs.href);
    el('share-open').onclick();await tick();const row=el('share-list').children.at(-1);await row.children[1].onclick();await tick();assert(requests.some(r=>r.method==='DELETE'&&r.path==='/api/v1/shares/'+id));
    fail=true;el('share-consent').checked=true;el('share-consent').onchange();await el('share-create').onclick();await tick();assert.equal(requests.filter(r=>r.method==='POST').length,2);assert.match(el('share-status').textContent,/unknown/);assert.equal(el('share-link').value,'');
    fail=false;catalog={id:'d'.repeat(64),revision:'rev-1',source_id:'source',phase:'ready',title:'<script>plain title</script>',metadata:{checks:'2',errors:'3'}};
    el('share-cancel').onclick();el('share-open').onclick();await tick();assert.equal(el('share-drc').hidden,false);assert(!el('share-drc-consent').checked);
    assert.match(el('share-drc-title').textContent,/<script>plain title<\/script>/);
    el('share-consent').checked=true;await el('share-create').onclick();await tick();assert(!('drc' in requests.filter(r=>r.method==='POST').at(-1).body),'layout consent is not DRC consent');
    el('share-consent').checked=true;el('share-drc-consent').checked=true;await el('share-create').onclick();await tick();
    assert.deepEqual(requests.filter(r=>r.method==='POST').at(-1).body.drc,{id:catalog.id,revision:'rev-1',approve:true});assert(!el('share-drc-consent').checked);
    el('share-cancel').onclick();catalog.source_id='another-source';el('share-open').onclick();await tick();assert.equal(el('share-drc').hidden,true);assert(el('share-drc-consent').disabled);
    c.suspend();assert(el('share-dialog').hidden);c.stop();assert(el('share-open').hidden);
    console.log('WEB SHARE OWNER UI: ALL OK (default off, separate whole-DRC consent/source/revision, plain titles, stale context, private fragment link, revoke, uncertain outcome/no retry, lifecycle)');
})().catch(e=>{console.error(e);process.exitCode=1;});
