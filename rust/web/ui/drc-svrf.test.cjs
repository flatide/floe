'use strict';
// Deterministic actor replies/DOM; native API/Python parity and browser QA are
// separate gates. Exercise held responses without timing sleeps.
const assert=require('node:assert/strict'),D=require('./drc.js'),P=require('./protocol.js');
const nodes=new Map(),requests=[],moves=[],saves=[],chosen=new Map();
let saved=null,serial=0,selectionRev='1',holdComparison=false,holdDescription=false,holdType=false,holdFocus=false;
let heldComparison,heldDescription,heldType,heldFocus,badComparison=false,badTypes=false,mode='normal';
const ctx=new Proxy({}, {get:(t,k)=>k in t?t[k]:()=>{}});
class Element {
    constructor(){this.children=[];this.style={};this.checked=false;this.value='';this.hidden=false;this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}
    set innerHTML(_){throw new Error('Never interpret metadata as HTML');}
    appendChild(e){this.children.push(e);return e;}setAttribute(k,v){this[k]=v;}
    getContext(){return ctx;}focus(){}scrollIntoView(){}
}
const el=id=>{if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
el('drc-waived').value='all';el('drc-markers').checked=true;
let context={id:'view1',source:'source',connected:true,pending:false,state:{state_rev:'1',status:'idle',dbu_um:'1',bbox_dbu:['0','0','20','20'],pixels:[200,200]}};
const types=['width','area','other',...Array.from({length:67},(_,i)=>'custom'+String(i).padStart(2,'0'))];
const a={check:'0',name:'WIDTH <img>',errors:'2',waived:'1'},b={check:'1',name:'AREA',errors:'1',waived:'0'};
const row=(i,check='0')=>({check,local:String(i),global:check==='0'?P.next(String(i)):'9007199254740993',kind:'p',status:i===1?1:0,bbox_um:['1','1','5','3'],points:'4'});
const meta={rule:{desc:'<script>not HTML</script>',constraints:[{metric:'width',op:'<',value:'0.7',text:'WIDTH < 0.7',raw:null},
    {metric:'width',op:'<',value:'0.7',text:'WIDTH < 0.7',raw:null},{metric:'width',op:'>',value:null,text:'WIDTH > BAD',raw:'BAD'}],
    layers:['M1<&>'],source_gds:[['7',null],['8','0']],unresolved:['UNKNOWN']},metrics:['width'],
    derivations:[{name:'M1',rhs:'A AND B <raw>'}],derivations_more:true};
const compared=(q)=>({check:q.check,local:q.error,global:row(Number(q.error),q.check).global,comparison:badComparison?{metric:'width',measured:'NaN'}:
    q.error==='1'?null:{constraint:'1',metric:q.check==='0'?'width':'area',op:'<',unit:q.check==='0'?'um':'um2',measured:'0.6',bound:'0.7',delta:'-0.09999999999999998',percent:'-14.285714285714283'}});
function hold(token,v,set){return new Promise(resolve=>{set({token,q:null,v,resolve:()=>resolve(v)});token.abort=()=>{};});}
function groups(){const rules=[...chosen].map(([check,errors])=>({check,errors}));return {revision:'r1',view_id:context.id,state:{selection_rev:selectionRev,total:String(rules.reduce((n,r)=>n+r.errors.length,0)),limit:5000,rules}};}
function http(method,path,envelope,missing,token){
    const q=envelope&&envelope.body;requests.push({method,path,q,envelope,token});
    if(path.endsWith('/selection')){
        if(method==='POST'){assert.equal(envelope.base_selection_rev,selectionRev);selectionRev=P.next(selectionRev);if(q.kind==='clear_all')chosen.clear();else chosen.set(q.check,q.errors);}
        return Promise.resolve(groups());
    }
    if(!q)return Promise.resolve({drc:{id:'drc',revision:'r1',source_id:'source',title:'SVRF sample',phase:'ready',metadata:{checks:'2',errors:'3',
        svrf:mode==='none'?null:{matched:mode==='empty'?'0':'2',checks:'2',type_count:mode==='empty'?'0':String(types.length)}}}});
    if(q.kind==='types'){
        const list=mode==='empty'?[]:types, start=Number(q.start),end=Math.min(start+q.limit,list.length);
        const v={available:true,total:String(list.length),rows:list.slice(start,end).map(metric=>({metric,checks:'2'})),next:badTypes?'0':end<list.length?String(end):null};
        return holdType?hold(token,v,h=>{heldType=h;}):Promise.resolve(v);
    }
    if(q.kind==='rules'){
        assert('metric' in q&&'waived' in q);assert.equal(q.limit,32);
        let list=q.metric==='width'?[a]:q.metric==='area'?[b]:q.metric===null?[a,b]:[];
        list=list.filter(r=>(!q.search||r.name.toLowerCase().includes(q.search.toLowerCase()))&&
            (q.waived===null||(q.waived?Number(r.waived)>0:Number(r.errors)>Number(r.waived))));
        // Real actor bounds the input scan, not just matching output count.
        const v={rows:q.metric==='width'&&q.start==='0'?[]:list,next:q.metric==='width'&&q.start==='0'?'4096':null};
        return Promise.resolve(v);
    }
    if(q.kind==='rule'){
        const r=q.check==='0'?a:b,v={...r,description:'pack description '+r.name,svrf:mode==='normal'?meta:null};
        return holdDescription?hold(token,v,h=>{heldDescription=h;}):Promise.resolve(v);
    }
    if(q.kind==='comparison'){
        const v=compared(q);return holdComparison?hold(token,v,h=>{heldComparison=h;}):Promise.resolve(v);
    }
    if(q.kind==='list')return Promise.resolve({rows:(q.check==='0'?[row(0),row(1)]:[row(0,'1')]).filter(r=>P.compare(r.local,q.start)>=0&&
        (q.waived===null||(r.status===1)===q.waived)),next:null,scanned:'2',bbox_um:q.in_view?context.state.bbox_dbu:null,selection_rev:q.selection_rev});
    if(q.kind==='geometry')return Promise.resolve({...row(Number(q.error),q.check),precision:'1',points_dbu:[['1','1'],['5','1'],['5','3'],['1','3']].slice(0,q.limit),start:'0',total:'4',next:q.limit<4?String(q.limit):null});
    if(q.kind==='focus'){
        const v=require('./test-focus.cjs').reply(q,{navigation:{kind:'goto',center_um:['3','2'],width_um:'8'}});
        return holdFocus?hold(token,v,h=>{heldFocus=h;}):Promise.resolve(v);
    }
    if(q.kind==='measurements')return Promise.resolve({check:q.check,local:q.error,global:row(Number(q.error),q.check).global,segments:[]});
    if(q.kind==='records')return Promise.resolve({rows:q.errors.map(id=>row(Number(id),q.check))});
    throw new Error('Unexpected '+JSON.stringify(q));
}
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},protocol:P,rulers:require('./rulers.js'),groups:require('./drc-groups.js'),http,
    window:{requestAnimationFrame:()=>++serial,cancelAnimationFrame(){}},context:()=>context,
    navigate:require('./test-focus.cjs').accept(moves),resize(){},
    stateStore:{bind:o=>{let ready=false;return {attach:async()=>{ready=false;await o.apply(saved);ready=true;},change:v=>{if(ready){saved=JSON.parse(JSON.stringify(v));saves.push(saved);}},close(){ready=false;}};}}});
