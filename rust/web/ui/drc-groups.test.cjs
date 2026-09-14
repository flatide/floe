'use strict';
const assert=require('node:assert/strict'), G=require('./drc-groups.js'), P=require('./protocol.js');
const scope={path:'/selection',revision:'drc-r1',view:'v1'}, calls=[], statuses=[];
const snapshot=(revision='1',rules=[])=>({revision:scope.revision,view_id:scope.view,
    state:{selection_rev:revision,total:String(rules.reduce((n,r)=>n+r.errors.length,0)),limit:5000,rules}});
function http(method,path,body,missing,token){return new Promise((resolve,reject)=>{
    const call={method,path,body,token,resolve,reject};calls.push(call);token.abort=()=>{};
});}
const groups=G.bind({http,protocol:P,changed(){},status:s=>statuses.push(s)});
const apply={kind:'apply',check:'0',errors:['9007199254740993'],mode:'toggle'};
const last=()=>calls.at(-1);
async function tick(){for(let i=0;i<20;i++)await Promise.resolve();}
(async()=>{
    let promise=groups.attach(scope);assert(!groups.ready());last().resolve(snapshot());await promise;assert(groups.ready());
    promise=groups.change(apply,'7');assert(!groups.ready());assert.equal(last().body.base_selection_rev,'1');assert.equal(last().body.state_rev,'7');
    const count=calls.length;assert.equal(await groups.change(apply),false);assert.equal(calls.length,count,'queued a second toggle');
    last().resolve(snapshot('2',[{check:'0',errors:apply.errors}]));assert(await promise);assert(groups.contains('0',apply.errors[0]));assert.equal(groups.total(),1);
    assert.deepEqual(groups.references(),[{check:'0',error:'9007199254740993'}]);
    const copy=groups.references();copy[0].error='7';assert.equal(groups.references()[0].error,'9007199254740993');
    // The command may commit before a timeout. Only GET follows, never POST.
    promise=groups.change(apply);last().reject(new Error('timeout'));await tick();assert.equal(last().method,'GET');
    assert.equal(groups.references(),null);
    last().resolve(snapshot('3'));assert.equal(await promise,false);assert.equal(groups.total(),0);assert(groups.ready());assert.match(statuses.at(-1),/not retried/);
    assert.equal(calls.filter(c=>c.method==='POST').length,2);
    // An invalid success response is uncertain too. A failed GET disables edits.
    promise=groups.change(apply);last().resolve(snapshot('5'));await tick();assert.equal(last().method,'GET');last().reject(new Error('offline'));
    await promise;assert(!groups.ready());assert.match(statuses.at(-1),/uncertain/);assert.equal(await groups.change(apply),false);
    // An old scope's late POST/GET cannot populate a newly opened view.
    promise=groups.attach(scope);const old=last();const next=groups.attach({...scope,view:'v2'});assert(old.token.cancelled);
    old.resolve(snapshot('9',[{check:'0',errors:['1']}]));await promise;assert.equal(groups.total(),0);
    last().resolve({...snapshot(),view_id:'v2'});await next;assert(groups.ready());groups.close();assert(!groups.ready());assert.equal(groups.total(),0);
    assert.equal(groups.references(),null);
    const good=snapshot('18446744073709551615',[{check:'0',errors:['0','9007199254740993','18446744073709551615']}]);
    assert.equal(G.decode(good,scope,P).total,3);
    for(const change of [v=>v.state.total='4',v=>v.state.limit=6000,v=>v.view_id='wrong',v=>v.state.selection_rev=1,
        v=>v.state.rules[0].errors.reverse(),v=>v.state.rules[0].errors.push('1'),v=>v.state.rules.push({check:'0',errors:['7']}),
        v=>v.state.rules[0].errors=[],v=>v.state.rules[0].check='00',v=>v.state.rules[0].errors=['01'],
        v=>v.state.rules[0].errors=Array.from({length:5001},(_,i)=>String(i))]){
        const bad=JSON.parse(JSON.stringify(good));change(bad);assert.throws(()=>G.decode(bad,scope,P));
    }
    console.log('WEB DRC GROUPS: ALL OK (u64, bounds, strict CAS, no queue/replay, timeout reconciliation, invalid response, old scope)');
})().catch(e=>{console.error(e);process.exitCode=1;});
