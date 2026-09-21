'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
function page(){
    class XHR {open(){}send(body){this.body=body;}}
    const location=new URL('http://127.0.0.1:34567/'),window={XMLHttpRequest:XHR},removed=[];
    const nodes={logout:{disabled:false},'browse-dialog':{hidden:false},'browse-close':{disabled:false},
        connection:{textContent:'Not connected',getAttribute:k=>{assert.equal(k,'data-session-state');return 'restart-required';}}};
    const context={window,location,URL,WeakMap,document:{getElementById:id=>nodes[id]},
        sessionStorage:{removeItem:k=>{assert.equal(k,'floe-session:'+location.origin);removed.push(k);}}};
    vm.runInNewContext(fs.readFileSync(__dirname+'/session-loss-transport.js','utf8'),context);
    function send(method,url){const x=new XHR();x.open(method,url);const body=new Proxy({},{get(){throw Error('body read');}});x.send(body);assert.equal(x.body,body);}
    const probe=vm.runInNewContext('('+fs.readFileSync(__dirname+'/session-loss-probe.js','utf8')+')',context);
    return {context,window,nodes,removed,send,probe};
}
const first=page();first.send('POST','/api/v1/session/exchange');
assert.equal(first.probe('ready'),'wait','wait for the queued initial catalogue request');
first.send('POST','/api/v1/browse');
first.send('GET','/api/v1/capabilities');first.send('HEAD','/');first.send('POST','https://unrelated.invalid/api/v1/session/exchange');
assert.equal(first.probe('ready'),'loss-ready');assert.equal(first.probe('erase-storage'),'loss-erased');assert.equal(first.removed.length,1);
for(const method of ['POST','put','DELETE']){
    const h=page();h.send('POST','/api/v1/session/exchange');h.send(method,'/api/v1/operations');assert.equal(h.probe('ready'),'loss-failed-write');assert.equal(h.probe('erase-storage'),'loss-failed-write');assert.equal(h.removed.length,0);
}
const reloaded=page();reloaded.nodes.logout.disabled=true;assert.equal(reloaded.probe('check-storage'),'loss-confirmed');
assert.equal(reloaded.probe('check-cookie'),'loss-failed','missing storage must not pass as a lost cookie');
reloaded.nodes.connection.textContent='Session expired';assert.equal(reloaded.probe('check-cookie'),'loss-confirmed');
assert.equal(reloaded.probe('check-storage'),'loss-failed');
reloaded.send('POST','/api/v1/session/exchange');assert.equal(reloaded.probe('check-cookie'),'loss-failed-replay','reload must never replay bootstrap');
const early=page();delete early.nodes.logout;assert.equal(early.probe('ready'),'wait');
const busy=page();busy.send('POST','/api/v1/session/exchange');busy.nodes['browse-close'].disabled=true;assert.equal(busy.probe('erase-storage'),'wait');assert.equal(busy.removed.length,0);
const denied=page();denied.send('POST','/api/v1/session/exchange');denied.send('POST','/api/v1/browse');denied.context.sessionStorage.removeItem=()=>{throw Error('denied');};assert.equal(denied.probe('erase-storage'),'loss-failed-exception');
const repeated=page();repeated.send('POST','/api/v1/session/exchange');repeated.send('POST','/api/v1/browse');repeated.send('POST','/api/v1/browse');assert.equal(repeated.probe('ready'),'loss-failed-write');
const hidden=page();hidden.nodes.logout.disabled=false;assert.equal(hidden.probe('check-storage'),'wait','terminal marker alone is not a complete QA verdict');
console.log('DESKTOP SESSION LOSS QA: OK (exact key removal; paths/methods only; no bodies/auth reads; replay refused)');
