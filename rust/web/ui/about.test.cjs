'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),A=require('./about.js');
const bundle='a'.repeat(40),font=fs.readFileSync(__dirname+'/../../render-core/assets/NotoSansMono-OFL.txt','utf8');
const value={product:'floe2-web',bundle,build:{app_version:'0.1.0',source_revision:'unknown',target:'x86_64-unknown-linux-musl',index_compatibility:'0.12.87',renderd_compatibility:'0.12.87'},python_runtime:false,desktop_acceptance:'unverified',notice_scope:'embedded_font_only',font_name:'Noto Sans Mono',font_notice:font,
    notices:{status:'not_packaged',index_id:null,files:0,total_bytes:0,page_bytes:65536,list_size:64}};
assert.equal(A.parse(value,bundle),value);
assert.match(A.describe(value),/Expected renderd compatibility: 0.12.87/);
assert.match(A.describe({...value,build:null}),/unavailable/);
for(const patch of [{bundle:'old'},{build:{}},{build:{...value.build,target:'bad\npath'}},{font_notice:''},{python_runtime:true},{desktop_acceptance:'passed'},{notice_scope:'all'}]){
    assert.throws(()=>A.parse({...value,...patch},bundle));
}
const nodes=new Map(),listeners={};
const doc={activeElement:null,contains:n=>Array.from(nodes.values()).includes(n),addEventListener(k,fn,capture){assert.equal(capture,true);listeners[k]=fn;}};
class Node{
    constructor(id){this.id=id;this.attrs={};this.hidden=false;this.disabled=false;this.textContent='';this.events={};}
    focus(){doc.activeElement=this;}
    setAttribute(k,v){this.attrs[k]=v;}getAttribute(k){return this.attrs[k]===undefined?null:this.attrs[k];}removeAttribute(k){delete this.attrs[k];}
    addEventListener(k,fn){this.events[k]=fn;}
    contains(n){return ['about-dialog','about-close','about-build','about-font'].includes(n.id);}
    querySelectorAll(){return ['about-close','about-build','about-font'].map(el);}
    getClientRects(){return [{}];}
}
function el(id){if(!nodes.has(id)){nodes.set(id,new Node(id));}return nodes.get(id);}
let pending=[];
const ui=A.bind({el,document:doc,bundle,http(method,path,body,missing,token){
    assert.equal(method,'GET');assert.equal(path,'/api/v1/about');assert.equal(body,undefined);assert.equal(missing,false);
    return new Promise((resolve,reject)=>{token.abort=()=>{};pending.push({resolve,reject,token});});
}});
function key(key,shiftKey=false){const e={key,shiftKey,stopped:false,prevented:false,stopPropagation(){this.stopped=true;},preventDefault(){this.prevented=true;}};listeners.keydown(e);return e;}
async function run(){
    await el('about-open').onclick();assert.equal(pending.length,0);ui.init();assert.equal(el('about-open').disabled,false);
    el('about-open').focus();el('app-workspace').setAttribute('aria-hidden','false');
    let p=el('about-open').onclick();assert.equal(pending.length,1);assert.equal(doc.activeElement,el('about-close'));
    assert.equal(el('app-header').getAttribute('aria-hidden'),'true');assert.equal(key('ArrowUp').stopped,true);
    let e=key('Tab',true);assert.equal(e.prevented,true);assert.equal(doc.activeElement,el('about-font'));
    e=key('Tab');assert.equal(e.prevented,true);assert.equal(doc.activeElement,el('about-close'));
    listeners.focusin({target:el('viewport')});assert.equal(doc.activeElement,el('about-close'));
    pending.shift().resolve(value);await p;assert.equal(el('about-font').textContent,font);
    assert.match(el('about-build').textContent,/unverified/);
    e=key('Escape');assert.equal(e.prevented,true);assert.equal(el('about-dialog').hidden,true);assert.equal(doc.activeElement,el('about-open'));
    assert.equal(el('app-header').getAttribute('aria-hidden'),null);assert.equal(el('app-workspace').getAttribute('aria-hidden'),'false');
    assert.equal(key('ArrowUp').stopped,false);
    p=el('about-open').onclick();const old=pending.shift();el('about-close').onclick();assert.equal(old.token.cancelled,true);
    const newer=el('about-open').onclick(),next=pending.shift();old.resolve(value);await p;assert.equal(el('about-build').textContent,'');
    next.reject(new Error('offline'));await newer;assert.match(el('about-status').textContent,/Close and reopen/);
    el('about-dialog').events.click({target:el('about-dialog')});assert.equal(el('about-dialog').hidden,true);
    p=el('about-open').onclick();const end=pending.shift();ui.stop();assert.equal(end.token.cancelled,true);end.resolve(value);await p;
    assert.equal(el('about-open').disabled,true);assert.equal(el('about-dialog').hidden,true);assert.equal(el('about-build').textContent,'');
    await el('about-open').onclick();assert.equal(pending.length,0);
    console.log('About: schema, readonly GET, original font, stale cancellation, focus/Escape and stop OK');
}
run().catch(e=>{console.error(e);process.exitCode=1;});
