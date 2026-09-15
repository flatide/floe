'use strict';
// Deterministic lifecycle/DOM binding, not a browser or compositor acceptance test.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const Page=require('./display-page.js');
const bundle='a'.repeat(40),csrf='b'.repeat(64),token='c'.repeat(64),id='d'.repeat(64);
const auth={protocol:1,bundle,csrf,session_id:id};
const caps={protocol:1,bundle,display_only:true,render:false,catalog:false};
const key='floe-display-session:http://127.0.0.1:1234';
const flush=async()=>{for(let i=0;i<8;i++){await Promise.resolve();}};
function harness({fragment='#bootstrap='+token,saved=null,storageFails=false}={}){
    const requests=[],nodes=new Map(),events={},order=[],stored=new Map(saved?[[key,JSON.stringify(saved)]]:[]);
    const display={opened:0,closed:0,runs:0,open(){this.opened++;},close(){this.closed++;}};
    const exit={enabled:0,stops:0,init(){this.enabled++;},stop(){this.stops++;}};
    let displayOptions,exitOptions;
    class XHR{
        constructor(){this.headers={};this.responseText='';this.status=0;requests.push(this);}
        open(method,path){this.method=method;this.path=path;order.push('request:'+path);}
        setRequestHeader(k,v){this.headers[k]=v;}
        send(body){this.body=body;}
        abort(){this.aborted=true;if(this.onabort){this.onabort();}}
        answer(status,value){this.status=status;this.responseText=value===undefined?'':JSON.stringify(value);this.onload();}
    }
    const doc={getElementById(k){if(!nodes.has(k)){nodes.set(k,{textContent:''});}return nodes.get(k);},
        querySelector(){return {content:bundle};},createElement(){return {getContext(){return {};}};}};
    const win={FloeDisplayTest:{bind(o){displayOptions=o;return display;}},FloeSessionExit:{bind(o){exitOptions=o;return exit;}},
        FloeImageDecode:{create(){throw Error('No decoder before explicit Run');}},ImageData:function(){},URL:{createObjectURL(){}},
        addEventListener(n,f){events[n]=f;},setTimeout,clearTimeout,
        sessionStorage:{getItem(k){if(storageFails){throw Error('denied');}return stored.get(k)||null;},setItem(k,v){if(storageFails){throw Error('denied');}stored.set(k,v);},removeItem(k){if(storageFails){throw Error('denied');}stored.delete(k);}}};
    const page=Page.bind({window:win,document:doc,location:{hash:fragment,origin:'http://127.0.0.1:1234',pathname:'/'},
        history:{replaceState(a,b,p){assert.equal(p,'/');order.push('scrub');}},XHR});
    return {page,requests,display,exit,events,stored,order,nodes,displayOptions,get exitOptions(){return exitOptions;},status(){return nodes.get('display-page-status').textContent;}};
}
(async()=>{
    const h=harness();const start=h.page.start();
    assert.deepEqual(h.order,['scrub','request:/api/v1/session/exchange']);
    assert.deepEqual(JSON.parse(h.requests[0].body),{bootstrap:token,protocol:1,bundle});
    assert.equal(h.display.opened,0);h.requests[0].answer(200,auth);await flush();
    assert.equal(h.requests[1].path,'/api/v1/capabilities');assert.equal(h.requests[1].headers['X-Floe-CSRF'],csrf);
    h.requests[1].answer(200,caps);await start;assert.equal(h.display.opened,1);assert.equal(h.exit.enabled,1);
    assert.match(h.status(),/Ready/);assert.equal(h.display.runs,0);assert.equal(h.requests.length,2);
    assert.equal(h.displayOptions.csrf(),csrf);await h.page.start();assert.equal(h.requests.length,2);
    const quit=h.exitOptions.confirm();assert.equal(h.requests[2].method,'DELETE');assert.equal(h.requests[2].path,'/api/v1/session');assert.equal(h.requests[2].headers['X-Floe-CSRF'],csrf);
    assert.equal(h.stored.size,0);assert.equal(h.display.closed,1);
    h.requests[2].answer(204);await quit;assert.match(h.status(),/session closed/);assert.equal(h.displayOptions.csrf(),'');
    await h.exitOptions.confirm();assert.equal(h.requests.length,3);

    const reload=harness({fragment:'',saved:auth});const r=reload.page.start();
    assert.equal(reload.requests.length,1);assert.equal(reload.requests[0].path,'/api/v1/capabilities');
    reload.requests[0].answer(200,caps);await r;assert.equal(reload.display.opened,1);
    reload.events.pagehide();assert.equal(reload.display.closed,1);assert.equal(reload.requests.length,1);assert.equal(reload.stored.size,1);
    reload.events.pageshow({persisted:true});assert.match(reload.status(),/stopped state/);
    await reload.page.start();assert.equal(reload.requests.length,1);

    for(const fragment of ['','#bootstrap=bad','#arbitrary']){
        const n=harness({fragment,saved:fragment?auth:null});await n.page.start();
        assert.equal(n.requests.length,0);assert.equal(n.display.opened,0);assert.equal(n.stored.size,0);
    }
    for(const stage of ['exchange','capabilities']){
        const c=harness();const p=c.page.start();
        if(stage==='capabilities'){c.requests[0].answer(200,auth);await flush();}
        const current=c.requests[c.requests.length-1];c.events.pagehide();assert.equal(current.aborted,true);
        current.answer(200,stage==='exchange'?auth:caps);await p;
        assert.equal(c.display.opened,0);assert.equal(c.exit.enabled,0);assert.equal(c.displayOptions.csrf(),'');
        if(stage==='exchange'){assert.equal(c.stored.size,0);}
    }
    for(const bad of [{...auth,csrf:'wrong'},{...auth,bundle:'stale'},null]){
        const c=harness();const p=c.page.start();c.requests[0].answer(200,bad);await p;
        assert.equal(c.display.opened,0);assert.equal(c.requests.length,1);assert.equal(c.stored.size,0);
    }
    for(const bad of [{...caps,display_only:false},{...caps,render:true},{...caps,bundle:'wrong'}]){
        const c=harness({fragment:'',saved:auth});const p=c.page.start();c.requests[0].answer(200,bad);await p;
        assert.equal(c.display.opened,0);assert.equal(c.stored.size,0);assert.match(c.status(),/not a matching/);
    }
    const failed=harness({fragment:'',saved:auth});const f=failed.page.start();failed.requests[0].answer(401);await f;
    assert.equal(failed.stored.size,0);assert.equal(failed.requests.length,1);
    const huge=harness();const hp=huge.page.start();huge.requests[0].onprogress({loaded:65537});await hp;
    assert.equal(huge.requests[0].aborted,true);assert.equal(huge.display.opened,0);
    const noStore=harness({storageFails:true});const ns=noStore.page.start();noStore.requests[0].answer(200,auth);await flush();noStore.requests[1].answer(200,caps);await ns;
    assert.equal(noStore.display.opened,1);assert.match(noStore.status(),/storage is unavailable/);
    const q=noStore.exitOptions.confirm();noStore.requests[2].ontimeout();await q;
    assert.match(noStore.status(),/not confirmed/);assert.equal(noStore.requests.length,3);
    const html=fs.readFileSync(path.join(__dirname,'display.html'),'utf8');
    for(const source of ['display-test.js','session-exit.js']){
        const text=fs.readFileSync(path.join(__dirname,source),'utf8');
        for(const m of text.matchAll(/el\('([^']+)'\)/g)){assert.ok(html.includes('id="'+m[1]+'"'),m[1]);}
    }
    assert.ok(!html.includes('/app.js'));assert.ok(html.includes('/image-decode.js'));
    console.log('DISPLAY PAGE: ALL OK (auth, no autorun/view, reload, cancellation, quit, bounds; deterministic DOM only)');
})().catch(e=>{console.error(e);process.exitCode=1;});
