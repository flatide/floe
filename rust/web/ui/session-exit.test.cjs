'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),Exit=require('./session-exit.js');
const html=fs.readFileSync(__dirname+'/index.html','utf8');
const ids=new Set([...html.matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
assert.match(html,/id="session-exit-dialog"[^>]*role="dialog"[^>]*aria-modal="true"/);
assert.match(html,/Earlier approved file writes are not undone/);
const nodes=new Map(),listeners={};
const doc={activeElement:null,contains:n=>[...nodes.values()].includes(n),
    defaultView:{addEventListener(k,f,capture){assert(capture);listeners[k]=f;}},
    addEventListener(){throw Error('exit must precede document modal traps');}};
class Node {
    constructor(id){this.id=id;this.attrs={};this.hidden=false;this.disabled=false;this.events={};}
    focus(){if(!this.disabled){doc.activeElement=this;}}
    setAttribute(k,v){this.attrs[k]=v;}
    getAttribute(k){return this.attrs[k]===undefined?null:this.attrs[k];}
    removeAttribute(k){delete this.attrs[k];}
    contains(n){return this===n||this.id==='session-exit-dialog'&&['session-exit-cancel','session-exit-confirm'].includes(n.id);}
    addEventListener(k,f){this.events[k]=f;}
}
function el(id){assert(ids.has(id),id);if(!nodes.has(id)){nodes.set(id,new Node(id));}return nodes.get(id);}
function key(k,extra={}){const e={key:k,stopped:false,used:false,preventDefault(){this.used=true;},stopPropagation(){this.stopped=true;},...extra};listeners.keydown(e);return e;}
let calls=0,finish;
const ui=Exit.bind({el,document:doc,confirm(){calls++;return new Promise(resolve=>{finish=resolve;});}});
async function run(){
    ui.open();assert.equal(calls,0);ui.init();el('viewport').focus();el('app-workspace').setAttribute('aria-hidden','false');
    ui.open();assert(!el('session-exit-dialog').hidden);assert.equal(calls,0);assert.equal(doc.activeElement,el('session-exit-cancel'));
    ui.open();assert.equal(doc.activeElement,el('session-exit-cancel'));
    assert.equal(el('app-header').getAttribute('aria-hidden'),'true');assert(key('ArrowRight').stopped);
    assert(key('Tab',{shiftKey:true}).used);assert.equal(doc.activeElement,el('session-exit-confirm'));
    assert(key('Tab').used);assert.equal(doc.activeElement,el('session-exit-cancel'));
    let trapped=false;listeners.focusin({target:el('viewport'),stopPropagation(){trapped=true;}});assert(trapped);assert.equal(doc.activeElement,el('session-exit-cancel'));
    trapped=false;listeners.focusin({target:el('session-exit-confirm'),stopPropagation(){trapped=true;}});assert(trapped,'other modal must not steal focus from exit buttons');
    key('Escape',{isComposing:true});assert(!el('session-exit-dialog').hidden);
    const enter=key('Enter');assert(!enter.used);assert.equal(doc.activeElement,el('session-exit-cancel'));assert.equal(calls,0);
    el('session-exit-cancel').onclick();assert(el('session-exit-dialog').hidden);assert.equal(doc.activeElement,el('viewport'));
    assert.equal(el('app-header').getAttribute('aria-hidden'),null);assert.equal(el('app-workspace').getAttribute('aria-hidden'),'false');
    assert(!key('ArrowRight').stopped);assert.equal(calls,0);
    el('logout').focus();el('logout').onclick();assert(key('Escape').used);assert.equal(doc.activeElement,el('logout'));assert.equal(calls,0);
    ui.open();el('session-exit-dialog').events.click({target:el('session-exit-confirm')});assert(!el('session-exit-dialog').hidden);
    el('session-exit-dialog').events.click({target:el('session-exit-dialog')});assert(el('session-exit-dialog').hidden);assert.equal(calls,0);
    ui.open();ui.stop();assert(el('session-exit-dialog').hidden);assert(el('logout').disabled);assert.equal(calls,0);
    ui.open();assert(el('session-exit-dialog').hidden);ui.init();ui.open();const p=el('session-exit-confirm').onclick();
    assert.equal(calls,1);assert(el('logout').disabled);assert(el('session-exit-dialog').hidden);
    ui.init();ui.open();el('session-exit-confirm').onclick();assert.equal(calls,1,'duplicate shutdown callback');
    finish();await p;assert.equal(calls,1);assert(el('logout').disabled);
    console.log('WEB SESSION EXIT: ALL OK (cancel default, modal focus/Escape/IME, no action before confirmation, once-only shutdown, lifecycle)');
}
run().catch(e=>{console.error(e);process.exitCode=1;});
