'use strict';
// Development-only Node editor + actual Rust HTTP, NOT browser acceptance.
const assert=require('node:assert/strict'),fs=require('node:fs'),http=require('node:http');
const N=require('./drc-notes.js'),W=require('./drc-waives.js'),P=require('./protocol.js');
const config=JSON.parse(fs.readFileSync(0,'utf8'));assert(/^http:\/\/127\.0\.0\.1:\d+$/.test(config.origin));
const ids=new Set([...fs.readFileSync(__dirname+'/index.html','utf8').matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
const nodes=new Map(),timers=new Map(),writes={notes:0,waives:0};let serial=0,context=config.context,reader,notes,waives;
function el(id){assert(ids.has(id));if(!nodes.has(id))nodes.set(id,{value:'',textContent:'',checked:false,disabled:false,hidden:false,focus(){}});return nodes.get(id);}
function call(method,path,body){
    assert(path.startsWith('/api/v1/drc'));const data=body===undefined?null:JSON.stringify(body);
    if(method==='POST'&&path==='/api/v1/drc/review/notes')++writes.notes;
    if(method==='POST'&&path==='/api/v1/drc/review/waives')++writes.waives;
    return new Promise((resolve,reject)=>{
        const req=http.request(new URL(path,config.origin),{method,headers:{Origin:config.origin,Cookie:config.cookie,
            'X-Floe-CSRF':config.csrf,'Content-Type':'application/json',...(data?{'Content-Length':Buffer.byteLength(data)}:{})}},res=>{
            let text='';res.setEncoding('utf8');res.on('data',part=>{text+=part;if(text.length>1048576)req.destroy(new Error('Oversized response'));});
            res.on('end',()=>{try{const value=text?JSON.parse(text):null;
                if(res.statusCode>=400)reject(Object.assign(new Error('Native HTTP rejected request'),{status:res.statusCode,code:value&&value.error}));else resolve(value);
            }catch(e){reject(e);}});
        });req.on('error',reject);req.setTimeout(8000,()=>req.destroy(new Error('Native HTTP timeout')));req.end(data);
    });
}
const common={el,protocol:P,http:call,session:()=>'f'.repeat(64),now:()=>Date.now(),loadPending:()=>null,savePending(){},
    connection:()=>'9'.repeat(64), // Explicit headless transport model, not a browser socket.
    setTimeout(fn,ms){timers.set(++serial,{fn,ms});return serial;},clearTimeout:id=>timers.delete(id),
    selection:()=>({context:{...context},epoch:'9'.repeat(64),key:'native-two',caption:'Two synthetic errors',count:2,references:()=>config.references})};
async function refreshReview(){const v=await call('GET','/api/v1/drc');reader=v.drc;context={...context,revision:reader.revision};
    notes.attach(v.notes);notes.changed();waives.attach(v.waives,reader);}
notes=N.bind(common);
waives=W.bind({...common,selection:()=>waives.suspended()?null:common.selection(),refreshReview});
function mode(kind,on){el(kind+'-autosave').checked=on;el(kind+'-autosave').onchange();}
async function until(check){const end=Date.now()+15000;while(!check()){if(Date.now()>end)throw Error('Native editor outcome timed out');await new Promise(r=>setTimeout(r,20));}}
async function settle(kind,expected){await until(()=>writes[kind]===expected);const panel=kind==='notes'?notes:waives;
    const end=Date.now()+15000;for(;;){await panel.refresh();if(kind==='waives')await refreshReview();
        const state=await call('GET','/api/v1/drc/review/'+kind),last=state.operations.history.at(-1);
        if(last&&['failed','cancelled'].includes(last.phase))throw Error('Native save '+last.phase+': '+(last.error||'no code'));
        if(last&&last.phase==='succeeded'&&!state.operations.active){assert.equal(last.published,true);if(kind==='waives')assert.equal(last.reader_applied,true);return;}
        if(Date.now()>end)throw Error('Native save did not finish');await new Promise(r=>setTimeout(r,20));}
}
async function main(){
    await refreshReview();assert(!el('notes-autosave').checked);assert(!el('waives-autosave').checked);
    mode('notes',true);mode('waives',true);assert.deepEqual(writes,{notes:0,waives:0});
    await el('notes-read').onclick();el('notes-text').value='자동 저장 confirmed\nsecond line';el('notes-text').oninput();assert.equal(writes.notes,0);
    await el('notes-prepare').onclick();await settle('notes',1);assert.equal(el('notes-text').value,'');
    const readback=await call('POST','/api/v1/drc/review/notes/display',{context,errors:config.references,focus:config.references[0]});
    assert.equal(readback.focus.text,'자동 저장 confirmed\nsecond line');
    await el('waives-read').onclick();el('waives-action').value='waive';await el('waives-action').onchange();await settle('waives',1);
    assert(!waives.suspended());assert(el('waives-autosave').checked,'reader revision reset the opt-in');
    waives.open();await settle('waives',2);assert(!waives.suspended());
    const cleared=await call('POST','/api/v1/drc/review/waives/read',{context,errors:config.references});assert.equal(cleared.waived_count,'0');
    await call('POST','/api/v1/drc/review/waives/revoke',{token:cleared.token});
    mode('notes',false);await el('notes-read').onclick();el('notes-text').value='not approved';el('notes-text').oninput();await el('notes-prepare').onclick();
    assert.equal(writes.notes,1);assert(!el('notes-review').hidden);assert(el('notes-approve').disabled);
    notes.stop();waives.stop();assert.equal(timers.size,0);assert(!el('waives-autosave').checked);
    console.log('NATIVE REVIEW AUTOSAVE: ALL OK (real note sidecar/readback, waive/clear+reader revisions, explicit opt-in, opt-out preview, fixed targets, lifecycle)');
}
main().catch(e=>{notes.stop();waives.stop();console.error(e.message);process.exitCode=1;});
