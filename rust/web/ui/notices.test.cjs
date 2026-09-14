'use strict';
const assert=require('node:assert/strict'),N=require('./notices.js');
const m={status:'available',index_id:'a'.repeat(40),files:70,total_bytes:65609,page_bytes:65536,list_size:64};
const f={id:0,name:'NOTICES/<script>.html',bytes:65540,pages:2,encoding:'utf8'};
function list(start){const files=[];for(let id=start;id<Math.min(start+64,70);id++){files.push(id===0?f:{id,name:'NOTICES/'+id,bytes:1,pages:1,encoding:'utf8'});}return {index_id:m.index_id,start,next:start===0?64:null,total:70,files};}
function chunk(page){return {index_id:m.index_id,file:f,page,offset:page?65536:0,bytes:page?4:65536,text:page?'<b/>':'a'.repeat(65536)};}
assert.equal(N.metadata(m),m);N.listing(list(0),m,0);N.chunk(chunk(1),m,f,1);
for(const v of [{...m,index_id:null},{...m,files:4097},{...m,status:'unknown'},{...m,page_bytes:999}]){assert.throws(()=>N.metadata(v));}
assert.throws(()=>N.listing({...list(0),next:null},m,0));assert.throws(()=>N.listing({...list(0),index_id:'b'.repeat(40)},m,0));
assert.throws(()=>N.chunk({...chunk(1),text:'truncated'},m,f,1));assert.throws(()=>N.chunk({...chunk(1),file:{...f,name:'NOTICES/../secret'}},m,f,1));
const binary={id:1,name:'NOTICES/binary',bytes:2,pages:1,encoding:'hex'};
N.chunk({index_id:m.index_id,file:binary,page:0,offset:0,bytes:2,text:'ff 00 '},m,binary,0);
assert.throws(()=>N.chunk({index_id:m.index_id,file:binary,page:0,offset:0,bytes:2,text:'zz 00 '},m,binary,0));
const nodes=new Map(),requests=[];
class Element{constructor(){this.children=[];this.hidden=false;this.disabled=false;this.value='';}appendChild(c){this.children.push(c);}set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}}
function el(id){if(!nodes.has(id)){nodes.set(id,new Element());}return nodes.get(id);}
const ui=N.bind({el,document:{createElement:()=>new Element()},http(method,path,body,missing,token){
    assert.equal(method,'GET');assert.match(path,/^\/api\/v1\/about\/notices\/[0-9]+(?:\/[0-9]+)?$/);assert.equal(body,undefined);assert.equal(missing,false);
    return new Promise((resolve,reject)=>{token.abort=()=>{};requests.push({path,token,resolve,reject});});
}});
const tick=()=>new Promise(resolve=>setImmediate(resolve));
async function run(){
    ui.open({status:'not_packaged',index_id:null,files:0,total_bytes:0,page_bytes:65536,list_size:64});assert.equal(requests.length,0);assert.equal(el('notice-catalog').hidden,true);
    ui.open(m);let r=requests.shift();assert.equal(r.path,'/api/v1/about/notices/0');r.resolve(list(0));await tick();
    assert.equal(el('notice-files').children.length,64);assert.match(el('notice-files').children[0].textContent,/<script>/);
    el('notice-files').children[0].onclick();r=requests.shift();assert.equal(r.path,'/api/v1/about/notices/0/0');r.resolve(chunk(0));await tick();assert.equal(el('notice-text').textContent.length,65536);
    el('notice-next').onclick();r=requests.shift();assert.equal(el('notice-text').textContent,'');r.resolve(chunk(1));await tick();assert.equal(el('notice-text').textContent,'<b/>');
    assert.equal(el('notice-next').disabled,true);el('notice-jump').value='01';el('notice-go').onclick();assert.equal(requests.length,0);
    el('notice-jump').value='1';el('notice-go').onclick();const old=requests.shift();ui.close();assert.equal(old.token.cancelled,true);
    ui.open(m);r=requests.shift();old.resolve(chunk(0));r.resolve(list(0));await tick();assert.equal(el('notice-text').textContent,'');
    el('notice-files').children[0].onclick();r=requests.shift();r.reject(new Error('changed'));await tick();assert.match(el('notice-page-status').textContent,/No partial/);assert.equal(requests.length,0);
    el('notice-retry').onclick();r=requests.shift();r.resolve(chunk(0));await tick();assert.equal(el('notice-text').textContent.length,65536);
    el('notice-list-next').onclick();r=requests.shift();assert.equal(el('notice-text').textContent,'');r.resolve(list(64));await tick();assert.equal(el('notice-files').children.length,6);
    el('notice-list-prev').onclick();r=requests.shift();ui.close();r.resolve(list(0));await tick();assert.equal(el('notice-files').children.length,0);assert.equal(el('notice-catalog').hidden,true);
    assert.equal(requests.length,0);console.log('NOTICES UI: ALL OK (bounded list/chunks, exact UTF8/hex, text-only HTML, invalid/stale/failed/closed reads, navigation and no writes)');
}
run().catch(e=>{console.error(e);process.exitCode=1;});
