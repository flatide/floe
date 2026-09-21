'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const script='('+fs.readFileSync(__dirname+'/recovery-probe.js','utf8')+')';
function context(){
    const values=new Map([['unrelated','preserve']]),nodes={logout:{disabled:false},'browse-dialog':{hidden:false},'browse-close':{disabled:false}};
    const c={window:{},location:{hash:''},document:{getElementById:id=>nodes[id]},sessionStorage:{getItem:k=>values.get(k)??null,setItem:(k,v)=>values.set(k,v),removeItem:k=>values.delete(k)}};
    const invoke=stage=>vm.runInNewContext(script,c)(stage);
    return {c,values,nodes,invoke};
}
{
    const h=context();assert.equal(h.invoke('arm'),'armed');assert.equal(h.invoke('check'),'wait');
    h.c.window={};h.nodes['browse-close'].disabled=true;assert.equal(h.invoke('check'),'wait');
    h.nodes['browse-close'].disabled=false;assert.equal(h.invoke('check'),'recovered');
    assert.deepEqual([...h.values],[['unrelated','preserve']]);
}
for(const mutate of [h=>h.c.location.hash='#bootstrap=synthetic',h=>h.c.sessionStorage.getItem=()=>{throw Error('blocked');},
    h=>h.values.set('floe.desktop.recovery.probe','existing'),h=>h.c.window.__floeDesktopRecoveryProbe=false]){
    const h=context();mutate(h);const before=[...h.values];assert.equal(h.invoke('arm'),'recovery-failed');assert.deepEqual([...h.values],before);
}
{
    const h=context();h.nodes.logout.disabled=true;assert.equal(h.invoke('arm'),'wait');assert.equal(h.values.size,1);
    h.nodes.logout.disabled=false;assert.equal(h.invoke('arm'),'armed');h.c.window={};h.values.delete('floe.desktop.recovery.probe');
    assert.equal(h.invoke('check'),'recovery-failed');assert.equal(h.invoke('unknown'),'recovery-failed');
}
console.log('desktop recovery probe: fixed markers, new document, storage retention/loss and no unrelated changes OK');
