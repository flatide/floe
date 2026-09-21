'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const nodes={};
const document={getElementById:id=>nodes[id],hidden:false};
const probe=vm.runInNewContext('('+fs.readFileSync(__dirname+'/renderer-failure-probe.js','utf8')+')',{document});
assert.equal(probe('ready'),'wait');
for(const id of ['status','notice','canvas','margin-canvas','logout','fit','close','empty','zoom-in','rendering','open']){
    nodes[id]={textContent:'',hidden:false,disabled:false,dataset:{},width:1};
}
nodes.empty.hidden=true;nodes.status.textContent='Live · margin crop · gen 2';
assert.equal(probe('ready'),'renderer-await-margin','must require an actual landed frame');
nodes['margin-canvas'].dataset.frameId='synthetic';
assert.equal(probe('ready'),'renderer-ready');
document.hidden=true;assert.equal(probe('ready'),'renderer-document-hidden');document.hidden=false;
nodes['margin-canvas'].hidden=true;assert.equal(probe('ready'),'renderer-margin-hidden');nodes['margin-canvas'].hidden=false;
nodes.status.textContent='Live';assert.equal(probe('ready'),'renderer-await-live-crop');nodes.status.textContent='Live · margin crop · gen 2';
assert.equal(probe('failed'),'wait');
nodes.notice.textContent='The renderer failed.';
assert.equal(probe('failed'),'renderer-status-invalid','retained Live is the regression');
nodes.status.textContent='failed · last displayed image (not live)';
nodes.fit.disabled=true;nodes['zoom-in'].disabled=true;nodes.rendering.hidden=true;
assert.equal(probe('failed'),'renderer-failed');
for(const [id,property,value] of [['fit','disabled',false],['close','disabled',true],
    ['rendering','hidden',false],['empty','hidden',false]]){
    const before=nodes[id][property];nodes[id][property]=value;
    assert.equal(probe('failed'),'renderer-status-invalid');nodes[id][property]=before;
}
nodes.status.textContent='View closed';nodes.empty.hidden=false;nodes.close.disabled=true;
assert.equal(probe('closed'),'wait','stale frame metadata must be cleared');
delete nodes['margin-canvas'].dataset.frameId;
assert.equal(probe('closed'),'renderer-closed');
nodes.open.disabled=true;assert.equal(probe('closed'),'wait');
assert.equal(probe('unknown'),'wait');
console.log('DESKTOP RENDERER PROBE: OK (landed margin; failed not live; controls; explicit close)');
