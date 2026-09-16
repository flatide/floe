'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs');
const D=require('./guest-drc.js'),G=require('./drc-geometry.js'),P=require('./protocol.js'),selection=require('./drc-groups.js');
const tick=()=>new Promise(r=>setImmediate(r));
const clone=v=>JSON.parse(JSON.stringify(v));
function environment(mode='explore',format='ice'){
    const nodes=new Map(),calls=[],drawing=[],moves=[];let context={view_id:'view-a',epoch:'epoch1',state_rev:'1',mode,pending:false,drc:{id:'approved',revision:'rev1'}};
    let panel=null,panelRev='1',selectionRev='1',ids=[],failedSelection=false,hold=null,active=0,maxActive=0,stepReply=null;
    class Element{
        constructor(){this.children=[];this.value='';this.checked=false;this.disabled=false;this.hidden=false;this.attrs={};}
        set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}set innerHTML(_){throw Error('No HTML in DRC text');}
        appendChild(e){this.children.push(e);}setAttribute(k,v){this.attrs[k]=v;}
    }
    for(const m of fs.readFileSync(__dirname+'/guest.html','utf8').matchAll(/id="([^"]+)"/g)){nodes.set(m[1],new Element());}
    const el=id=>{assert(nodes.has(id),'missing real HTML id '+id);return nodes.get(id);};
    const a={check:'0',local:'9007199254740993',global:'9007199254740994',kind:'p',status:0,bbox_um:['10','10','30','30'],points:'3'};
    const b={check:'0',local:'9007199254740994',global:'9007199254740995',kind:'e',status:1,bbox_um:['40','10','60','30'],points:'2'};
    const records=[a,b];
    function selected(){return {selection_rev:selectionRev,total:String(ids.length),limit:5000,rules:ids.length?[{check:'0',errors:ids.slice().sort((a,b)=>P.compare(a,b))}]:[]};}
    async function http(method,path,body,t){
        assert(['/drc','/drc/read','/drc/panel','/drc/selection'].includes(path));assert(!t.cancelled);active++;maxActive=Math.max(maxActive,active);
        const c=clone(context),request={method,path,body:body&&clone(body)};calls.push(request);
        try{await tick();if(hold&&path==='/drc/read'&&body.body.kind===hold.kind){const h=hold;hold=null;await new Promise((resolve,reject)=>{h.resolve=resolve;h.reject=reject;t.abort=()=>reject(Error('cancelled'));});t.abort=null;}
            let data;
            if(path==='/drc'){data={checks:'1',errors:'2',precision:'1000',format,truncated_records:'0',read_only:true};}
            else{assert.equal(body===undefined?c.view_id:body.view_id,c.view_id);assert.equal(body===undefined?c.drc.revision:body.revision,c.drc.revision);
                if(path==='/drc/panel'){
                    if(method==='POST'){assert(!body.body.metric&&!body.body.note_target);assert.equal(body.base_panel_rev,panelRev);panel=clone(body.body);panelRev=P.next(panelRev);}
                    data={panel_rev:panelRev,body:panel};
                }else if(path==='/drc/selection'){
                    if(method==='POST'){assert.equal(body.base_selection_rev,selectionRev);const v=body.body;
                        if(v.bbox_um){assert.equal(body.state_rev,c.state_rev);const area=v.bbox_um.map(Number);v.errors=v.errors.filter(id=>records.some(r=>r.local===id&&
                            (v.waived===null||(r.status===1)===v.waived)&&r.bbox_um[0]<=area[2]&&r.bbox_um[2]>=area[0]&&r.bbox_um[1]<=area[3]&&r.bbox_um[3]>=area[1]));}
                        if(v.kind==='clear_all'){ids=[];}else if(v.mode==='replace'){ids=v.errors.slice();}else if(v.mode==='add'){ids=[...new Set([...ids,...v.errors])];}else{for(const id of v.errors){ids=ids.includes(id)?ids.filter(v=>v!==id):[...ids,id];}}
                        selectionRev=P.next(selectionRev);if(failedSelection){failedSelection=false;throw Error('reply lost');}}
                    data=selected();
                }else{const r=body.body;assert(['rules','rule','list','records','geometry','focus','filtered_step'].includes(r.kind));
                    if(r.kind==='filtered_step'){data=stepReply||{hit:r.after===a.local?b:a,next:null,scanned:'1',bbox_um:r.in_view?['0','0','64','32']:null,selection_rev:r.selection_rev};stepReply=null;}
                    if(r.kind==='rules'){data={rows:[{check:'0',name:'<script>RULE</script>',errors:'2',waived:'1'}],next:null};}
                    if(r.kind==='rule'){data={check:r.check,description:'<img src=x> rule description'};}
                    if(r.kind==='list'){data={rows:records.filter(v=>(r.waived===null||(v.status===1)===r.waived)&&(!r.selection_rev||ids.includes(v.local))),next:null,selection_rev:r.selection_rev,bbox_um:null};}
                    if(r.kind==='records'){data={rows:records.filter(v=>r.errors.includes(v.local))};}
                    if(r.kind==='geometry'){const v=records.find(v=>v.local===r.error);data={...v,start:'0',total:v.points,next:null,precision:'1000',...(format==='ice'?{points_dbu:v.kind==='p'?[['10000','10000'],['30000','10000'],['30000','30000']]:[['40000','10000'],['60000','30000']]}:{points_um:v.kind==='p'?[['10.000125','10'],['30.000125','10'],['30.000125','30']]:[['40.000125','10'],['60.000125','30']]})};}
                    if(r.kind==='focus'){assert.equal(r.isolate,false);assert.equal(body.state_rev,c.state_rev);data={check:r.check,local:r.error,navigation:{kind:'goto',center_um:['20','20'],width_um:'64'}};}
                }
            }return {view_id:c.view_id,revision:c.drc.revision,data:clone(data)};
        }finally{active--;}
    }
    const ctx=new Proxy({}, {get(target,k){return target[k]||((...args)=>drawing.push([k,...args]));},set(t,k,v){t[k]=v;return true;}});
    const panelUI=D.bind({el,document:{createElement:()=>new Element()},protocol:P,geometry:G,selection,steps:require('./guest-drc-step.js'),http,context:()=>context,repaint(){},
        navigate(n,c){if(!context||context.mode!=='explore'||context.pending||context.state_rev!==c.state_rev){return false;}moves.push(n);return true;}});
    const busy=()=>el('gd-filter').disabled;
    async function settle(){for(let i=0;i<80&&busy();i++){await tick();}assert(!busy(),el('gd-status').textContent);}
    return {panelUI,el,calls,moves,drawing,a,b,settle,busy,context(v){context=v;},getContext:()=>context,
        fail(){failedSelection=true;},hold(kind){hold={kind};return hold;},max:()=>maxActive,stepReply(v){stepReply=v;},
        paint(rect={left:0,top:0,width:64,height:32}){drawing.length=0;panelUI.paint(ctx,G.projection({bbox_dbu:['0','0','64','32'],width:64,height:32},[0,0],'1'),[64,32],rect);}};
}
(async()=>{
    for(const format of ['ice','ascii']){
        const e=environment('explore',format);assert.equal(e.calls.length,0);e.panelUI.changed();await e.settle();
        assert.equal(e.el('gd-rules').children[0].textContent,'<script>RULE</script> · 2 errors');assert.equal(e.el('gd-description').textContent,'<img src=x> rule description');
        assert.equal(e.el('gd-errors').children.length,2);e.el('gd-errors').children[0].onclick({});await e.settle();
        assert.equal(e.moves.length,0,'selection must not move');assert.match(e.el('gd-selection').textContent,/1 selected/);e.paint();assert(e.drawing.some(v=>v[0]==='closePath'));
        assert(e.drawing.some(v=>v[0]==='moveTo'&&v[1]===(format==='ascii'?10.000125:10)));
        e.el('gd-go').onclick();await e.settle();assert.deepEqual(e.moves[0],{kind:'goto',center_um:['20','20'],width_um:'64'});
        e.el('gd-errors').children[1].onclick({shiftKey:true});await e.settle();assert.match(e.el('gd-selection').textContent,/2 selected/);e.paint();assert(!e.drawing.some(v=>v[0]==='closePath'),'edge must not become polygon');
        e.el('gd-selected-only').checked=true;e.el('gd-selected-only').onchange();await e.settle();assert.equal(e.el('gd-errors').children.length,2);
        e.el('gd-clear').onclick();await e.settle();assert.equal(e.el('gd-errors').children.length,0);
        e.el('gd-selected-only').checked=false;e.el('gd-selected-only').onchange();await e.settle();
        // Restore this guest's private selection and panel; never repeat Go.
        e.panelUI.reset();e.panelUI.changed();await e.settle();assert.match(e.el('gd-selected').textContent,/9007199254740995/);assert.equal(e.moves.length,1);
        // A committed selection whose response was lost is not replayed.
        const before=e.calls.filter(c=>c.method==='POST'&&c.path==='/drc/selection').length;e.fail();e.el('gd-errors').children[0].onclick({ctrlKey:true});
        for(let i=0;i<10;i++){await tick();}assert(e.busy());assert.match(e.el('gd-status').textContent,/unconfirmed/);
        e.el('gd-reload').onclick();await e.settle();assert.equal(e.calls.filter(c=>c.method==='POST'&&c.path==='/drc/selection').length,before+1);
        // Late focus cannot move a changed camera, even on the same grant.
        const delayed=e.hold('focus');e.el('gd-go').onclick();await tick();await tick();e.context({...e.getContext(),state_rev:'2'});e.panelUI.changed();delayed.resolve();await e.settle();assert.equal(e.moves.length,1);
        e.el('gd-in-view').checked=true;e.el('gd-in-view').onchange();await e.settle();
        const changed=e.hold('list');e.context({...e.getContext(),state_rev:'3'});e.panelUI.changed();await tick();await tick();
        e.context({...e.getContext(),state_rev:'4'});e.panelUI.changed();const conflict=Error('HTTP 409');conflict.status=409;changed.reject(conflict);await e.settle();
        assert.equal(e.calls.filter(c=>c.body&&c.body.body.kind==='list').at(-1).body.state_rev,'4','view-fenced rejection follows latest camera');
        assert.equal(e.max(),1,'one active guest DRC request');
        // Revoked context clears all rows and any eventual late outline.
        const late=e.hold('geometry');e.el('gd-errors').children[0].onclick({});for(let i=0;i<8&&!late.resolve;i++){await tick();}
        assert(late.resolve);e.context(null);e.panelUI.changed();late.resolve();await tick();assert(e.el('gd-panel').hidden);assert.equal(e.el('gd-errors').children.length,0);e.paint();assert.equal(e.drawing.length,0);
    }
    const f=environment('follow');f.panelUI.changed();await f.settle();f.el('gd-errors').children[0].onclick({});await f.settle();
    assert(f.el('gd-go').disabled&&f.el('gd-fit').disabled);f.el('gd-go').onclick();await tick();assert.equal(f.moves.length,0);assert(!f.calls.some(v=>v.body&&v.body.body&&v.body.body.kind==='focus'));
    const no=environment();no.context({...no.getContext(),drc:null});no.panelUI.changed();await tick();assert.equal(no.calls.length,0);
    for(const mode of ['follow','explore']){
        const e=environment(mode);e.panelUI.changed();await e.settle();
        e.el('gd-step-next').onclick();await e.settle();assert.match(e.el('gd-selected').textContent,/9007199254740994/);assert.equal(e.moves.length,0);
        assert(!e.calls.some(c=>c.method==='POST'&&c.path==='/drc/selection'),'step is not a group edit');
        e.panelUI.key('Tab',false);await e.settle();assert.match(e.el('gd-selected').textContent,/9007199254740995/);
        e.stepReply({hit:null,next:{next:'9007199254741000',remaining:'9007199254740000'},scanned:'262144',bbox_um:null,selection_rev:null});
        e.el('gd-step-prev').onclick();await e.settle();assert(!e.el('gd-step-continue').hidden);assert.match(e.el('gd-step-status').textContent,/incomplete/);
        const count=e.calls.length;for(let i=0;i<8;i++){await tick();}assert.equal(e.calls.length,count,'never drain search automatically');
        e.el('gd-step-continue').onclick();await e.settle();const step=e.calls.filter(c=>c.body&&c.body.body.kind==='filtered_step').at(-1).body.body;
        assert.equal(step.backwards,true);assert.equal(step.cursor.remaining,'9007199254740000');assert(e.el('gd-step-continue').hidden);
        // Native device markers projected through fractional CSS origin/DPR.
        const rect={left:10.25,top:20.5,width:32,height:16};e.paint(rect);
        assert.equal(e.panelUI.click(20.25,26.5,false,{}),true);await e.settle();assert.match(e.el('gd-selection').textContent,/1 selected/);
        assert.equal(e.panelUI.click(9,26.5,false,{}),false,'letterbox outside the painted overlay');
        e.el('gd-box').onclick();assert(e.panelUI.active());e.paint(rect);
        assert(e.panelUI.click(29.25,21,false,{}));e.panelUI.move(40.75,35.5);e.paint(rect);assert(e.drawing.some(v=>v[0]==='strokeRect'));
        assert(e.panelUI.click(40.75,35.5,false,{}));await e.settle();
        const box=e.calls.filter(c=>c.path==='/drc/selection'&&c.method==='POST').at(-1).body;
        assert.equal(box.state_rev,'1');assert.deepEqual(box.body.errors,[e.a.local,e.b.local]);assert.deepEqual(box.body.bbox_um,['38','2','61','31']);
        assert.match(e.el('gd-selection').textContent,/1 selected/);assert.equal(e.moves.length,0);
        // Any accepted camera change retires the half-built box and hit table.
        e.paint(rect);e.panelUI.click(29.25,21,false,{});e.context({...e.getContext(),state_rev:'2'});e.panelUI.changed();assert(!e.panelUI.active());
        assert.equal(e.panelUI.click(20.25,26.5,false,{}),false);
        // A double click during an asynchronous first click preserves the row
        // node and queues only focus intent, never a second toggle command.
        const slow=e.hold('geometry'),button=e.el('gd-errors').children[0];button.onclick({ctrlKey:true});
        for(let i=0;i<12&&!slow.resolve;i++){await tick();}assert(slow.resolve);assert.equal(e.el('gd-errors').children[0],button);assert.equal(button.disabled,false);
        const before=e.calls.filter(c=>c.path==='/drc/selection'&&c.method==='POST').length;
        button.onclick({detail:2,ctrlKey:true});button.ondblclick({ctrlKey:true});slow.resolve();await e.settle();
        assert.equal(e.calls.filter(c=>c.path==='/drc/selection'&&c.method==='POST').length,before);assert.equal(e.moves.length,mode==='explore'?1:0);
        const fast=e.el('gd-errors').children[0];fast.onclick({ctrlKey:true});await e.settle();
        const fastCount=e.calls.filter(c=>c.path==='/drc/selection'&&c.method==='POST').length;
        fast.onclick({detail:2,ctrlKey:true});fast.ondblclick({ctrlKey:true});await e.settle();
        assert.equal(e.calls.filter(c=>c.path==='/drc/selection'&&c.method==='POST').length,fastCount,'fast first click must not toggle twice either');
        const searching=e.hold('filtered_step');e.el('gd-step-next').onclick();for(let i=0;i<8&&!searching.resolve;i++){await tick();}
        assert(searching.resolve);assert(e.panelUI.key('Escape'));searching.resolve();await e.settle();assert.match(e.el('gd-status').textContent,/stopped/);
        // Box selection is a mutation: a lost reply cannot be replayed, even
        // when the camera changes concurrently (unlike a current-view read).
        e.paint(rect);e.el('gd-box').onclick();e.panelUI.click(11,21,false,{});e.fail();e.panelUI.click(40,35,false,{});
        e.context({...e.getContext(),state_rev:'3'});e.panelUI.changed();for(let i=0;i<12;i++){await tick();}
        assert(e.busy());assert.match(e.el('gd-status').textContent,/unconfirmed/);
    }
    console.log('WEB GUEST DRC UI: ALL OK (explicit grant only, ICE/ASCII geometry, plain text/u64, independent selection/panel, follow cannot move, stale/revoke, uncertain update/no replay)');
})().catch(e=>{console.error(e);process.exitCode=1;});
