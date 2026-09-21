'use strict';
// Explicit macOS QA: generate one valmini and feed that same immutable cache to
// WKWebView and Electron. No caller source, clipboard or review write authority.
const fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {spawnSync}=require('node:child_process'),{createHash}=require('node:crypto');
if(process.platform!=='darwin'||process.argv.length!==2)throw Error('macOS only; no arguments; NEW synthetic fixture');
const repo=path.resolve(__dirname,'..');
const root=fs.mkdtempSync(path.join(os.tmpdir(),'floe-native-parity-'));fs.chmodSync(root,0o700);
console.log('NATIVE PARITY: synthetic artifacts '+root);
function selected(key,fallback) {return Object.hasOwn(process.env,key)?process.env[key]:fallback;}
function run(binary,args,label,env=process.env) {
    const r=spawnSync(binary,args,{cwd:repo,env,encoding:'utf8',timeout:120000,maxBuffer:8*1024*1024});
    const lines=((r.stdout||'')+'\n'+(r.stderr||'')).split('\n').filter(s=>
        /^(?:DESKTOP LAYOUT:|ELECTRON (?:LAYOUT|SMOKE):)/.test(s));
    for(const line of lines)console.log(line);
    if(r.error||r.status!==0)throw Error(label+' failed; raw session diagnostics suppressed');
    return lines;
}
function snapshot(directory,prefix='') {
    const out={};
    for(const e of fs.readdirSync(directory,{withFileTypes:true}).sort((a,b)=>a.name.localeCompare(b.name))) {
        const name=path.join(prefix,e.name),full=path.join(directory,e.name);
        if(e.isDirectory())Object.assign(out,snapshot(full,name));
        else if(e.isFile())out[name]=createHash('sha256').update(fs.readFileSync(full)).digest('hex');
        else throw Error('Unexpected synthetic input type');
    }
    return JSON.stringify(out);
}
const source=path.join(root,'valmini.oas');
run(selected('FLOE_QA_PYTHON_BIN',path.join(repo,'.venv/bin/python')),
    ['-B',path.join(repo,'tools/gen_valmini.py'),source],'Synthetic generator');
const index=selected('FLOE_INDEX_BIN',path.join(repo,'rust/target/release/floe-index'));
const renderd=selected('FLOE_RENDERD_BIN',path.join(repo,'rust/target/release/floe-renderd'));
run(index,['vfs',source,path.join(root,'.valmini.oas.ice'),'--jobs','4'],'Synthetic index');
const before=snapshot(root);
try {
    for(const reuse of ['on','off']) {
        console.log('NATIVE PARITY: pan reuse '+reuse);
        const env={...process.env,FLOE_INDEX_BIN:index,FLOE_RENDERD_BIN:renderd,FLOE_RUST_PAN_REUSE:reuse};
        const wk=run(selected('FLOE_QA_DESKTOP_BIN',path.join(repo,'desktop/target/debug/floe2-desktop')),
            ['--smoke-frame-parity-test',source],'Actual WKWebView parity',env);
        if(!wk.some(s=>s.startsWith('DESKTOP LAYOUT: OK ('))||wk.filter(s=>s.startsWith('DESKTOP LAYOUT: phase=')).length!==3) {
            throw Error('Missing native geometry verdict');
        }
        const electron=run('sh',[path.join(repo,'tools/run_electron_dev.sh'),'--smoke-frame-parity-test',source],
            'Actual Electron parity',env);
        if(!electron.some(s=>s.startsWith('ELECTRON LAYOUT: FRAME PARITY OK (')))throw Error('Missing Electron geometry verdict');
    }
} finally {
    if(before!==snapshot(root))throw Error('Source/cache changed during native parity QA');
}
console.log('NATIVE PARITY: OK (same source/cache/workers, per-host Canvas parity; NOT cross-host pixel or speed equivalence)');
