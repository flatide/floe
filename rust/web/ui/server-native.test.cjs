'use strict';
// Rust supplies actual native bytes/snapshot via stdin, never a saved credential
// or production layout. DOM/RAF are simulated; this is NOT browser acceptance.
const assert=require('node:assert/strict'),fs=require('node:fs');
const {environment}=require('./server.test.cjs');
const {hello,state,packet}=JSON.parse(fs.readFileSync(0,'utf8'));
(async()=>{
    const e=environment({id:hello.view_id});await e.c.start();
    const ws=e.sockets[0];ws.onopen();ws.text({...hello,bundle:'test-bundle'});ws.text(state);
    ws.onmessage({data:Uint8Array.from(packet).buffer});e.raf();
    assert.equal(e.el('empty').hidden,true);
    assert.equal(ws.sent.at(-1).type,'frame.ack');assert.equal(ws.sent.at(-1).connection_epoch,hello.connection_epoch);
    assert.equal(e.el('canvas').width,state.pixels[0]);assert.equal(e.el('canvas').height,state.pixels[1]);
    assert(e.el('canvas').pixels.some((v,i)=>i%4!==3&&v!==0),'native geometry was displayed');
    e.el('in').onclick();const edit=ws.sent.at(-1);assert.equal(edit.type,'view.set');assert.equal(edit.base_state_rev,state.state_rev);
    assert.equal(edit.view_id,hello.view_id);e.c.stop();console.log('WEB SERVER NATIVE DISPLAY: ALL OK');
})().catch(e=>{console.error(e);process.exitCode=1;});
