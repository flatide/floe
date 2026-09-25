'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {createGap,markerName}=require('./electron-review-gap.cjs');
function fixture(t,kind='notes') {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'floe-electron-review-gap-'));fs.chmodSync(root,0o700);
  t.after(()=>fs.rmSync(root,{recursive:true}));
  const leaf=kind==='notes'?'.synthetic.db.notes.native-recovery-test.fe':'.synthetic.db.waive.native-recovery-test';
  const target=path.join(root,leaf);fs.writeFileSync(target,'synthetic payload',{mode:0o600});fs.writeFileSync(target+'.lock','',{mode:0o600});
  const identity=p=>{const s=fs.statSync(p);return [s.dev,s.ino];};
  const mark={v:1,target:[...Buffer.from(leaf)],stage:'.floe-review-123-456.tmp',directory:identity(root),file:identity(target),lock:identity(target+'.lock')};
  const read=()=>({[markerName]:Buffer.from(JSON.stringify(mark)).toString('hex'),'test.binding':'1234'});
  return {root,target,mark,read,kind};
}
test('model only the exact marked gap, preserve payload and verify actual unlink',t=>{
  for(const kind of ['notes','waives']) {
    const f=fixture(t,kind),g=createGap(f.root,kind,f.read);g.verify(2);
    fs.unlinkSync(g.stage);g.verify(1);assert(fs.existsSync(g.target));
    fs.appendFileSync(g.target,'changed');assert.throws(()=>g.verify(1));
  }
});
test('reject malformed or foreign markers before creating any link',t=>{
  for(const change of [m=>m.stage='../other',m=>m.stage='.floe-review-1-2.tmp/child',m=>m.file[1]++,m=>m.lock[1]++,
    m=>m.directory[1]++,m=>m.target[0]=0,m=>m.v=2,m=>m.extra=true,m=>m.file[1]=Number.MAX_SAFE_INTEGER+1]) {
    const f=fixture(t);change(f.mark);assert.throws(()=>createGap(f.root,f.kind,f.read));assert.equal(fs.statSync(f.target).nlink,1);
  }
});
test('no overwrite or symlink following and no silent cleanup',t=>{
  for(const link of [false,true]) {
    const f=fixture(t),stage=path.join(f.root,f.mark.stage);
    if(link)fs.symlinkSync('missing',stage);else fs.writeFileSync(stage,'decoy');
    assert.throws(()=>createGap(f.root,f.kind,f.read));assert.equal(fs.statSync(f.target).nlink,1);
    if(link)assert.equal(fs.readlinkSync(stage),'missing');else assert.equal(fs.readFileSync(stage,'utf8'),'decoy');
  }
});
test('security attributes and permissions remain part of the preservation check',t=>{
  const f=fixture(t),g=createGap(f.root,f.kind,f.read);f.mark.v=2;assert.throws(()=>g.verify(2));
  f.mark.v=1;fs.chmodSync(f.target,0o640);assert.throws(()=>g.verify(2));
});
