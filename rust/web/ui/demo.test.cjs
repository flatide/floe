'use strict';
const assert=require('node:assert/strict'),Demo=require('./demo.js');
const tick=()=>new Promise(r=>setImmediate(r));
function fixture(options={}){
    const requests=[],links=[],buttons=[],status={textContent:''},warning={hidden:true};
    const doc={querySelector:()=>options.httpTest===undefined?null:{content:String(options.httpTest)},
        getElementById:id=>id==='demo-status'?status:id==='transport-warning'?warning:{appendChild:b=>buttons.push(b)},createElement:()=>({})};
    class XHR{
        open(method,path){this.method=method;this.path=path;}setRequestHeader(k,v){assert.equal(k,'Content-Type');assert.equal(v,'application/json');}
        send(body){requests.push({method:this.method,path:this.path,body:body&&JSON.parse(body)});
            if(this.method==='POST'&&options.timeout){this.ontimeout();return;}
            this.status=options.code||200;this.responseText=JSON.stringify(this.method==='GET'?{samples:options.samples||['valmini']}:
                options.reply||{launch_id:'a'.repeat(64),bootstrap:'b'.repeat(64),viewer_ready:true});this.onload();}
    }
    const c=Demo.bind({document:doc,XHR,location:{protocol:options.http?'http:':'https:',assign:url=>links.push(url)}});
    return {c,requests,links,buttons,status,warning};
}
(async()=>{
    const f=fixture();await f.c.start();assert.equal(f.requests.length,1);assert.equal(f.buttons[0].textContent,'valmini');
    f.buttons[0].onclick();f.buttons[0].onclick();await tick();assert.equal(f.requests.length,2);
    assert.deepEqual(f.requests[1],{method:'POST',path:'/api/v1/demo/launches',body:{sample_id:'valmini'}});
    assert.equal(f.links[0],'/server/'+'a'.repeat(64)+'#bootstrap='+'b'.repeat(64));
    const http=fixture({http:true,httpTest:true});await http.c.start();assert.equal(http.buttons.length,1);assert.equal(http.warning.hidden,false);
    http.buttons[0].onclick();await tick();assert.equal(http.links.length,1);
    assert.equal(f.warning.hidden,true);
    for(const options of [{samples:['<script>']},{samples:['a','a']},{samples:['../a']},{code:429},{http:true},{http:true,httpTest:false},{httpTest:true}]){
        const f=fixture(options);await f.c.start();assert.equal(f.buttons.length,0);assert.equal(f.links.length,0);
    }
    for(const options of [{timeout:true},{reply:{launch_id:'https://evil.test',bootstrap:'b'.repeat(64),viewer_ready:true}}]){
        const f=fixture(options);await f.c.start();f.buttons[0].onclick();await tick();
        assert.equal(f.requests.length,2,'no automatic POST replay');assert.equal(f.links.length,0);assert.equal(f.buttons[0].disabled,false);
    }
    console.log('PUBLIC DEMO UI: ALL OK (HTTPS default, explicit HTTP test warning, IDs, explicit launch, bounded response, no replay/redirect injection)');
})().catch(e=>{console.error(e);process.exitCode=1;});
