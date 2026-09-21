'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const source=fs.readFileSync(__dirname+'/ime-probe.js','utf8');
require('../../tools/vendor/acorn-8.15.0/acorn.js').parse(source,{ecmaVersion:2017});
function probe(fault){
    let events=0;
    const panel={hidden:false};
    const input={disabled:false,value:'before',focus(){document.activeElement=this;},
        dispatchEvent(e){
            events++;assert(['Enter','Escape','Tab'].includes(e.key));
            if(fault==='hidden'){panel.hidden=true;}
            if(fault==='focus'){document.activeElement=null;}
            if(fault==='text'){this.value='changed';}
            return fault!=='cancelled';
        }};
    const document={activeElement:null,getElementById(id){return id==='browse-dialog'?panel:input;}};
    function KeyboardEvent(type,init){assert.equal(type,'keydown');Object.assign(this,init);if(fault==='unsupported'){this.keyCode=0;}}
    const result=vm.runInNewContext(source,{document,KeyboardEvent});
    assert.equal(input.value,'before');assert(events<=6);
    return result;
}
assert.equal(probe(null),'ime-ok');
for(const fault of ['cancelled','hidden','focus','text','unsupported']){assert.equal(probe(fault),'ime-failed');}
console.log('desktop synthetic IME probe: fixed markers, restoration and failures OK (not OS IME acceptance)');
