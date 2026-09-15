'use strict';
// Real DRC navigation, editor and display modules; all input is synthetic.
const assert=require('node:assert/strict'),fs=require('node:fs'),D=require('./drc.js'),P=require('./protocol.js');
const F=require('./drc-notes.test.cjs'),N=require('./drc-note-display.test.cjs'),clone=v=>JSON.parse(JSON.stringify(v));
const readonly=process.argv.includes('--read-only');
const ids=new Set([...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
const nodes=new Map(),calls=[],moves=[],saves=[],drawing=[],timers=new Map(),raf=new Map();
let serial=0,restore=null,hold=false,ack=null,displayHold=null,holdDisplay=false,noteText='saved <script> 한글';
const ctx=new Proxy({measureText:s=>({width:s.length*6})},{get:(t,k)=>k in t?t[k]:(...a)=>drawing.push([k,...a])});
class Element {
    constructor(){this.children=[];this.style={};this.value='';this.hidden=false;this.checked=false;this.width=this.height=1;}
    set textContent(v){this.text=v;this.children=[];}get textContent(){return this.text||'';}
    set innerHTML(_){throw Error('HTML injection');}appendChild(v){this.children.push(v);return v;}setAttribute(k,v){this[k]=v;}
    focus(){}scrollIntoView(){}getContext(){return ctx;}
    getBoundingClientRect(){return {left:0,top:0,width:200,height:160};}
}
const el=id=>{assert(ids.has(id),'missing HTML '+id);if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
el('drc-waived').value='all';el('drc-markers').checked=true;
const view={id:F.context.view_id,source:'source',connected:true,pending:false,state:{connection_epoch:'e'.repeat(64),state_rev:'1',status:'idle',dbu_um:'1',bbox_dbu:['0','0','200','160'],pixels:[200,160]}};
const cat={drc:{id:F.context.drc_id,revision:F.context.revision,source_id:'source',title:'synthetic.db',phase:'ready',metadata:{checks:'2',errors:'4',format:'ice'}},notes:F.catalog()};
cat.notes.editable=!readonly;
const row=(ci='0',ei='0')=>({check:ci,local:ei,global:ei==='0'?'1':'2',kind:'p',status:0,bbox_um:ei==='0'?['10','10','70','50']:['90','50','160','70'],points:'4'});
async function http(method,path,body,missing,t){
    calls.push({method,path,body,t});const api='/api/v1/drc/review/notes';
    if(path==='/api/v1/drc')return clone(cat);
    if(path===api){assert.equal(method,'GET','no publication in display test');return clone(cat.notes);}
    if(path===api+'/display'){
        const s={...cat.notes,body},v=N.reply(s,body.focus&&body.focus.error==='0'?noteText:'second note');
        if(holdDisplay)return new Promise(r=>{displayHold=()=>r(v);});return v;
    }
    if(path===api+'/read')return F.snapshot(body.context,body.errors.length,{review_rev:cat.notes.review_rev});
    if(path===api+'/prepare')return F.prepared(body.context,1,body.text,{review_rev:cat.notes.review_rev});
    if(path===api+'/revoke')return null;
    if(path.endsWith('/selection'))return {revision:cat.drc.revision,view_id:view.id,state:{selection_rev:'1',total:'0',limit:5000,rules:[]}};
    const q=body.body;
    if(q.kind==='rules')return {rows:[{check:'0',name:'WIDTH',errors:'2',waived:'0'},{check:'1',name:'SPACE',errors:'2',waived:'0'}],next:null};
    if(q.kind==='rule')return {name:'WIDTH',description:'Synthetic',errors:'2',waived:'0'};
    if(q.kind==='list')return {rows:[row(q.check),row(q.check,'1')],next:null,scanned:'2',bbox_um:null,selection_rev:null};
    if(q.kind==='comparison')return {...row(q.check,q.error),comparison:null};
    if(q.kind==='geometry')return {...row(q.check,q.error),precision:'1',points_dbu:[['10','10'],['70','10'],['70','50'],['10','50']].slice(0,q.limit),total:'4',start:'0',next:q.limit===1?'1':null};
    if(q.kind==='focus')return require('./test-focus.cjs').reply(q,{navigation:{kind:'goto',center_um:['40','30'],width_um:'200'}});
    if(q.kind==='measurements')return {check:q.check,local:q.error,global:P.next(q.error),segments:[]};
    throw Error('Unexpected read '+q.kind);
}
const panel=D.bind({document:{getElementById:el,createElement:()=>new Element()},
    window:{requestAnimationFrame:fn=>{raf.set(++serial,fn);return serial;},cancelAnimationFrame:id=>raf.delete(id)},
    protocol:P,rulers:require('./rulers.js'),groups:require('./drc-groups.js'),notes:require('./drc-notes.js'),noteDisplay:require('./drc-note-display.js'),
    http,context:()=>view,session:()=>'f'.repeat(64),loadNotePending:()=>null,saveNotePending(){},now:()=>100,
    setTimeout:(fn,ms)=>{timers.set(++serial,{fn,ms});return serial;},clearTimeout:id=>timers.delete(id),
    navigate:(nav,token,callback)=>{moves.push(nav);if(hold)ack=callback;else callback(null);return ()=>{};},resize(){},
    stateStore:{bind:o=>({attach:()=>o.apply(restore),change:v=>saves.push(clone(v)),close(){}})}});
const tick=async()=>{for(let i=0;i<180;i++)await Promise.resolve();};
const reads=()=>calls.filter(r=>r.path.endsWith('/display')).length;
const p=D.projection({bbox_dbu:['0','0','200','160'],width:400,height:320},[0,0],'1'),size={pixels:[400,320],dpr:2,left:0,top:0};
const paint=()=>{drawing.length=0;panel.paint(p,size);};
async function jump(i){el('drc-errors').children[i].ondblclick();await tick();}
(async()=>{
    await panel.init();await tick();assert.match(el('drc-errors').children[0].textContent,/^\* #1/);assert(el('drc-note-body').hidden);
    const before=reads();el('drc-errors').children[0].onclick();await tick();assert.equal(reads(),before,'single selection read focus');
    hold=true;await jump(0);assert.equal(el('drc-note-text').textContent,'','unacknowledged jump changed note');ack(null);hold=false;await tick();
    assert.equal(el('drc-note-text').textContent,noteText);assert.deepEqual(saves.at(-1).note_target,{check:'0',error:'0'});paint();
    assert(drawing.some(v=>v[0]==='fillText'&&v[1].includes('saved <script>')));
    const savedReads=reads();hold=true;await jump(1);ack('Synthetic rejected navigation');hold=false;await tick();
    assert.equal(reads(),savedReads);assert.equal(el('drc-note-text').textContent,noteText,'failed jump replaced the previous note');
    const n=reads();view.state.state_rev='2';view.pending=true;panel.contextChanged();paint();await tick();view.pending=false;assert.equal(reads(),n,'pan read saved notes');
    // Canvas single-select never changes the last jumped target.
    panel.paint(p,size);assert(panel.click(125,100,false));await tick();assert.equal(saves.at(-1).selected.error,'1');assert.equal(saves.at(-1).note_target.error,'0');
    assert.equal(reads(),n);assert.equal(el('drc-note-text').textContent,noteText);
    restore=saves.at(-1);const nav=moves.length;await el('drc-reload').onclick();await tick();assert.equal(moves.length,nav);assert.equal(el('drc-note-text').textContent,noteText);
    assert.equal(saves.at(-1).note_target.error,'0');
    el('drc-markers').checked=false;el('drc-markers').onchange();paint();assert(!el('drc-canvas').hidden);assert(drawing.some(v=>v[0]==='fillText'));
    panel.overlayMode('focus');assert(!el('drc-canvas').hidden);panel.overlayMode('none');assert(el('drc-canvas').hidden);panel.overlayMode('all');assert(!el('drc-canvas').hidden);
    panel.key('K');assert.equal(el('drc-note-text').textContent,noteText,'ruler clear removed note');
    // Snapshot uses the same DRC canvas, with a synchronous flush.
    drawing.length=0;panel.flush();assert(drawing.some(v=>v[0]==='fillText'));
    const app=fs.readFileSync(__dirname+'/app.js','utf8');assert(app.includes("['query-canvas','drc-canvas','ruler-canvas']"));assert(app.includes('drcPanel.flush()'));
    if(readonly){
        assert.equal(el('drc-review-mode').textContent,'READ-ONLY REVIEWER');assert(el('notes-authoring').hidden);
        assert(!el('notes-panel').hidden);assert(!el('drc-saved-notes').hidden);assert(el('notes-autosave').disabled);
        const before=calls.length;panel.key('n');panel.key('w');await tick();assert.equal(calls.length,before,'read-only shortcuts started editor IO');
        assert(calls.filter(r=>r.path.startsWith('/api/v1/drc/review/')).every(r=>r.method==='GET'||r.path.endsWith('/display')));
        view.connected=false;panel.contextChanged();await tick();assert.equal(el('drc-note-text').textContent,'');
        view.connected=true;view.state.connection_epoch='7'.repeat(64);panel.contextChanged();await tick();
        assert.equal(el('drc-note-text').textContent,noteText);assert(el('notes-autosave').disabled);
        panel.stop();await tick();assert.equal(timers.size,0);assert.equal(raf.size,0);
        console.log('WEB READ REVIEWER PANEL: ALL OK (real DRC/editor/display; badges/body/canvas/restore, no pan reads or editor IO, reconnect, cleanup)');return;
    }
    // Display must leave an editor/preview intact, then refresh after explicit reload.
    await el('notes-read').onclick();el('notes-text').value='UNSAVED';el('notes-text').oninput();await el('notes-prepare').onclick();await tick();
    assert(!el('notes-review').hidden);el('drc-notes-refresh').onclick();await tick();assert(!el('notes-review').hidden);assert.equal(el('notes-text').value,'UNSAVED');
    el('notes-discard').onclick();holdDisplay=true;el('drc-notes-refresh').onclick();await tick();const late=displayHold;
    cat.notes=F.catalog([F.op('1','publishing')]);await panel.refresh();assert.equal(el('drc-note-text').textContent,'');
    cat.notes=F.catalog([F.op('1','succeeded')]);noteText='after saved revision';holdDisplay=false;await panel.refresh();late();await tick();assert.equal(el('drc-note-text').textContent,noteText);
    cat.notes=F.catalog([F.op('1','succeeded'),F.op('2','failed',{review_rev:'2',outcome_unknown:true,published:null,error:'review_unavailable'})]);
    await panel.refresh();await tick();assert.equal(el('drc-note-text').textContent,'');assert.match(el('drc-notes-status').textContent,/unconfirmed/);
    panel.stop();await tick();assert.equal(timers.size,0);assert.equal(raf.size,0);assert.equal(el('drc-note-text').textContent,'');
    assert(!JSON.stringify(saves).includes('UNSAVED'));assert(!JSON.stringify(saves).includes('saved <script>'));
    console.log('WEB DRC NOTE DISPLAY PANEL: ALL OK (real notes/navigation, ACK target, independent selection/restore, markers/Tab/rulers, snapshot canvas, no pan reads, preview preserved, publication fences)');
})().catch(e=>{panel.stop();console.error(e);process.exitCode=1;});
