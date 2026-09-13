'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs');
const C=require('./clip.js'),P=require('./protocol.js'),Q=require('./query.js');
const clone=v=>JSON.parse(JSON.stringify(v));
const entry=(seq,phase='queued')=>({seq,kind:'exact_clip',phase});
const artifact=(id='1')=>({id,bytes:'64',expires_in_ms:'600000',name:'floe-clip-'+id+'.oas'});
function status(history=[],files=[]) {
    const last=history.at(-1),active=last&&!['failed','cancelled','ready'].includes(last.phase)?last.seq:null;
    return {operations:{last_seq:last?last.seq:'0',active,history},available:true,kind:'exact_clip',jobs_default:4,jobs_min:1,jobs_max:16,
        limits:{artifacts:4,artifact_bytes:'536870912',total_bytes:'2147483648',readers:2,ttl_seconds:600},
        usage:{entries:files.length,pending:0,bytes:String(files.length*64),readers:0},artifacts:files};
}
function ready(seq='1') {return {...entry(seq,'ready'),artifact:{...artifact(seq),records:'12',bbox_dbu:['-10','0','90','80'],source_stale:false,available:true}};}
const flush=async()=>{for(let i=0;i<10;i++)await Promise.resolve();};
function harness() {
    const state={view_id:'a'.repeat(64),connection_epoch:'b'.repeat(64),dataset_revision:'1',worker_epoch:'2',state_rev:'3',render_rev:'3',render_key:'1',
        pixels:[200,160],bbox_dbu:['-100','-80','100','80'],dbu_um:'.001',source_stale:false,status:'idle',capabilities:{clip:true},layers:{mode:'only',pairs:[[7,0],[8,0]]}};
    const context={id:state.view_id,state,frame:{...state,frame_id:'4',width:200,height:160,purpose:'foreground',query:false,
        query_scene:{generation:'1',round:'1',complete:false,summary_layers:'2'}},acked:true,connected:true,pending:false,hidden:false,
        origin:[0,0],rect:{left:10,top:20},size:{pixels:[200,160],dpr:2,left:.25,top:.125}};
    let time=0,serial=0,seq='9007199254740992',model=status(),override=null;
    const nodes=new Map(),timers=new Map(),requests=[],sent=[],downloads=[],submissions=[];
    class Element {
        constructor(tag='div'){this.tag=tag;this.children=[];this.dataset={};this.hidden=false;this.disabled=false;this.value='';this.style={};}
        get textContent(){return this.text||'';}set textContent(v){this.text=v;this.children=[];}
        set innerHTML(v){throw new Error('HTML injection: '+v);}
        appendChild(c){this.children.push(c);return c;}removeChild(c){this.children.splice(this.children.indexOf(c),1);return c;}
        setAttribute(k,v){this[k]=v;}focus(){document.activeElement=this;}
        submit(){submissions.push({method:this.method,action:this.action,target:this.target,rel:this.rel,enctype:this.enctype,
            inputs:this.children.map(c=>({type:c.type,name:c.name,value:c.value}))});}
    }
    const ids=[...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]);
    function el(id){assert(ids.includes(id),'missing HTML element '+id);if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);}
    const document={activeElement:null,body:new Element('body'),getElementById:el,createElement:t=>new Element(t)};
    async function http(method,path,body,missing,token) {
        const r={method,path,body,token};requests.push(r);
        if(override) {const v=override(r);if(v!==undefined)return await v;}
        if(method==='GET'&&path==='/api/v1/exports')return clone(model);
        if(method==='POST'&&path==='/api/v1/exports') {
            let op=model.operations.history.find(v=>v.seq===body.seq);
            if(!op){op={...entry(body.seq),view_id:body.view_id};model=status(model.operations.history.concat(op).slice(-32),model.artifacts);}
            return clone(op);
        }
        if(method==='POST'&&path.endsWith('/cancel')) {
            const op=entry(path.split('/')[4],'cancelled');model=status([op],model.artifacts);return clone(op);
        }
        if(method==='DELETE') {model=status(model.operations.history,model.artifacts.filter(f=>f.id!==path.split('/').at(-1)));return {released:true};}
        throw new Error('unexpected request '+method+' '+path);
    }
    const c=C.bind({document,protocol:P,query:Q,context:()=>context,http,now:()=>time,
        setTimeout:(fn,ms)=>{timers.set(++serial,{fn,at:time+ms});return serial;},clearTimeout:id=>timers.delete(id),
        send:v=>{v.seq=seq=P.next(seq);sent.push(clone(v));return seq;},download:id=>downloads.push(id)});
    const prepare=()=>{el('clip-open').onclick();el('clip-form').onsubmit({preventDefault(){}});return sent.at(-1);};
    function reply(t=sent.at(-1),extra={}) {
        const m={type:'clip.prepared',seq:t.seq,view_id:t.view_id,connection_epoch:t.connection_epoch,source_stale:state.source_stale,
            draft:{token:'d'.repeat(64),dataset_revision:t.body.anchor.dataset_revision,bbox_dbu:['-9007199254740993','-80','100','80'],
                layers:t.body.layers==='visible'?(state.layers.mode==='only'?{count:state.layers.pairs.length,mode:'only'}:{mode:state.layers.mode}):{mode:t.body.layers},
                jobs:t.body.jobs,cell_name:t.body.cell_name,expires_in_ms:'30000'},...extra};
        c.receive(m);return m;
    }
    return {c,context,state,document,el,requests,sent,timers,downloads,submissions,prepare,reply,
        init:()=>c.init(true),approve:()=>el('clip-approve').onclick(),
        tick:ms=>{time+=ms;for(const [id,t]of [...timers])if(t.at<=time){timers.delete(id);t.fn();}},
        get model(){return model;},set model(v){model=v;},set override(f){override=f;}};
}
function posts(h){return h.requests.filter(r=>r.method==='POST'&&r.path==='/api/v1/exports');}
(async()=>{
    // Strict bounded receipts and lossless 64-bit coordinates/sequences.
    const id='9007199254740993',s=status([entry(id,'failed')],[artifact('1')]);
    assert.equal(C.catalog(s,P,Q),s,'artifact may outlive operation history');
    assert.equal(C.operation(ready(id),P,Q).artifact.id,id);
    const unavailable=ready();unavailable.artifact.available=false;unavailable.artifact.expires_in_ms=null;C.operation(unavailable,P,Q);
    for(const mutate of [v=>v.artifacts.push(v.artifacts[0]),v=>v.artifacts[0].name='../source.oas',v=>v.artifacts[0].bytes='536870913',
        v=>v.artifacts[0].id=1,v=>v.operations.active=id,v=>v.operations.last_seq='0',v=>v.usage.readers=3,v=>v.path='/private/source',
        v=>v.artifacts[0].expires_in_ms='600001']) {
        const bad=clone(s);mutate(bad);assert.throws(()=>C.catalog(bad,P,Q));
    }
    for(const box of [['-9223372036854775809','0','1','1'],['1','0','1','1'],['2','0','1','1'],['0','0','9223372036854775808','1']]) {
        const op=ready();op.artifact.bbox_dbu=box;assert.throws(()=>C.operation(op,P,Q));
    }
    const edge=ready();edge.artifact.bbox_dbu=['-9223372036854775808','-1','9223372036854775807','1'];C.operation(edge,P,Q);
    {
        const h=harness();await h.c.init(false);assert(h.el('clip-panel').hidden);assert.equal(h.requests.length,0);
        await h.init();assert(!h.el('clip-open').disabled,'summary display must support exact export');
        const t=h.prepare();assert.equal(t.body.bounds.kind,'viewport');assert.equal(posts(h).length,0);h.reply(t);
        assert(!h.el('clip-approve').disabled);assert.match(h.el('clip-bounds').textContent,/-9007199254740993/);
        assert.match(h.el('clip-selection').textContent,/2 visible/);
        const approving=h.approve();await h.approve();await approving;
        assert.equal(posts(h).length,1);assert.deepEqual(posts(h)[0].body,{seq:'1',view_id:h.state.view_id,token:'d'.repeat(64),approve:true});
        assert(h.el('clip-form').hidden);h.context.connected=false;h.c.changed();assert(!h.el('clip-cancel').hidden);
        await h.el('clip-cancel').onclick();assert.match(h.el('clip-status').textContent,/cancelled/);h.c.stop();assert.equal(h.timers.size,0);
    }
    for(const mutate of [h=>h.context.pending=true,h=>h.context.acked=false,h=>h.context.hidden=true,
        h=>h.context.size.dpr=1,h=>h.state.connection_epoch='c'.repeat(64),h=>h.state.render_key='2',h=>h.state.capabilities.clip=false]) {
        const h=harness();await h.init();const t=h.prepare();mutate(h);h.c.changed();h.reply(t);await h.approve();
        assert.equal(posts(h).length,0);assert(h.el('clip-form').hidden);h.c.stop();
    }
    {
        const h=harness();await h.init();h.prepare();h.reply();h.el('clip-cell-name').value='changed';h.el('clip-cell-name').oninput();
        await h.approve();assert.equal(posts(h).length,0);h.el('clip-form').onsubmit({preventDefault(){}});h.reply();
        assert.match(h.el('clip-options').textContent,/changed/);h.tick(30001);await flush();await h.approve();assert.equal(posts(h).length,0);
        h.prepare();h.tick(5001);await flush();assert.match(h.el('clip-note').textContent,/timed out/);h.c.stop();
    }
    for(const layers of ['visible','all','none']) {
        const h=harness();await h.init();h.prepare();h.el('clip-layers').value=layers;h.el('clip-layers').onchange();
        h.el('clip-jobs').value='16';h.el('clip-cell-name').value='<script>한글 & test</script>';h.el('clip-form').onsubmit({preventDefault(){}});h.reply();
        assert.match(h.el('clip-options').textContent,/<script>한글/);assert(!h.el('clip-approve').disabled);
        await h.approve();assert.equal(posts(h).length,1);h.c.stop();
    }
    for(const [id,value]of [['clip-jobs','0'],['clip-jobs','17'],['clip-jobs','1.5'],['clip-cell-name',''],['clip-cell-name','x\n'],['clip-cell-name','한'.repeat(1366)]]) {
        const h=harness();await h.init();h.prepare();const n=h.sent.length;h.el(id).value=value;h.el(id).oninput();
        h.el('clip-form').onsubmit({preventDefault(){}});assert.equal(h.sent.length,n);assert.match(h.el('clip-note').textContent,/Choose/);h.c.stop();
    }
    {
        const h=harness();await h.init();h.prepare();h.reply(undefined,{draft:{}});assert(h.el('clip-approve').disabled);assert.match(h.el('clip-note').textContent,/Invalid/);
        h.prepare();const t=h.sent.at(-1);h.c.receive({type:'error',seq:t.seq,code:'stale_frame'});assert.match(h.el('clip-note').textContent,/view changed/);h.c.stop();
    }
    {
        // A lost receipt is not permission to invent a fresh operation. Even
        // GET showing the sequence cannot establish another tab's signature.
        const h=harness();h.model=status([entry(id,'failed')]);await h.init();h.prepare();h.reply();
        h.override=r=>{if(r.method==='POST'&&r.path==='/api/v1/exports'){
            h.override=null;h.model=status([entry(r.body.seq)]);return Promise.reject(new Error('lost receipt'));}};
        await h.approve();assert(!h.el('clip-resolve').hidden);const request=posts(h)[0].body;
        assert.equal(request.seq,'9007199254740994');h.c.stop();await h.c.resume();assert.equal(posts(h).length,1);
        h.context.connected=false;await h.el('clip-resolve').onclick();assert.equal(posts(h).length,2);assert.equal(posts(h)[1].body,request);
        assert(h.el('clip-resolve').hidden);h.c.stop();
    }
    {
        const h=harness();await h.init();h.prepare();h.reply();let reject;
        h.override=r=>r.method==='POST'?new Promise((_,no)=>{reject=no;r.token.abort=()=>no(new Error('aborted'));}):undefined;
        const approving=h.approve();await flush();assert(reject);h.c.stop();await approving;
        h.override=null;await h.c.resume();assert.equal(posts(h).length,1);assert(!h.el('clip-resolve').hidden);h.c.stop();
    }
    {
        // A pre-approval read that finishes after pagehide must not clear a
        // newer approval's busy flag or submit against its replaced draft.
        const h=harness();await h.init();h.prepare();h.reply();let oldRead,newRead;
        h.override=r=>r.method==='GET'?new Promise(resolve=>{oldRead=resolve;}):undefined;
        const old=h.approve();await flush();h.c.stop();h.override=null;await h.c.resume();h.prepare();h.reply();
        h.override=r=>r.method==='GET'?new Promise(resolve=>{newRead=resolve;}):undefined;
        const next=h.approve();await flush();oldRead(status());await old;
        assert(h.el('clip-approve').disabled);assert.equal(posts(h).length,0);
        h.override=null;newRead(status());await next;assert.equal(posts(h).length,1);h.c.stop();
    }
    {
        const h=harness();await h.init();h.prepare();h.reply();h.override=r=>r.method==='POST'?Promise.reject(Object.assign(new Error('expired'),{status:410,code:'export_draft_expired'})):undefined;
        await h.approve();assert(h.el('clip-resolve').hidden);assert.match(h.el('clip-note').textContent,/expired/);assert(!h.el('clip-open').disabled);h.c.stop();
    }
    {
        const h=harness();h.model=status([entry('1')]);await h.init();h.override=r=>{if(r.path.endsWith('/cancel')){const op=ready();h.model=status([op],[artifact()]);return Promise.resolve(op);}};
        await h.el('clip-cancel').onclick();assert.match(h.el('clip-status').textContent,/ready/);assert.equal(h.el('clip-files').children.length,1);h.c.stop();
    }
    {
        const h=harness();h.model=s;await h.init();h.context.connected=false;h.c.changed();
        const row=h.el('clip-files').children[0],save=row.children[1].children[0],release=row.children[1].children[1];
        save.focus();h.model.artifacts[0].expires_in_ms='599500';await h.c.refresh();
        assert.equal(h.el('clip-files').children[0],row,'TTL polling replaced the focused download button');assert.equal(h.document.activeElement,save);
        save.onclick();assert.deepEqual(h.downloads,['1']);await release.onclick();assert.match(h.el('clip-files').textContent,/No ready files/);
        assert.equal(h.document.activeElement,h.el('clip-refresh'));assert.match(h.el('clip-note').textContent,/released/);h.c.stop();
    }
    {
        const h=harness();h.model=status([entry('1')]);await h.init();h.override=r=>r.method==='GET'?Promise.reject(new Error('offline')):undefined;
        await h.c.refresh();assert(h.el('clip-cancel').disabled);assert(h.el('clip-open').disabled);h.override=null;await h.c.refresh();assert(!h.el('clip-cancel').disabled);h.c.stop();
    }
    {
        const h=harness(),csrf='c'.repeat(64);C.download(h.document,csrf,id,P);
        assert.deepEqual(h.submissions,[{method:'POST',action:'/api/v1/artifacts/'+id+'/download',target:'_blank',rel:'noopener noreferrer',
            enctype:'application/x-www-form-urlencoded',inputs:[{type:'hidden',name:'csrf',value:csrf}]}]);
        assert.equal(h.document.body.children.length,0);assert(!h.submissions[0].action.includes(csrf));
        for(const bad of ['../file','01','18446744073709551616'])assert.throws(()=>C.download(h.document,csrf,bad,P));
        assert.throws(()=>C.download(h.document,'bad',id,P));assert.equal(h.submissions.length,1);h.c.stop();
    }
    console.log('WEB CLIP UI: ALL OK (display-bound preparation, explicit approval, u64/i64, replay-only uncertainty, lifecycle, cancel/ready, file inventory/focus, native CSRF download)');
})().catch(e=>{console.error(e);process.exitCode=1;});
