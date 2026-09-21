'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const source=fs.readFileSync(__dirname+'/download-cancel-probe.js','utf8');
function page(){
    const nodes={logout:{disabled:false},'browse-dialog':{hidden:false},'browse-close':{disabled:false},'browse-refresh':{disabled:false}};
    let clicked=0,created=0;
    const context={window:{},location:{hash:''},Blob:class {constructor(parts,options){assert.deepEqual(Array.from(parts),['Synthetic download cancellation QA']);assert.equal(options.type,'text/plain');}},
        URL:{createObjectURL:()=>{created++;return 'blob:synthetic-qa';}},
        document:{getElementById:id=>nodes[id],body:{appendChild:()=>{}},createElement:tag=>{assert.equal(tag,'a');return {click(){assert.equal(this.download,'synthetic-download-cancel.txt');assert.equal(this.href,'blob:synthetic-qa');clicked++;},remove(){}};}}};
    return {nodes,context,run:()=>vm.runInNewContext(source,context),counts:()=>[clicked,created]};
}
const h=page();assert.equal(h.run(),'download-started');assert.equal(h.run(),'download-started');assert.deepEqual(h.counts(),[1,1]);
for(const mutate of [h=>{h.nodes.logout.disabled=true;},h=>{h.nodes['browse-dialog'].hidden=true;},h=>{h.nodes['browse-close'].disabled=true;},h=>{h.nodes['browse-refresh'].disabled=true;},h=>{h.context.location.hash='#synthetic';}]){
    const h=page();mutate(h);assert.equal(h.run(),'wait');assert.deepEqual(h.counts(),[0,0]);
}
console.log('DESKTOP DOWNLOAD PROBE: OK (one synthetic blob; no caller files/auth; waits for ready empty workspace)');