const count=k=>requests.filter(r=>r.q&&r.q.kind===k).length,last=k=>requests.filter(r=>r.q&&r.q.kind===k).at(-1);
async function tick(){for(let i=0;i<90;i++)await Promise.resolve();}
async function type(value){el('drc-type').value=value;el('drc-type').onchange();await tick();}
async function toggle(id,v){el(id).checked=v;el(id).onchange();await tick();}
(async()=>{
    assert.equal(D.metadataText(meta).match(/constraint: WIDTH < 0.7/g).length,1);
    assert.match(D.metadataText(meta),/unresolved bound: BAD/);assert.match(D.metadataText(meta),/7\/\*, 8\/0/);
    assert.match(D.metadataText(meta),/<raw>/);assert.match(D.metadataText(meta),/more derivations/);
    const value=compared({check:'0',error:'0'});
    assert.match(D.comparisonText(value,row(0),P),/Δ -0.09999999999999998 µm/);
    assert.throws(()=>D.comparisonText({...value,global:'9007199254740993'},row(0),P));
    assert.throws(()=>D.comparisonText({...value,comparison:{...value.comparison,measured:'NaN'}},row(0),P));
    assert.match(D.comparisonText({...value,comparison:{...value.comparison,bound:'0',delta:'0.6',percent:null}},row(0),P),/Δ \+0.6 µm$/);
    await panel.init();await tick();assert.equal(count('types'),1);assert.equal(el('drc-type').children.length,33);
    assert(!el('drc-type').disabled);assert.match(el('drc-rule-metadata').textContent,/<script>not HTML/);
    el('drc-errors').children[0].onclick({shiftKey:true});await tick();assert.equal(chosen.size,1);
    const errorReads=count('list'),n=saves.length;await type('width');assert.equal(count('list'),errorReads);
    assert.equal(chosen.size,1);assert.match(el('drc-rules').textContent,/Continue/);assert(!el('drc-rule-next').disabled);
    assert.equal(saved.metric,'width');assert(saves.length>n);assert.equal(saved.check,'0','type filter cleared the open rule');
    el('drc-rule-next').onclick();await tick();assert.equal(last('rules').q.start,'4096');assert.equal(el('drc-rules').children[0].textContent,'WIDTH <img>  ·  2');
    el('drc-type-next').onclick();await tick();assert.equal(count('types'),2);assert.equal(el('drc-type').children.length,34);assert.equal(el('drc-type').value,'width');
    el('drc-type-next').onclick();await tick();assert(el('drc-type-next').disabled);assert.equal(el('drc-type').children.length,8);
    await type('custom66');const reads=count('types');await el('drc-reload').onclick();await tick();
    assert.equal(count('types'),reads+1,'restore drained every type page');assert.equal(el('drc-type').value,'custom66');assert.equal(saved.metric,'custom66');
    await type('');el('drc-search').value='area';el('drc-search-form').onsubmit({preventDefault(){}});await tick();
    el('drc-waived').value='waived';el('drc-waived').onchange();await tick();assert.equal(chosen.size,0);
    assert.deepEqual({metric:last('rules').q.metric,waived:last('rules').q.waived,search:last('rules').q.search},{metric:null,waived:true,search:'area'});
    assert.match(el('drc-rules').textContent,/No matching/);
    el('drc-search').value='';el('drc-waived').value='all';el('drc-waived').onchange();await tick();
    holdDescription=true;el('drc-rules').children[1].onclick();await tick();const oldDescription=heldDescription;
    holdDescription=false;el('drc-rules').children[0].onclick();await tick();oldDescription.resolve();await tick();
    assert(oldDescription.token.cancelled);assert.match(el('drc-description').textContent,/WIDTH/);
    holdComparison=true;el('drc-errors').children[0].onclick();await tick();const old=heldComparison;
    holdComparison=false;el('drc-errors').children[1].onclick();await tick();old.resolve();await tick();
    assert(old.token.cancelled);assert.match(el('drc-comparison').textContent,/No unambiguous/);assert.equal(moves.length,0);
    el('drc-errors').children[0].onclick();await tick();assert.match(el('drc-comparison').textContent,/Measured 0.6 µm vs < 0.7/);
    const compReads=count('comparison'),typeReads=count('types');context={...context,state:{...context.state,state_rev:'2'}};panel.contextChanged();await tick();
    assert.equal(count('comparison'),compReads);assert.equal(count('types'),typeReads,'pan reread metadata');
    await toggle('drc-in-view',true);el('drc-errors').children[0].onclick();await tick();assert(el('drc-in-view').checked,'plain selection changed filter');
    holdFocus=true;el('drc-errors').children[0].ondblclick();await tick();const stale=heldFocus;
    context={...context,state:{...context.state,state_rev:'3'}};stale.resolve();await tick();assert.equal(moves.length,0);assert(el('drc-in-view').checked,'failed jump changed filter');
    holdFocus=false;el('drc-frame').onclick();await tick();assert.equal(moves.length,1);assert(!el('drc-in-view').checked);
    assert.equal(saved.selected.error,'0');assert(saved.jump_active&&saved.focus_visible);assert.equal(last('list').q.in_view,false);assert.equal(last('list').q.start,'0');
    assert.equal(last('list').envelope.state_rev,undefined);assert.match(el('drc-comparison').textContent,/Measured/);
    saved=JSON.parse(JSON.stringify(saved));const savesBefore=saves.length;await el('drc-reload').onclick();await tick();assert.equal(moves.length,1);
    assert.equal(saves.length,savesBefore,'restore wrote back before an edit');assert.match(el('drc-comparison').textContent,/Measured/);
    badComparison=true;el('drc-errors').children[1].onclick();await tick();assert.match(el('drc-comparison').textContent,/unavailable/);badComparison=false;
    holdComparison=true;el('drc-errors').children[0].onclick();await tick();const cleared=heldComparison;el('drc-clear').onclick();cleared.resolve();await tick();assert(cleared.token.cancelled);
    assert.match(el('drc-comparison').textContent,/Select an error/);holdComparison=false;
    badTypes=true;el('drc-type-next').onclick();await tick();assert(el('drc-type').disabled);assert.match(el('drc-type-info').textContent,/unavailable/);badTypes=false;
    await el('drc-reload').onclick();await tick();assert(!el('drc-type').disabled);
    holdType=true;el('drc-type-next').onclick();await tick();const oldType=heldType;
    context={...context,id:'other',source:'other'};panel.contextChanged();oldType.resolve();await tick();assert(oldType.token.cancelled);assert(el('drc-type').disabled);
    holdType=false;mode='empty';saved=null;context={...context,id:'empty',source:'source'};panel.stop();await panel.resume();await tick();
    assert(el('drc-type').disabled);assert.match(el('drc-type-info').textContent,/no classified/);assert.equal(el('drc-type').children.length,1);
    mode='none';saved=null;context={...context,id:'none'};panel.stop();const before=count('types');await panel.resume();await tick();
    assert.equal(count('types'),before);assert(el('drc-type').disabled);assert.match(el('drc-type-info').textContent,/No SVRF/);panel.stop();
    console.log('WEB SVRF UI: ALL OK (bounded types, intersections/groups, plain-text metadata/comparison, stale/cancel/restore, In view jump ordering, no pan reads)');
})().catch(e=>{console.error(e);process.exitCode=1;});
