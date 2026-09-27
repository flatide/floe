'use strict';
const assert=require('node:assert/strict'),R=require('./index-revisions.js');
const all={mode:'all'},one={mode:'only',ids:['1']},two={mode:'only',ids:['1','2']};
const source='a'.repeat(64),rev='b'.repeat(32),receipt={kind:'check_revision',phase:'succeeded',source_id:source,levels:all,index_revision:rev};
assert.equal(R.candidate([]),null);
assert.equal(R.candidate([{...receipt,kind:'index_revision'}]),null,'building never proposes an automatic switch');
assert.equal(R.candidate([receipt]),receipt,'read-only reconciliation can restore a checked revision');
assert.equal(R.candidate([receipt,{kind:'check_revision',phase:'failed'}]),null);
assert.equal(R.candidate([{...receipt,index_revision:'../revision'}]),null);
assert.equal(R.matches(receipt,source,all),true);
assert.equal(R.matches(receipt,'c'.repeat(64),all),false);
assert.equal(R.matches(receipt,source,one),false);
assert.equal(R.selection(two),R.selection({mode:'only',ids:['2','1']}));
assert.notEqual(R.selection(all),R.selection(two));
assert.throws(()=>R.selection({mode:'only',ids:[]}));
assert.throws(()=>R.selection({mode:'only',ids:['1','1']}));
const empty=R.useRequest(receipt,source,all,'level',[137,103],null);
assert.equal(empty.approved,true);assert.deepEqual(empty.target,{kind:'empty'});
assert.equal(empty.revision,rev);assert.equal(empty.kind,'use_revision');assert(!empty.options&&!empty.force);
const current={source_id:source,mode:'chip',levels:all,view_id:'d'.repeat(64),state_rev:'27'};
const use=R.useRequest(receipt,source,all,'chip',[137,103],current);
assert.deepEqual(use.target,{kind:'replace',view_id:current.view_id,state_rev:'27'});
assert.throws(()=>R.useRequest(receipt,source,all,'layer',[137,103],current));
assert.throws(()=>R.useRequest(receipt,source,one,'chip',[137,103],current));
const usage={kind:'revision_usage',phase:'succeeded',source_id:source,inventory:{logical_bytes:'9007199254740993',stores_scanned:2,unknown_entries:0,unavailable_entries:0,partial:false,rows:[
    {kind:'source',source_number:1,revision:rev,logical_bytes:'9007199254740000',format:2,current:false,seal:'valid',readers:'in_use',owner:'this_dataset',set_revision:'c'.repeat(32)}]}};
assert.equal(R.usage([]),null);
assert.equal(R.usage([{...usage,phase:'failed'}]),null);
assert.match(R.usage([usage]).text,/9007199254740993 logical file bytes/,'bytes must not be rounded by Number');
assert.match(R.usage([usage]).text,/in_use/);
assert.match(R.usage([usage]).text,/No deletion performed/);
assert.match(R.usage([{...usage,inventory:{...usage.inventory,partial:true}}]).text,/PARTIAL/);
assert.equal(R.usage([usage,{...usage,phase:'failed'}]),null,'failed refresh must not look successful');
assert.equal(R.usage([{...usage,inventory:{...usage.inventory,logical_bytes:900}}]),null);
assert.equal(R.candidate([usage]),null,'storage observation is not a checked cutover revision');
const preview={kind:'prepare_reclaim',phase:'succeeded',source_id:source,preview:{token:'c'.repeat(64),revision:rev,
    files:7,logical_bytes:'9007199254740993',sources:1,recovery:false,complete:false,expires_in_s:300}};
assert.equal(R.reclaimCandidate([preview]),preview);
assert.equal(R.reclaimCandidate([preview,usage]),null,'any subsequent accepted operation invalidates approval');
assert.equal(R.reclaimCandidate([{...preview,phase:'cancelled'}]),null);
assert.deepEqual(R.reclaimRequest(preview,source,rev,true),{kind:'reclaim_revision',source_id:source,revision:rev,token:'c'.repeat(64),approved:true});
for(const [s,r,a] of [[source,rev,false],['e'.repeat(64),rev,true],[source,'e'.repeat(32),true]]){
    assert.throws(()=>R.reclaimRequest(preview,s,r,a));
}
for(const [key,value] of [['token','../path'],['revision','a'.repeat(33)],['files',7171],['sources',0],['expires_in_s',0],
    ['logical_bytes','18446744073709551616'],['logical_bytes',10],['complete',true],['path','/outside']]){
    assert.equal(R.reclaimCandidate([{...preview,preview:{...preview.preview,[key]:value}}]),null,key);
}
const complete={...preview,preview:{...preview.preview,files:0,logical_bytes:'0',complete:true,recovery:true}};
assert.equal(R.reclaimCandidate([complete]),complete);
assert.throws(()=>R.reclaimRequest(complete,source,rev,true));
assert.match(R.reclaimText([complete]),/already complete/);
assert.match(R.reclaimText([preview]),/9007199254740993/);
const interrupted={kind:'reclaim_revision',phase:'incomplete',outcome:{revision:rev,status:'outcome_unknown',removed_files:1,removed_logical_bytes:'5',sync_warning:false}};
assert.equal(R.reclaimCandidate([interrupted]),null);
assert.match(R.reclaimText([interrupted]),/No automatic retry/);
assert.match(R.reclaimText([interrupted]),/acknowledged deletion/);
assert.match(R.reclaimText([{...interrupted,phase:'failed'}]),/protected/);
const observed={...usage,inventory:{...usage.inventory,recoveries:[rev]}};
assert.deepEqual(R.usage([observed]).choices,[{revision:rev,label:rev+' — recovery evidence, unverified',current:false}]);
assert.equal(R.reclaimCandidate([observed]),null,'journal names are observations, not approval');
assert.equal(R.usage([{...observed,inventory:{...observed.inventory,recoveries:['../journal']}}]),null);
console.log('WEB INDEX REVISIONS: ALL OK (build/check/use, bounded observation, narrow reclamation, invalidation, explicit recovery)');
