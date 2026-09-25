'use strict';
// Synthetic QA only. Reconstruct a marked link gap after a real Rust save;
// never scan for orphan names, invent a marker or unlink anything here.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const markerName = process.platform === 'darwin' ? 'com.floe.review-stage-v1' : 'user.floe.review-stage-v1';
function attributes(file) {
  const opts = { encoding:'utf8', timeout:5000, maxBuffer:1024*1024, stdio:['ignore','pipe','pipe'] }, out = {};
  if (process.platform === 'darwin') {
    const names = execFileSync('/usr/bin/xattr', [file], opts).split('\n').filter(Boolean).sort();
    for (const name of names) {
      const hex = execFileSync('/usr/bin/xattr', ['-px', name, file], opts).replace(/\s/g, '');
      assert(/^(?:[0-9a-fA-F]{2})*$/.test(hex));out[name] = hex.toLowerCase();
    }
  } else if (process.platform === 'linux') {
    // Development-only attr utility; never install it or change the product.
    const dump = execFileSync('getfattr', ['--dump','--match=-','--encoding=hex','--absolute-names','--',file], opts);
    for (const row of dump.split('\n').sort()) {
      if (!row || row.startsWith('# file: ')) continue;
      const match = /^([^=]+)=0x((?:[0-9a-fA-F]{2})*)$/.exec(row);assert(match);out[match[1]] = match[2].toLowerCase();
    }
  } else throw Error('Synthetic gap QA requires macOS/Linux');
  return out;
}
function stamp(file) {
  const s = fs.lstatSync(file, {bigint:true});assert(s.isFile());
  return {dev:s.dev,ino:s.ino,mode:s.mode,uid:s.uid,gid:s.gid,size:s.size,mtime:s.mtimeNs};
}
function sameId(pair, s) {
  return Array.isArray(pair) && pair.length === 2 && pair.every(Number.isSafeInteger) &&
    BigInt(pair[0]) === s.dev && BigInt(pair[1]) === s.ino;
}
function createGap(root, kind, readAttributes = attributes) {
  assert(['notes','waives'].includes(kind));assert(/^floe-electron-review-/.test(path.basename(root)));
  const directory = fs.lstatSync(root, {bigint:true});
  assert(directory.isDirectory() && (directory.mode & 0o777n) === 0o700n);
  const leaf = kind === 'notes' ? '.synthetic.db.notes.native-recovery-test.fe' : '.synthetic.db.waive.native-recovery-test';
  const target = path.join(root, leaf), before = stamp(target), bytes = fs.readFileSync(target), attrs = readAttributes(target);
  assert((before.mode & 0o777n) === 0o600n && fs.lstatSync(target).nlink === 1);
  const encoded = attrs[markerName];assert(typeof encoded === 'string' && encoded.length <= 8192);
  const m = JSON.parse(Buffer.from(encoded, 'hex').toString('utf8'));
  assert.deepEqual(Object.keys(m).sort(), ['directory','file','lock','stage','target','v']);
  assert.equal(m.v,1);assert(Array.isArray(m.target) && m.target.every(n=>Number.isInteger(n)&&n>=0&&n<=255));
  assert(Buffer.from(m.target).equals(Buffer.from(leaf)));
  assert(typeof m.stage === 'string' && /^\.floe-review-[0-9]+-[0-9]+\.tmp$/.test(m.stage));
  const lock = stamp(target+'.lock');assert.equal(lock.size,0n);assert.equal(fs.lstatSync(target+'.lock').nlink,1);
  assert(sameId(m.directory,directory) && sameId(m.file,before) && sameId(m.lock,lock));
  const stage = path.join(root,m.stage);assert.equal(path.dirname(stage),root);
  // linkSync is no-replace, including a dangling symlink at the stage name.
  fs.linkSync(target,stage);
  function verify(links) {
    assert([1,2].includes(links));assert.deepEqual(stamp(target),before);
    assert(fs.readFileSync(target).equals(bytes));assert.deepEqual(readAttributes(target),attrs);
    assert.deepEqual(stamp(target+'.lock'),lock);assert.equal(fs.lstatSync(target).nlink,links);
    if (links === 2) assert.deepEqual(stamp(stage),before);
    else assert.throws(()=>fs.lstatSync(stage),{code:'ENOENT'});
  }
  verify(2);return {target,stage,leaf,bytes:bytes.length,verify};
}
module.exports = {createGap,attributes,markerName};
