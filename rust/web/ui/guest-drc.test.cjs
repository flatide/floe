'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs');
const D=require('./guest-drc.js'),G=require('./drc-geometry.js'),P=require('./protocol.js'),selection=require('./drc-groups.js');
const tick=()=>new Promise(r=>setImmediate(r));
const clone=v=>JSON.parse(JSON.stringify(v));
function environment(mode='explore',format='ice'){
    const nodes=new Map(),calls=[],drawing=[],moves=[];let context={view_id:'view-a',epoch:'epoch1',state_rev:'1',mode,pending:false,drc:{id:'approved',revision:'rev1'}};
    let panel=null,panelRev='1',selectionRev='1',ids=[],failedSelection=false,hold=null,active=0,maxActive=0;
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
                        if(v.kind==='clear_all'){ids=[];}else if(v.mode==='replace'){ids=v.errors.slice();}else if(v.mode==='add'){ids=[...new Set([...ids,...v.errors])];}else{for(const id of v.errors){ids=ids.includes(id)?ids.filter(v=>v!==id):[...ids,id];}}
                        selectionRev=P.next(selectionRev);if(failedSelection){failedSelection=false;throw Error('reply lost');}}
                    data=selected();
                }else{const r=body.body;assert(['rules','rule','list','records','geometry','focus'].includes(r.kind));
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
    const panelUI=D.bind({el,document:{createElement:()=>new Element()},protocol:P,geometry:G,selection,http,context:()=>context,repaint(){},
        navigate(n,c){if(!context||context.mode!=='explore'||context.pending||context.state_rev!==c.state_rev){return false;}moves.push(n);return true;}});
    const busy=()=>el('gd-filter').disabled;
    async function settle(){for(let i=0;i<80&&busy();i++){await tick();}assert(!busy(),el('gd-status').textContent);}
    return {panelUI,el,calls,moves,drawing,a,b,settle,busy,context(v){context=v;},getContext:()=>context,
        fail(){failedSelection=true;},hold(kind){hold={kind};return hold;},max:()=>maxActive,
        paint(){drawing.length=0;panelUI.paint(ctx,G.projection({bbox_dbu:['0','0','64','32'],width:64,height:32},[0,0],'1'),[64,32]);}};
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
    console.log('WEB GUEST DRC UI: ALL OK (explicit grant only, ICE/ASCII geometry, plain text/u64, independent selection/panel, follow cannot move, stale/revoke, uncertain update/no replay)');
})().catch(e=>{console.error(e);process.exitCode=1;});
