'use strict';
const assert=require('node:assert/strict'),D=require('./drc-note-display.js'),P=require('./protocol.js');
const clone=v=>JSON.parse(JSON.stringify(v)),tick=async()=>{for(let i=0;i<40;i++)await Promise.resolve();};
const source={reviewer:'owner',review_rev:'0',read_turn:0,epoch:'e'.repeat(64),blocked:'',body:{
    context:{drc_id:'a'.repeat(64),revision:'b'.repeat(64),view_id:'c'.repeat(64)},
    errors:[{check:'2',error:'9007199254740993'},{check:'0',error:'0'}],focus:{check:'2',error:'9007199254740993'}}};
function reply(s=source,text='saved <script> 한글'){return {kind:'drc_note_display',context:clone(s.body.context),review_rev:s.review_rev,reviewer:s.reviewer,
    name:'.synthetic.notes.owner.fe',rows:s.body.errors.map((r,i)=>({...r,noted:i===0})),focus:s.body.focus?{...s.body.focus,text}:null,
    exists:true,legacy_unverified:false,cache_hit:true,import_report:{skipped_lines:0,invalid_members:0,reassigned_members:0}};}
async function main(){
    D.validate(reply(),source,P);
    for(const mutate of [v=>v.rows.reverse(),v=>v.rows.pop(),v=>v.rows[0].noted=1,v=>v.focus.error='0',v=>v.context.revision='wrong',
        v=>v.reviewer='other',v=>v.review_rev='1',v=>v.path='secret',v=>v.import_report.skipped_lines=-1,v=>v.focus.text='한'.repeat(22000)]){
        const v=reply();mutate(v);assert.throws(()=>D.validate(v,source,P));
    }
    let s=clone(source),resolve=null,mode='success';const calls=[];
    const d=D.bind({protocol:P,source:()=>s,changed(){},http:async(method,path,body,missing,t)=>{
        assert.equal(method,'POST');assert.equal(path,'/api/v1/drc/review/notes/display');calls.push({body:clone(body),t});const v=reply(s);
        if(mode==='hold')return new Promise(r=>{resolve=()=>r(v);});if(mode==='error')throw Object.assign(new Error('changed'),{code:'review_changed'});return v;}});
    d.sync();await tick();assert.equal(calls.length,1);assert.equal(d.noted(source.body.errors[0]),true);assert.equal(d.noted(source.body.errors[1]),false);
    for(let i=0;i<100;i++)d.sync();await tick();assert.equal(calls.length,1,'same identity reread');
    // Slow old revision cannot return during an approved save, nor after it.
    mode='hold';d.sync(true);await tick();const old=resolve;
    s.blocked='publication pending';d.sync();assert.equal(d.text(),null);s={...s,blocked:'',review_rev:'1'};d.sync();await tick();assert.equal(calls.length,2,'concurrent read');
    mode='success';old();await tick();assert.equal(calls.length,3);assert.match(d.message(),/revision 1/);
    s.epoch='f'.repeat(64);d.sync();await tick();assert.equal(calls.length,4);
    mode='error';d.sync(true);await tick();assert.equal(d.noted(source.body.errors[0]),null);assert.match(d.message(),/unavailable.*review_changed/);
    for(let i=0;i<20;i++)d.sync();await tick();assert.equal(calls.length,5,'error retried without input');
    mode='success';s.read_turn++;d.sync();await tick();assert(d.text(),'explicit edit read did not invalidate display');
    let measured=0;const drawing=[];const ctx=new Proxy({measureText:t=>{measured++;return {width:t.length*6};}},{get:(t,k)=>k in t?t[k]:(...a)=>drawing.push([k,...a])});
    d.paint(ctx,{pixels:[400,320],dpr:2});assert(drawing.some(r=>r[0]==='fillText'&&r[1].includes('<script>')));const n=measured;
    d.paint(ctx,{pixels:[400,320],dpr:2});assert.equal(measured,n,'pan rewrapped text');
    d.stop();assert.equal(d.text(),null);await tick();const stopped=calls.length;d.sync(true);await tick();assert.equal(calls.length,stopped);
    d.resume();await tick();assert.equal(calls.length,stopped+1);d.stop();
    // Long Unicode notes are bounded by the visible prefix, with an explicit warning.
    const long=D.bind({protocol:P,source:()=>source,changed(){},http:async()=>reply(source,'한😀\n'.repeat(5000))});long.sync();await tick();
    drawing.length=0;measured=0;long.paint(ctx,{pixels:[400,160],dpr:2});
    assert(measured<100);assert(drawing.some(r=>r[0]==='fillText'&&r[1].includes('note clipped')));assert.equal(long.text().length,20000);long.stop();
    const zero=D.bind({protocol:P,source:()=>source,changed(){},http:async()=>reply(source,'\u0301'.repeat(30000))});zero.sync();await tick();
    let maxRun=0;measured=0;ctx.measureText=t=>{measured++;maxRun=Math.max(maxRun,Array.from(t).length);return {width:0};};
    zero.paint(ctx,{pixels:[400,160],dpr:2});assert(maxRun<=128);assert(measured<=256,'zero-width text shaping was unbounded');zero.stop();
    let empty={...clone(source),body:{context:clone(source.body.context),errors:[],focus:null}},emptyReads=0;
    const quiet=D.bind({protocol:P,source:()=>empty,changed(){},http:async()=>{emptyReads++;const v=reply(empty,null);
        v.legacy_unverified=true;v.import_report.skipped_lines=2;return v;}});
    quiet.sync();await tick();assert.equal(emptyReads,0);assert.match(quiet.message(),/Choose an error page/);
    empty.body.errors=[{check:'0',error:'0'}];quiet.sync();await tick();assert.equal(emptyReads,1);assert.equal(quiet.text(),null);
    assert.match(quiet.message(),/UNVERIFIED.*import loss: skipped_lines: 2/);empty=null;quiet.sync();assert(!quiet.visible());quiet.stop();
    console.log('WEB DRC NOTE DISPLAY: ALL OK (strict schema, serialized reads, revisions/epoch, no pan reads, unknown/error invalidation, bounded Unicode wrap, no writes)');
}
if(require.main===module)main().catch(e=>{console.error(e);process.exitCode=1;});
module.exports={reply,source};
