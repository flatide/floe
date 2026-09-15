'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs');
const T=require('./drc-transfer.js'),P=require('./protocol.js'),clone=v=>JSON.parse(JSON.stringify(v));
const C={drc_id:'a'.repeat(64),revision:'b'.repeat(64),view_id:'c'.repeat(64)},epoch='d'.repeat(64),token='e'.repeat(64),CHUNK=1048576;
const report={skipped_lines:0,invalid_members:0,reassigned_members:0};
const flush=async()=>{for(let i=0;i<30;i++)await Promise.resolve();};
function preview(k='notes'){return {kind:k==='notes'?'drc_note':'drc_waive',phase:'prepared',action:'replace_all',name:'synthetic-'+k,
    replaces_existing:true,legacy_unverified:true,scope:'registered_reviewer_entire_review',bytes:'100',token,context:clone(C),review_rev:'0',reviewer:'fixed',expires_in_ms:'30000',
    ...(k==='notes'?{groups:'2',members:'3',clears:false,import_report:clone(report)}:{waived_count:'1'})};}
function catalog(){return {kind:'drc_review_transfer',available:true,operations:{last_seq:'0',active:null,history:[]},upload:null,artifacts:[],
    limits:{chunk_bytes:CHUNK,file_bytes:'536870912',entries:2,readers:1,ttl_seconds:600},usage:{entries:0,bytes:'0',pending:0,readers:0}};}
