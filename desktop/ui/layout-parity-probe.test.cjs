'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const vm=require('node:vm'),fs=require('node:fs');
const code='('+fs.readFileSync(__dirname+'/layout-parity-probe.js','utf8')+')(compare,configure,fingerprint)';
function setup({badPhase=-1,labels=true,hidden=false}={}) {
    const nodes={logout:{disabled:false},fit:{disabled:false},empty:{hidden:true},rendering:{hidden:true},
        status:{textContent:'Live margin crop'},'margin-info':{textContent:''},detail:{value:'high'},'goto-width':{value:'300'},
        'goto-x':{value:'200'},'goto-y':{value:'200'},
        canvas:{dataset:{frameId:'foreground',renderRev:'1'}},'margin-canvas':{hidden:false,dataset:{frameId:'margin',renderRev:'1'}},
        labels:{checked:labels},frames:{checked:true}};
    let comparisons=0,changes=0,clock=0;
    for(const id of ['labels','frames']) nodes[id].dispatchEvent=event=>{
        assert.equal(event.type,'change');changes++;
        nodes.canvas.dataset.renderRev=nodes['margin-canvas'].dataset.renderRev=String(changes+1);
    };
    const context=vm.createContext({window:{},document:{hidden,getElementById:id=>nodes[id]},location:{hash:''},devicePixelRatio:2,configure:undefined,fingerprint:undefined,
        performance:{now:()=>clock+=1000},Event:class {constructor(type){this.type=type;}},
        requestAnimationFrame:cb=>queueMicrotask(cb),setTimeout:cb=>queueMicrotask(cb),
        compare:()=>({pixels:[100,80],foreground_lit:300,margin_lit:328,
            changed_pixels:comparisons++===badPhase?1:changes===0?28:0})});
    return {poll:()=>vm.runInContext(code,context),nodes,context,counts:()=>[comparisons,changes]};
}
async function finish(h) {
    const reports=[];
    for(let i=0;i<200;i++) {
        const r=h.poll();if(r!=='wait')reports.push(r);
        if(r==='layout-ok'||r.startsWith('layout-failed-'))return reports;
        await new Promise(resolve=>setImmediate(resolve));
    }
    throw Error('probe did not terminate');
}
test('one walk, three ordered comparisons; label difference allowed, geometry strict',async()=>{
    const h=setup();const results=await finish(h);
    assert.deepEqual(results,['layout-metric 0 100 80 2 300 328 28','layout-metric 1 100 80 2 300 328 0',
        'layout-metric 2 100 80 2 300 328 0','layout-ok']);
    assert.deepEqual(h.counts(),[3,2]);assert.equal(h.poll(),'layout-ok');assert.deepEqual(h.counts(),[3,2]);
});
test('geometry mismatch never becomes success',async()=>{
    for(const phase of [1,2]) {
        const h=setup({badPhase:phase});const reports=await finish(h);
        assert.equal(reports.at(-1),'layout-failed-compare'+phase);assert.ok(!reports.includes('layout-ok'));
    }
});
test('wrong initial controls, hidden, mismatched, unready or zero-lit frames fail',async()=>{
    for(const alter of [h=>h.nodes.labels.checked=false,h=>h.context.document.hidden=true,
        h=>h.nodes['margin-canvas'].dataset.renderRev='old',h=>h.nodes.rendering.hidden=false,
        h=>h.context.location.hash='#not-consumed',h=>h.context.compare=()=>({foreground_lit:0,margin_lit:0})]) {
        const h=setup();alter(h);assert.match((await finish(h)).at(-1),/^layout-failed-(ready|compare)0$/);
    }
});
test('phase transition must await a new revision; existing margin does not pass',async()=>{
    const h=setup();h.nodes.labels.dispatchEvent=()=>{};
    const reports=await finish(h);assert.equal(reports.at(-1),'layout-failed-ready1');assert.deepEqual(h.counts(),[1,0]);
});
test('cross-host mode waits for actual resized Canvas and emits fingerprints in phase order',async()=>{
    const h=setup();let configured=0;
    h.context.configure=()=>{configured++;h.nodes.canvas.width=100;h.nodes.canvas.height=80;return [100,80];};
    h.context.fingerprint=async()=> '100 80 2 fingerprint';
    const r=await finish(h);assert.equal(configured,1);
    assert.equal(r.filter(s=>s.startsWith('layout-cross ')).length,3);
    assert.equal(r.at(-1),'layout-ok');
    const missing=setup();missing.context.configure=()=>[100,80];
    assert.equal((await finish(missing)).at(-1),'layout-failed-resize');
});
test('resize keeping scale is followed by one explicit goto, not an assumed unchanged world width',async()=>{
    const h=setup();let gotos=0;
    h.context.configure=()=>{
        h.nodes.canvas.width=100;h.nodes.canvas.height=80;h.nodes['goto-width'].value='260';
        h.nodes.canvas.dataset.renderRev=h.nodes['margin-canvas'].dataset.renderRev='resize';return [100,80];
    };
    h.nodes.goto={disabled:false,click:()=>{
        gotos++;assert.equal(h.nodes['goto-width'].value,'300');
        h.nodes.canvas.dataset.renderRev=h.nodes['margin-canvas'].dataset.renderRev='goto';
    }};
    assert.equal((await finish(h)).at(-1),'layout-ok');assert.equal(gotos,1);
});
