'use strict';
const assert=require('node:assert/strict'), api=require('./settings.js');
const nodes=new Map(),requests=[],readers=[],edits=[],downloads=[],urls=new Set(),timers=new Map();
let c={id:'a'.repeat(64),epoch:'b'.repeat(64),rev:'9007199254740993',ready:true,idle:true};
const el=id=>{if(!nodes.has(id)){nodes.set(id,{value:'',textContent:'',files:[],click(){}});}return nodes.get(id);};
class Reader {constructor(){readers.push(this);}readAsArrayBuffer(file){this.file=file;}abort(){this.aborted=true;if(this.onabort){this.onabort();}}ok(text){this.result=typeof text==='string'?new TextEncoder().encode(text).buffer:text;this.onload();}}
class XHR {constructor(){requests.push(this);this.headers={};}open(method,path){this.method=method;this.path=path;}setRequestHeader(k,v){this.headers[k]=v;}getResponseHeader(){return this.mime;}send(text){this.body=text;}abort(){this.aborted=true;this.onabort();}reply(text,status=200,mime=this.method==='POST'?'application/json':'text/plain; charset=utf-8'){this.status=status;this.mime=mime;this.responseText=typeof text==='string'?text:JSON.stringify(text);this.onload();}}
const document={body:{children:[],appendChild(n){this.children.push(n);},removeChild(n){this.children.splice(this.children.indexOf(n),1);}},createElement(tag){assert.equal(tag,'a');return {click(){downloads.push({href:this.href,name:this.download});}};}};
let tid=0;
const panel=api.bind({el,window:{FileReader:Reader,URL:{createObjectURL(blob){const url='blob:'+tid++;urls.add(url);assert(blob.size<=4194304);return url;},revokeObjectURL(url){urls.delete(url);}}},document,XHR,Blob,Encoder:TextEncoder,Decoder:TextDecoder,csrf:()=> 'csrf',context:()=>({...c}),message:s=>s,
    edit:(body,done)=>{const e={body,done};edits.push(e);return ()=>{e.cancelled=true;done('cancelled');};},setTimeout:(f)=>{timers.set(++tid,f);return tid;},clearTimeout:id=>timers.delete(id)});
const tick=()=>new Promise(setImmediate), message=()=>el('settings-status').textContent;
function choose(size=12){el('settings-load').onclick();el('settings-file').files=[{size}];el('settings-file').onchange();}
function prepared(extra={}){return {view_id:c.id,state_rev:c.rev,prepared_token:'d'.repeat(64),rows:1,malformed:0,...extra};}
(async()=>{
    panel.capabilities(false);assert(el('settings-panel').hidden);panel.capabilities(true);assert(!el('settings-load').disabled);
    choose(4194305);assert.equal(readers.length,0);assert.match(message(),/4 MiB/);
    choose();readers.at(-1).ok(new Uint8Array([0xff]).buffer);await tick();assert.equal(requests.length,0);assert(el('settings-cancel').disabled);
    choose();readers.at(-1).ok('7 red solid MASK 0 3');await tick();const req=requests.at(-1);assert(req.path.endsWith('/9007199254740993/calibre'));
    assert.equal(req.headers['X-Floe-CSRF'],'csrf');assert.equal(req.headers['Content-Type'],'text/plain; charset=utf-8');assert.equal(req.body,'7 red solid MASK 0 3');assert.equal(edits.length,0);
    req.reply(prepared({malformed:2}));await tick();assert.deepEqual(edits.at(-1).body,{prepared_token:'d'.repeat(64)});
    c.rev='9007199254740994';panel.changed();assert(!el('settings-cancel').disabled,'own apply was cancelled by the new snapshot');edits.at(-1).done(null);assert.match(message(),/applied.*2 malformed/);
    choose();const reader=readers.at(-1);c.rev='9007199254740995';panel.changed();assert(reader.aborted);reader.ok('1 blue solid');await tick();assert.equal(requests.length,1);
    choose();readers.at(-1).ok('{"format":"floe.layers"}');await tick();assert(requests.at(-1).path.endsWith('/native'));const old=requests.at(-1);c.epoch='e'.repeat(64);panel.changed();assert(old.aborted);old.reply(prepared());await tick();assert.equal(edits.length,1);
    choose();readers.at(-1).ok('1 red solid');await tick();requests.at(-1).reply(prepared({state_rev:'2'}));await tick();assert.match(message(),/Invalid prepared/);assert.equal(edits.length,1);
    choose();readers.at(-1).ok('1 red solid');await tick();requests.at(-1).ontimeout();await tick();assert.match(message(),/timed out/);const count=requests.length;await tick();assert.equal(requests.length,count);
    choose();readers.at(-1).ok('1 red solid');await tick();requests.at(-1).reply(prepared());await tick();el('settings-cancel').onclick();assert(edits.at(-1).cancelled);assert.match(message(),/may already/);edits.at(-1).done(null);assert.match(message(),/may already/);
    el('settings-format').value='native';el('settings-save').onclick();requests.at(-1).reply('{"format":"floe.layers"}');await tick();assert.equal(downloads.at(-1).name,'floe-layers.json');assert.equal(document.body.children.length,0);assert.equal(urls.size,1);
    el('settings-format').value='calibre';el('settings-save').onclick();requests.at(-1).reply({error:'unsupported'},400,'application/json');await tick();assert.match(message(),/Native JSON/);assert.equal(downloads.length,1);
    el('settings-save').onclick();requests.at(-1).reply('x'.repeat(4194305));await tick();assert.match(message(),/4 MiB/);assert.equal(downloads.length,1);
    el('settings-save').onclick();const saved=requests.at(-1);c.rev='9007199254740996';panel.changed();saved.reply('1 red solid');await tick();assert.equal(downloads.length,1);
    choose();panel.stop();assert.equal(urls.size,0);assert.equal(timers.size,0);assert(el('settings-load').disabled);panel.resume();assert(!el('settings-load').disabled);
    c.idle=false;panel.changed();assert(el('settings-save').disabled);panel.stop();
    console.log('WEB SETTINGS: ALL OK (chosen file, UTF8/limits, bound preparation/apply, no replay, custom bitmap refusal, downloads, cancellation/lifetime)');
})().catch(e=>{console.error(e);process.exitCode=1;});