function harness(){
    const ids=[...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]);
    const nodes=new Map(),timers=new Map(),models={notes:catalog(),waives:catalog()},records=new Map(),calls=[],slices=[],locks={},publications=[],downloads=[];
    let now=0,serial=0,override=null,scope={context:clone(C),epoch},canImport=true,canExport=true;
    function node(){return {disabled:false,hidden:false,checked:false,value:'',files:[],children:[],_text:'',focus(){this.focused=true;},
        appendChild(v){this.children.push(v);},removeChild(v){this.children.splice(this.children.indexOf(v),1);},
        set textContent(v){this._text=v;this.children=[];},get textContent(){return this._text;},set innerHTML(v){throw Error('HTML injection');}};}
    function el(id){assert(ids.includes(id),id);if(!nodes.has(id)){nodes.set(id,node());}return nodes.get(id);}
    const editors=Object.fromEntries(['notes','waives'].map(k=>[k,{transferReady:imp=>imp?canImport:canExport,transferLock:v=>locks[k]=v,refresh:async()=>{},
        async publishTransfer(v,valid){assert(valid());publications.push({kind:k,preview:clone(v)});return true;}}]));
    function execute(r){const k=r.kind,m=models[k];
        if(r.method==='GET'){if(r.path.endsWith('/transfer'))return clone(m);return clone(records.get(k+':'+r.path.split('/').at(-1)).result);}
        if(r.path.endsWith('/revoke')){m.upload=null;return null;}
        if(r.path.endsWith('/cancel'))return {kind:'drc_review_transfer',seq:r.path.split('/').at(-2),phase:'queued'};
        if(r.method==='DELETE'){m.artifacts=m.artifacts.filter(a=>a.id!==r.path.split('/').at(-1));return null;}
        const req=r.body,prev=records.get(k+':'+req.seq);
        if(prev){assert.deepEqual(req,prev.request);assert.equal(r.blob,prev.blob,'retry reread or replaced its pending slice');return clone(prev.result);}
        assert.equal(req.seq,P.next(m.operations.last_seq));
        const result={kind:'drc_review_transfer',seq:req.seq,phase:'succeeded',action:req.action,context:clone(req.context)};
        if(req.action==='import'){m.upload={token,context:clone(req.context),bytes:req.bytes,received:'0'};result.upload={token,bytes:req.bytes,received:'0',expires_in_ms:'600000'};}
        else if(req.action==='chunk'){assert.equal(r.at,Number(m.upload.received));m.upload.received=String(r.at+r.blob.size);result.upload={token,bytes:m.upload.bytes,received:m.upload.received,expires_in_ms:'600000'};}
        else if(req.action==='prepare'){result.preview=preview(k);result.preview.context=clone(req.context);m.upload=null;}
        else{const a={id:req.seq,name:'floe-'+k+'-'+req.seq+(k==='notes'?'.fe':'.waive'),bytes:'100',review_rev:'0'};m.artifacts.push({...a,context:clone(req.context),expires_in_ms:'600000'});
            result.artifact={...a,contents:k==='notes'?{groups:'2',members:'3'}:{waived_count:'1'},legacy_unverified:false,import_report:clone(report)};}
        m.operations={last_seq:req.seq,active:null,history:m.operations.history.concat([clone(result)]).slice(-32)};records.set(k+':'+req.seq,{request:clone(req),result:clone(result),blob:r.blob});return clone(result);
    }
    async function invoke(r){calls.push(r);if(override){const result=override(r);if(result!==undefined)return await result;}return execute(r);}
    const panel=T.bind({el,document:{createElement:node},protocol:P,editors,context:()=>scope,now:()=>now,
        setTimeout(fn,ms){timers.set(++serial,{fn,at:now+ms});return serial;},clearTimeout:id=>timers.delete(id),
        http(method,path,body,missing,t){return invoke({kind:path.split('/')[5],method,path,body,t});},
        chunk(k,request,at,blob,t){return invoke({kind:k,method:'POST',path:'/api/v1/drc/review/'+k+'/transfer/chunk',body:request,at,blob,t});},
        download:(k,id)=>downloads.push({kind:k,id})});
    return {panel,el,calls,slices,models,records,publications,downloads,locks,timers,editors,execute,
        init:()=>panel.attach({notes:{editable:true},waives:{}}),
        file(size=CHUNK+10){el('transfer-file').files=[{size,slice(at,end){assert(end-at<=CHUNK);const blob={size:end-at,at};slices.push(blob);return blob;},text(){throw Error('whole-file read');},arrayBuffer(){throw Error('whole-file read');}}];el('transfer-file').onchange();},
        upload:()=>el('transfer-import').onclick(),export:()=>el('transfer-export').onclick(),resolve:()=>el('transfer-resolve').onclick(),discard:()=>el('transfer-discard').onclick(),
        consent(){el('transfer-run').checked=el('transfer-consent').checked=true;el('transfer-consent').onchange();},approve:()=>el('transfer-approve').onclick(),
        tick(ms){now+=ms;for(const [id,t]of[...timers])if(t.at<=now){timers.delete(id);t.fn();}},
        set override(v){override=v;},set scope(v){scope=v;},set canImport(v){canImport=v;},set canExport(v){canExport=v;}};
}
async function tests(){
    const reader=harness();await reader.panel.attach({notes:{editable:false},waives:null});
    assert(reader.el('transfer-panel').hidden);reader.file();await reader.upload();await reader.export();
    assert.equal(reader.calls.length,0,'read-only reviewer opened transfer APIs');reader.panel.stop(true);
    const h=harness();assert(h.el('transfer-panel').hidden);await h.init();h.file();await h.upload();
    assert.equal(h.slices.length,2);assert.deepEqual(h.slices.map(b=>b.size),[CHUNK,10]);assert.equal(h.publications.length,0);assert(!h.el('transfer-review').hidden);
    assert(h.el('transfer-approve').disabled);await h.approve();assert.equal(h.publications.length,0);h.el('transfer-consent').checked=true;h.el('transfer-consent').onchange();assert(h.el('transfer-approve').disabled);
    h.consent();assert(!h.el('transfer-approve').disabled);await h.approve();assert.equal(h.publications.length,1);assert.equal(h.publications[0].kind,'notes');assert(h.el('transfer-review').hidden);assert(!h.locks.notes);
    assert(h.calls.every(r=>r.path.includes('/transfer')||r.path.endsWith('/revoke')),'transfer bypassed the existing save controller');
    h.canImport=false;h.file(40);assert(h.el('transfer-import').disabled);await h.export();assert.equal(h.models.notes.artifacts.length,1,'export must preserve a local selection editor');
    const row=h.el('transfer-files').children[0];row.children[1].focus();await h.panel.refresh();assert.equal(h.el('transfer-files').children[0],row,'poll replaced the focused download row');assert(row.children[1].focused);
    row.children[1].onclick();assert.equal(h.downloads.length,1);await row.children[2].onclick();assert.equal(h.models.notes.artifacts.length,0);h.panel.stop(true);assert.equal(h.timers.size,0);

    const queued=harness();await queued.init();queued.override=r=>{if(r.method==='POST'&&r.body.action==='export'){queued.execute(r);return {kind:'drc_review_transfer',seq:r.body.seq,phase:'queued'};}};
    const exporting=queued.export();await flush();assert(!queued.el('transfer-cancel').disabled);assert(queued.el('transfer-kind').disabled);queued.tick(500);await exporting;
    assert(queued.calls.some(r=>r.method==='GET'&&r.path.endsWith('/transfer/1')));assert.equal(queued.models.notes.artifacts.length,1);queued.panel.stop(true);assert.equal(queued.timers.size,0);
    assert.equal(queued.el('transfer-files').children.length,0);assert.equal(queued.el('transfer-usage').textContent,'');

    const cancelled=harness();await cancelled.init();cancelled.override=r=>{if(r.method==='POST'&&r.body.action==='export'){cancelled.execute(r);return {kind:'drc_review_transfer',seq:r.body.seq,phase:'queued'};}};
    const cancelling=cancelled.export();await flush();await cancelled.discard();await cancelling;
    assert(cancelled.calls.some(r=>r.path.endsWith('/transfer/1/cancel')));assert.equal(cancelled.publications.length,0);assert(!cancelled.locks.notes);cancelled.panel.stop(true);assert.equal(cancelled.timers.size,0);

    const ended=harness();await ended.init();ended.file();let late;
    ended.override=r=>r.body&&r.body.action==='import'?new Promise(resolve=>late=()=>resolve(ended.execute(r))):undefined;
    const ending=ended.upload();ended.panel.stop(true);const callsAtEnd=ended.calls.length;late();await ending;
    assert.equal(ended.calls.length,callsAtEnd,'late reply polled after session end');assert.equal(ended.slices.length,0);assert.equal(ended.publications.length,0);assert.equal(ended.timers.size,0);assert.match(ended.el('transfer-message').textContent,/^Session ended/);

    const lost=harness();await lost.init();lost.file();let once=true;lost.override=r=>{if(r.blob&&once){once=false;lost.execute(r);return Promise.reject(new Error('ACK lost'));}};
    await lost.upload();assert(!lost.el('transfer-resolve').hidden);assert.equal(lost.publications.length,0);assert.equal(lost.slices.length,1);await lost.resolve();
    assert.equal(lost.slices.length,2);assert(!lost.el('transfer-review').hidden);assert.equal(lost.publications.length,0);await lost.discard();assert(lost.el('transfer-review').hidden);lost.panel.stop(true);

    const expiry=harness();await expiry.init();expiry.file(20);await expiry.upload();expiry.consent();expiry.tick(30001);await Promise.resolve();assert(expiry.el('transfer-review').hidden);assert.equal(expiry.publications.length,0);expiry.panel.stop(true);
    const stale=harness();await stale.init();stale.file();let reply;stale.override=r=>r.body&&r.body.action==='import'?new Promise(resolve=>reply=()=>resolve(stale.execute(r))):undefined;
    const waiting=stale.upload();stale.scope={context:clone(C),epoch:'1'.repeat(64)};stale.panel.changed();reply();await waiting;assert.equal(stale.slices.length,0);assert.equal(stale.publications.length,0);await stale.discard();assert.equal(stale.models.notes.upload,null);stale.panel.stop(true);

    const limits=harness();await limits.init();limits.file(16*CHUNK+1);const n=limits.calls.length;await limits.upload();assert.equal(limits.calls.length,n);assert.match(limits.el('transfer-message').textContent,/16/);limits.panel.stop(true);
    const waive=harness();await waive.init();waive.el('transfer-kind').value='waives';await waive.el('transfer-kind').onchange();waive.file(6007);await waive.upload();waive.consent();await waive.approve();assert.equal(waive.publications[0].kind,'waives');waive.panel.stop(true);

    for(const k of ['notes','waives']){T.catalog(catalog(),k,P);T.preview(preview(k),k,P);for(const change of [v=>v.extra=true,v=>v.context.view_id='wrong',v=>v.action='merge',v=>v.legacy_unverified=false,v=>v.expires_in_ms='30001']){const v=preview(k);change(v);assert.throws(()=>T.preview(v,k,P));}}
    for(const change of [v=>v.limits.file_bytes='1',v=>v.usage.pending=3,v=>v.operations.last_seq='01',v=>v.artifacts=[{path:'/etc/passwd'}]]){const v=catalog();change(v);assert.throws(()=>T.catalog(v,'notes',P));}
    let submitted;const body={children:[],appendChild(v){this.children.push(v);},removeChild(v){this.children.splice(this.children.indexOf(v),1);}};
    const doc={body,createElement(){return {children:[],appendChild(v){this.children.push(v);},submit(){submitted={action:this.action,csrf:this.children[0].value,target:this.target,rel:this.rel};}};}};
    T.download(doc,'f'.repeat(64),'notes','9007199254740993',P);assert.equal(submitted.action,'/api/v1/drc/review/notes/artifacts/9007199254740993/download');assert.equal(submitted.csrf,'f'.repeat(64));assert.equal(submitted.target,'_blank');assert.equal(submitted.rel,'noopener noreferrer');assert.equal(body.children.length,0);
    console.log('WEB DRC TRANSFER UI: ALL OK (chunk/replay, whole replacement consent, editor preservation, limits, stale/expiry, downloads, teardown)');
}
module.exports={catalog,preview};
if(require.main===module){
    let completed=false;
    process.once('beforeExit',()=>{if(!completed){console.error('Transfer test exited with unresolved async work');process.exitCode=1;}});
    tests().then(()=>{completed=true;},e=>{completed=true;console.error(e);process.exitCode=1;});
}
