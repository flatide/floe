'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const Palette = require('./palette.js');
const tick = () => new Promise(setImmediate);
const event = extra => Object.assign({button:0,detail:1,preventDefault(){},stopPropagation(){}},extra);
function pairKey(p) { return p.join('/'); }
function fixture(n=70, group=true) {
    return Array.from({length:n}, (_,l) => (group?[1,2,300]:[0]).map((d,i) => ({pair:[l,d], name:l===0&&i===0?'<mask & text>':'L'+l+'.'+d,aliases:['plain <alias>'],head:group&&i===0,parent:group&&i?[l,1]:null,children:group&&i===0?2:0,closed:false,visible:true,color:'#abcdef',fill:{kind:'solid'},width:1}))).flat();
}
class Element {
    constructor(tag='div') {this.tag=tag;this.children=[];this.dataset={};this.style={};this.disabled=false;this.hidden=false;this._text='';this.attributes={};}
    set textContent(v){this._text=String(v);this.children=[];}
    get textContent(){return this._text;}
    appendChild(c){this.children.push(c);return c;}
    contains(n){return this===n||this.children.some(c=>c.contains(n));}
    setAttribute(k,v){this.attributes[k]=v;}
    getAttribute(k){return this.attributes[k];}
    getBoundingClientRect(){return {left:20,top:20,right:200,bottom:40};}
    focus(){this.document.activeElement=this;}
}
function harness(rows=fixture()) {
    const nodes = new Map(), requests=[], edits=[], events={}, calls={paints:0};
    const document={activeElement:null,createElement(tag){const e=new Element(tag);e.document=document;return e;},addEventListener(k,fn){events[k]=fn;}};
    function el(id){if(!nodes.has(id)){nodes.set(id,document.createElement('div'));}return nodes.get(id);}
    let state={id:'a'.repeat(64),key:'1',connected:true,editable:true};
    const port={el,document,window:{innerWidth:300,innerHeight:240},context:()=>state,
        http(method,path,body,missing,token){
            assert.equal(method,'POST');assert(path.endsWith('/palette'));
            return new Promise((resolve,reject)=>{const q={path,body,token,resolve,reject,key:state.key,done:false};requests.push(q);token.abort=()=>{q.aborted=true;};});
        },edit(body,done){edits.push({body,done});},
        styles(r,s,valid){const color=document.createElement('input'),style=document.createElement('button');color.valid=style.valid=valid;return {color,style};},
        painted(){calls.paints++;}};
    const panel=Palette.bind(port);
    function response(q) {
        const closed=k=>q.body.fold.closed!==q.body.fold.exceptions.some(p=>pairKey(p)===k);
        const visible=rows.filter(r=>!r.parent||!closed(pairKey(r.parent)));
        if(q.body.kind==='page') {
            const start=q.body.start,part=visible.slice(start,start+64).map(r=>({...r,closed:r.children>0&&closed(pairKey(r.pair))}));
            return {state_rev:'1',render_key:q.key,total:visible.length,all_total:rows.length,start,next:start+part.length<visible.length?start+part.length:null,rows:part};
        }
        const a=visible.findIndex(r=>pairKey(r.pair)===pairKey(q.body.first)),b=visible.findIndex(r=>pairKey(r.pair)===pairKey(q.body.last));
        assert(a>=0&&b>=0);const part=visible.slice(Math.min(a,b),Math.max(a,b)+1);
        return {state_rev:'1',render_key:q.key,pairs:part.map(r=>r.pair),groups:part.filter(r=>r.children).map(r=>r.pair)};
    }
    async function reply(q=requests.find(q=>!q.done&&!q.aborted),value) {assert(q);q.done=true;q.resolve(value||response(q));await tick();}
    async function fail(message,q=requests.find(q=>!q.done&&!q.aborted)){q.done=true;q.reject(Error(message));await tick();}
    function row(k){const v=el('layers').children.find(r=>r.children[3].children[0].dataset.pair===k);assert(v,'row '+k+' missing');return v;}
    function click(k,e){row(k).children[3].onclick(event(e));}
    async function complete(error=null,bump=true){const v=edits.at(-1);assert(v);if(bump){state.key=String(Number(state.key)+1);}v.done(error);await tick();}
    return {el,document,events,requests,edits,calls,panel,rows,response,reply,fail,row,click,complete,
        get state(){return state;},set state(v){state=v;},count:()=>Number(el('layers-selected').textContent.split(' ')[0])};
}
async function basic() {
    const h=harness();assert.equal(h.requests.length,1);assert.equal(h.edits.length,0);
    assert.equal(h.el('layers-show').disabled,true);await h.reply();
    assert.equal(h.el('layers').children.length,64);assert.equal(h.el('layers-count').textContent,'1–64 / 210');
    assert.equal(h.row('0/1').children[3].children[0].textContent,'<mask & text>');
    assert.equal(h.row('0/1').children[3].title,'0/1 · <mask & text> · plain <alias>');
    h.click('0/1');assert.equal(h.count(),1);assert.equal(h.row('0/1').children[3].getAttribute('aria-pressed'),'true');
    h.click('0/1');assert.equal(h.count(),0);
    h.click('0/1');h.click('1/1',{ctrlKey:true});assert.equal(h.count(),2);
    h.click('1/2',{metaKey:true});assert.equal(h.count(),3);
    h.click('2/1',{shiftKey:true});assert.equal(h.count(),3);assert.equal(h.requests.length,1,'same-page range made a request');
    h.row('5/1').oncontextmenu(event({button:2,clientX:299,clientY:239}));
    assert.equal(h.count(),3,'context click replaced the prepared selection');
    assert.equal(h.el('layer-menu').hidden,false);assert.equal(h.document.activeElement,h.el('layer-menu-show'));
    assert.equal(h.el('layer-menu').style.left,'110px');
    h.el('layer-menu').onkeydown(event({key:'End'}));assert.equal(h.document.activeElement,h.el('layer-menu-close'));
    h.el('layer-menu').onkeydown(event({key:'Escape'}));assert.equal(h.el('layer-menu').hidden,true);
    assert.equal(h.document.activeElement,h.row('5/1').children[3]);
    assert.equal(h.edits.length,0,'selection/menu wrote view state');
    h.el('layers-clear').onclick();assert.equal(h.count(),0);
    h.click('0/1');h.el('layers-next').onclick();await h.reply();
    assert.equal(h.el('layers-count').textContent,'65–128 / 210');assert.equal(h.count(),1);
    h.click('30/2',{shiftKey:true});assert.equal(h.requests.at(-1).body.kind,'range');
    assert.deepEqual(h.requests.at(-1).body.first,[0,1]);assert.equal(h.el('layers-hide').disabled,true);
    await h.reply();assert.equal(h.count(),92);assert.equal(h.edits.length,0);
    h.el('layers-hide').onclick();assert.equal(h.edits.length,1);
    assert.equal(h.edits[0].body.layer_batch.pairs.length,92);assert.deepEqual(h.edits[0].body.layer_batch.collapsed,[]);
    assert.equal(h.el('layers-show').disabled,true);assert.equal(h.row('30/2').children[1].disabled,true);
    await h.complete();await h.reply();assert.equal(h.count(),92);
    const read=h.requests.length;h.panel.changed();assert.equal(h.requests.length,read,'unchanged policy reloaded layers');
    h.el('layers-collapse').onclick();await h.reply();
    assert.equal(h.count(),92,'folding discarded hidden selection');assert.equal(h.el('layers-count').textContent,'1–64 / 70');
    assert.equal(h.row('0/1').children[2].getAttribute('aria-expanded'),'false');
    h.el('layers-show').onclick();assert.equal(h.edits.length,2);
    assert.deepEqual(h.edits[1].body.layer_batch.collapsed,Array.from({length:31},(_,l)=>[l,1]));
    await h.complete(null,false);assert.equal(h.requests.length,read+1,'no-op started another read');
    h.el('layers-expand').onclick();await h.reply();
    h.row('2/1').children[2].onclick();assert.deepEqual(h.requests.at(-1).body.fold,{closed:false,exceptions:[[2,1]]});await h.reply();
    assert.equal(h.el('layers-count').textContent,'1–64 / 208');assert.equal(h.row('2/1').children[2].getAttribute('aria-expanded'),'false');
    const check=h.row('2/1').children[0];check.checked=false;check.onchange();
    assert.deepEqual(h.edits.at(-1).body,{layer_batch:{action:'hide',pairs:[[2,1]],collapsed:[[2,1]]}});
    await h.complete('Rejected by current state',false);assert.match(h.el('layers-note').textContent,/Rejected/);
    h.el('layers-all').onclick();assert.deepEqual(h.edits.at(-1).body,{layers:{mode:'all'}});await h.complete(null,false);
    h.el('layer-menu-none').onclick();assert.deepEqual(h.edits.at(-1).body,{layers:{mode:'none'}});await h.complete(null,false);
    h.panel.stop();assert.equal(h.el('layers-all').disabled,true);
}
async function anchorAndDouble() {
    const h=harness();await h.reply();h.click('0/2');h.el('layers-collapse').onclick();await h.reply();
    const read=h.requests.length;h.click('4/1',{shiftKey:true});assert.equal(h.count(),1);
    assert.equal(h.requests.length,read,'hidden anchor was sent as a range');
    assert.equal(h.row('4/1').dataset.selected,'true');
    h.click('4/1',{detail:1});h.click('4/1',{detail:2});h.row('4/1').children[3].ondblclick(event({detail:2}));await tick();
    assert.equal(h.count(),1);assert.deepEqual(h.edits.at(-1).body,{layer_batch:{action:'toggle',pairs:[[4,1]],collapsed:[[4,1]]}});
    await h.complete();await h.reply();
    h.click('4/1',{detail:1});h.click('4/1',{detail:2});h.row('4/1').children[3].ondblclick(event({detail:2}));await tick();
    assert.equal(h.edits.length,2,'second double-click lost its toggle');await h.complete(null,false);
    h.el('layers-expand').onclick();await h.reply();h.click('0/1');h.el('layers-next').onclick();await h.reply();
    h.click('30/2',{shiftKey:true});const first=h.requests.at(-1);
    h.click('30/2',{shiftKey:true,detail:2});assert(first.aborted);const last=h.requests.at(-1);
    h.row('30/2').children[3].ondblclick(event({detail:2}));await h.reply(first);assert.equal(h.edits.length,2);
    await h.reply(last);assert.equal(h.count(),92);assert.equal(h.edits.length,3,'cross-page shift double-click lost its toggle');
    await h.complete(null,false);h.panel.stop();
}
async function lifecycle() {
    const h=harness();await h.reply();h.click('0/1');h.el('layers-collapse').onclick();const old=h.requests.at(-1);
    h.el('layers-expand').onclick();const fresh=h.requests.at(-1);assert(old.aborted);
    await h.reply(old);await h.reply(fresh);assert.equal(h.el('layers-count').textContent,'1–64 / 210');
    assert.equal(h.count(),1);
    const staleName=h.row('0/1').children[3],staleStyle=h.row('0/1').children[1];
    h.state={id:'b'.repeat(64),key:'1',connected:true,editable:true};h.panel.changed();assert.equal(h.count(),0);
    await h.reply();assert.equal(staleStyle.valid(),false);staleName.onclick(event());assert.equal(h.count(),0);
    h.click('0/1');h.state.connected=false;h.panel.changed();assert.equal(h.count(),1);assert.equal(h.el('layers-show').disabled,true);
    const count=h.requests.length;h.state.connected=true;h.panel.changed();assert.equal(h.requests.length,count,'same-view reconnect reloaded immutable metadata');
    h.state.key='2';h.panel.changed();const q=h.requests.at(-1);h.panel.suspend();assert(q.aborted);
    await h.reply(q);assert.equal(h.row('0/1').children[3].disabled,true);h.panel.resume();await h.reply();assert.equal(h.count(),1);
    h.state.key='3';h.panel.changed();const stop=h.requests.at(-1);h.panel.stop();assert(stop.aborted);await h.reply(stop);
    h.panel.resume();assert.equal(h.el('layers-show').disabled,true);assert.equal(h.edits.length,0);
    // Aborting a page must not make the previous page masquerade as the new
    // offset. Clearing selection need not abort a page at all.
    const p=harness();await p.reply();p.click('0/1');p.el('layers-next').onclick();const next=p.requests.at(-1);
    p.el('layers-clear').onclick();assert(!next.aborted);assert.equal(p.count(),0);await p.reply(next);
    p.el('layers-next').onclick();const interrupted=p.requests.at(-1);p.state.connected=false;p.panel.changed();assert(interrupted.aborted);
    p.state.connected=true;p.panel.changed();assert.notEqual(p.requests.at(-1),interrupted);await p.reply(interrupted);await p.reply();
    assert.equal(p.el('layers-count').textContent,'129–192 / 210');p.panel.stop();
    const w=harness();await w.reply();w.click('0/1');w.el('layers-hide').onclick();const oldWrite=w.edits.at(-1);
    w.state={id:'c'.repeat(64),key:'1',connected:true,editable:true};w.panel.changed();await w.reply();w.click('1/1');
    oldWrite.done('old view failed');assert.equal(w.count(),1);assert.equal(w.el('layers-note').textContent,'');w.panel.stop();
}
async function errors() {
    const h=harness();await h.fail('temporary failure');assert.equal(h.el('layers-retry').hidden,false);const n=h.requests.length;
    h.panel.changed();assert.equal(h.requests.length,n,'failed page retried without user action');
    h.el('layers-retry').onclick();const bad=h.requests.at(-1),value=h.response(bad);value.rows[0].closed=true;await h.reply(bad,value);
    assert.equal(h.calls.paints,0);assert.match(h.el('layers-note').textContent,/Invalid layer group/);
    h.el('layers-retry').onclick();await h.reply();h.click('0/1');h.el('layers-next').onclick();await h.reply();
    h.click('30/2',{shiftKey:true});await h.fail('Range exceeds 4096');assert.equal(h.count(),1);
    h.click('30/2',{shiftKey:true});const range=h.requests.at(-1),r=h.response(range);r.pairs.push([99,1]);await h.reply(range,r);
    assert.match(h.el('layers-note').textContent,/Invalid layer range/);assert.equal(h.count(),1);
    h.click('35/1');h.click('30/2',{shiftKey:true,ctrlKey:true});assert.equal(h.requests.at(-1),range,'same page unexpectedly read a range');
    h.state=null;h.panel.changed();assert.equal(h.count(),0);assert.equal(h.el('layers').children.length,0);
    const big=harness(fixture(5000,false));await big.reply();big.click('0/0');
    // Selection cap is checked on the whole result, not silently truncated.
    big.el('layers-next').onclick();await big.reply();big.click('100/0',{shiftKey:true});const q=big.requests.at(-1);
    await big.reply(q,{render_key:'1',pairs:fixture(4097,false).map(r=>r.pair),groups:[]});
    assert.equal(big.count(),1);assert.match(big.el('layers-note').textContent,/4096/);assert.equal(big.edits.length,0);big.panel.stop();
    assert.throws(()=>Palette.choose(Array.from({length:4096},(_,i)=>i+'/0'),null,'5000/0',{ctrlKey:true},null),/4096/);
}
async function styles() {
    const h=harness();await h.reply();h.click('0/1');h.click('0/2',{ctrlKey:true});
    const reads=h.requests.length;h.el('layers-style').onclick();
    assert.equal(h.el('palette-style').hidden,false);assert.equal(h.el('palette-style-title').textContent,'Style 2 selected rows');
    assert.equal(h.edits.length,0);assert.equal(h.requests.length,reads);
    h.el('palette-style').onsubmit(event());assert.match(h.el('layers-note').textContent,/at least one/);assert.equal(h.edits.length,0);
    h.el('palette-fill').value='pattern';h.el('palette-fill').onchange();assert.equal(h.el('palette-pattern').hidden,false);
    h.el('palette-pattern').value='abcd';h.el('palette-style').onsubmit(event());assert.match(h.el('layers-note').textContent,/16/);assert.equal(h.edits.length,0);
    h.el('palette-pattern').value=new Array(16).fill('1234').join(' ');
    h.el('palette-color-on').checked=true;h.el('palette-color').value='#22aa88';h.el('palette-width').value='+1';
    h.el('palette-style').onsubmit(event());
    assert.deepEqual(h.edits.at(-1).body,{style_batch:{pairs:[[0,1],[0,2]],collapsed:[],color:'#22aa88',fill:{kind:'pattern',rows:new Array(16).fill(0x1234)},width_step:1}});
    assert.equal(h.el('palette-style').hidden,true);assert.equal(h.el('layers-style').disabled,true);
    await h.complete('Rejected',false);assert.equal(h.count(),2);assert.match(h.el('layers-note').textContent,/Rejected/);assert.equal(h.edits.length,1);
    h.el('layers-style').onclick();h.click('1/1');assert.equal(h.el('palette-style').hidden,true,'selection change retained stale style target');
    h.el('palette-style').onsubmit(event());assert.equal(h.edits.length,1);
    h.el('layers-style').onclick();h.el('layers-collapse').onclick();assert.equal(h.el('palette-style').hidden,true);await h.reply();
    h.el('layer-menu-style').onclick();h.el('palette-width').value='1';h.el('palette-style').onsubmit(event());
    assert.deepEqual(h.edits.at(-1).body,{style_batch:{pairs:[[1,1]],collapsed:[[1,1]],width:1}});
    await h.complete(null,false);
    h.el('layers-style').onclick();h.state.key='2';h.panel.changed();assert.equal(h.el('palette-style').hidden,true);await h.reply();
    h.el('layers-style').onclick();h.state.connected=false;h.panel.changed();assert.equal(h.el('palette-style').hidden,true);
    h.state.connected=true;h.panel.changed();h.el('layers-style').onclick();h.el('palette-style-cancel').onclick();assert.equal(h.el('palette-style').hidden,true);
    assert.equal(h.edits.length,2);h.panel.stop();
}
async function main() {
    if(process.argv[2]) {
        const cases=JSON.parse(fs.readFileSync(process.argv[2],'utf8'));
        for(const c of cases){assert.deepEqual(Palette.choose(c.before,c.anchor,c.row,c.event,c.range),c.expected);}
        console.log('GTK WEB PALETTE SELECTION: ALL OK ('+cases.length+' source-derived clicks)');return;
    }
    await basic();await anchorAndDouble();await lifecycle();await errors();await styles();
    // Late range versus a newer local selection (without switching pages).
    const h=harness();await h.reply();h.click('0/1');h.el('layers-next').onclick();await h.reply();h.click('30/2',{shiftKey:true});const q=h.requests.at(-1);
    h.click('35/1');await h.reply(q);assert.equal(h.count(),1);assert.equal(h.row('35/1').dataset.selected,'true');assert.equal(h.el('layers-note').textContent,'');h.panel.stop();
    // Late page versus a newer render policy.
    const p=harness();const a=p.requests[0];p.state.key='2';p.panel.changed();assert(a.aborted);await p.reply(a);assert.equal(p.calls.paints,0);await p.reply();assert.equal(p.calls.paints,1);p.panel.stop();
    console.log('WEB PALETTE: ALL OK (bounded pages/ranges, modifiers/menu/double-click, folded groups, one batch, cancel/stale/failure, reconnect and cleanup)');
}
main().catch(e=>{console.error(e);process.exitCode=1;});
