'use strict';
// Node >=18 is a DEVELOPMENT gate only. The Rust executable embeds plain JS.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const {spawnSync} = require('node:child_process');
const acorn = require('./vendor/acorn-8.15.0/acorn.js');
const root = path.resolve(__dirname, '..');
const ui = path.join(root, 'rust/web/ui');
const options = {ecmaVersion: 2017, sourceType: 'script'};
// Source guard only; actual overlay-scrollbar hit testing is a browser gate.
const layerScroll = fs.readFileSync(path.join(ui, 'app.css'), 'utf8').match(/^\.layers\s*\{([^}]+)\}/m);
assert(layerScroll && /padding-right:\s*16px\s*;/.test(layerScroll[1]), 'layer style buttons need an overlay-scrollbar inset');
// Pointer exit clears this readout before the button click lands. Reserve its
// height even when empty; long DBU values/errors remain keyboard-scrollable.
const probeStyle = fs.readFileSync(path.join(ui, 'app.css'), 'utf8').match(/^#snap-status\s*\{([^}]+)\}/m);
assert(probeStyle && /height:\s*2\.8em\s*;/.test(probeStyle[1]) && /overflow:\s*auto\s*;/.test(probeStyle[1]), 'snap readout must not move following controls');
assert.match(fs.readFileSync(path.join(ui, 'index.html'), 'utf8'), /<p id="snap-status" tabindex="0" role="region" aria-label="Snap probe result"><\/p>/);
acorn.parse(fs.readFileSync(path.join(ui,'drc-geometry.js'),'utf8'),options);
for(const name of ['guest','guest-drc','guest-drc-step','guest-focus','guest-layers','guest-display','guest-query-wire','guest-tools','sharing']){
    acorn.parse(fs.readFileSync(path.join(ui,name+'.js'),'utf8'),options);
    const run=spawnSync(process.execPath,[path.join(ui,name+'.test.cjs')],{stdio:'inherit',timeout:15000});
    assert.equal(run.status,0,name+' UI: '+run.error);
}
const guestCD=spawnSync(process.execPath,[path.join(ui,'guest-drc-cd.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(guestCD.status,0,'guest DRC CD: '+guestCD.error);
const guestFocusUI=spawnSync(process.execPath,[path.join(ui,'guest-focus-ui.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(guestFocusUI.status,0,'guest focus UI: '+guestFocusUI.error);
const guestLifecycle=spawnSync(process.execPath,[path.join(ui,'guest-lifecycle.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(guestLifecycle.status,0,'guest lifecycle: '+guestLifecycle.error);
const frameStatus=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_FRAME_STATUS:'1'}});
assert.equal(frameStatus.status,0,'frame status client: '+frameStatus.error);
for(const file of ['wheel.test.cjs','client.test.cjs']){
    const run=spawnSync(process.execPath,[path.join(ui,file)],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_WHEEL:'1'}});
    assert.equal(run.status,0,'wheel '+file+': '+run.error);
}
for(const name of ['image-decode','display-test','display-input','display-page','display-dump']){
    acorn.parse(fs.readFileSync(path.join(ui,name+'.js'),'utf8'),options);
    const test=spawnSync(process.execPath,[path.join(ui,name+'.test.cjs')],{stdio:'inherit',timeout:15000});
    assert.equal(test.status,0,name+': '+test.error);
}
const displayClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_DISPLAY:'1'}});
assert.equal(displayClient.status,0,'display client: '+displayClient.error);
const dumpClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_DUMP:'1'}});
assert.equal(dumpClient.status,0,'dump client: '+dumpClient.error);
acorn.parse(fs.readFileSync(path.join(ui, 'review-save-mode.js'), 'utf8'), options);
for(const file of ['review-save-mode.test.cjs','review-autosave.test.cjs']){
    const run=spawnSync(process.execPath,[path.join(ui,file)],{stdio:'inherit',timeout:15000});
    assert.equal(run.status,0,file+': '+run.error);
}
acorn.parse(fs.readFileSync(path.join(ui, 'hangul.js'), 'utf8'), options);
const hangul=spawnSync(process.execPath,[path.join(ui,'hangul.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(hangul.status,0,'hangul.test.cjs: '+hangul.error);
acorn.parse(fs.readFileSync(path.join(ui, 'presets.js'), 'utf8'), options);
acorn.parse(fs.readFileSync(path.join(ui, 'fill-editor.js'), 'utf8'), options);
const fillEditor=spawnSync(process.execPath,[path.join(ui,'fill-editor.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(fillEditor.status,0,'fill-editor.test.cjs: '+fillEditor.error);
const presets=spawnSync(process.execPath,[path.join(ui,'presets.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(presets.status,0,'presets.test.cjs: '+presets.error);
acorn.parse(fs.readFileSync(path.join(ui, 'palette.js'), 'utf8'), options);
const palette=spawnSync(process.execPath,[path.join(ui,'palette.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(palette.status,0,'palette.test.cjs: '+palette.error);
const paletteClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_PALETTE:'1'}});
assert.equal(paletteClient.status,0,'palette client: '+paletteClient.error);
const fillClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_FILL_EDITOR:'1'}});
assert.equal(fillClient.status,0,'fill slot client: '+fillClient.error);
const gotoClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_GOTO:'1'}});
assert.equal(gotoClient.status,0,'goto client: '+gotoClient.error);
acorn.parse(fs.readFileSync(path.join(ui, 'index-open.js'), 'utf8'), options);
const indexOpen=spawnSync(process.execPath,[path.join(ui,'index-open.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(indexOpen.status,0,'index-open.test.cjs: '+indexOpen.error);
const indexClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_INDEX_OPEN:'1'}});
assert.equal(indexClient.status,0,'index/open client: '+indexClient.error);
const indexDefaults=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_INDEX_DEFAULTS:'1'}});
assert.equal(indexDefaults.status,0,'index defaults client: '+indexDefaults.error);
acorn.parse(fs.readFileSync(path.join(ui, 'browse.js'), 'utf8'), options);
const browse=spawnSync(process.execPath,[path.join(ui,'browse.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(browse.status,0,'browse.test.cjs: '+browse.error);
acorn.parse(fs.readFileSync(path.join(ui, 'launcher.js'), 'utf8'), options);
const launcher=spawnSync(process.execPath,[path.join(ui,'launcher.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(launcher.status,0,'launcher.test.cjs: '+launcher.error);
const launchClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_LAUNCH:'1'}});
assert.equal(launchClient.status,0,'launcher client: '+launchClient.error);
const startup=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_STARTUP:'1'}});
assert.equal(startup.status,0,'startup client: '+startup.error);
acorn.parse(fs.readFileSync(path.join(ui,'session-exit.js'),'utf8'),options);
const exit=spawnSync(process.execPath,[path.join(ui,'session-exit.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(exit.status,0,'session-exit.test.cjs: '+exit.error);
const exitClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_EXIT:'1'}});
const modeClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_MODE:'1'}});
assert.equal(modeClient.status,0,'deck mode client: '+modeClient.error);
assert.equal(exitClient.status,0,'session exit client: '+exitClient.error);
const exitFailed=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_EXIT:'1',FLOE_TEST_EXIT_FAILURE:'1'}});
assert.equal(exitFailed.status,0,'unconfirmed session exit client: '+exitFailed.error);
for(const boundary of ['current','hide','restore','replace','exit','exit-failure','absent','closed','notify']){
    for(const reply of ['absent','closed','notify'].includes(boundary)?['202']:['202','503','401']){
        const run=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,
            FLOE_TEST_CLOSE_BOUNDARY:boundary==='exit-failure'?'exit':boundary,FLOE_TEST_CLOSE_REPLY:reply,
            FLOE_TEST_EXIT_FAILURE:boundary==='exit-failure'?'1':'0'}});
        assert.equal(run.status,0,'view close '+boundary+'/'+reply+': '+run.error);
    }
}
for(const kind of ['restore','close','socket','snapshot','phase','current']){
    for(const reply of ['old','closed','missing','503','401',...(['snapshot','phase','current'].includes(kind)?['newer']:[])]){
        const run=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,
            FLOE_TEST_VIEW_READ_RACE:kind,FLOE_TEST_VIEW_READ_REPLY:reply}});
        assert.equal(run.status,0,'view read order '+kind+'/'+reply+': '+run.error);
    }
}
const startupReads=['capabilities','catalog','drc','exports','defaults','operations','view','startup'].map(name=>['GET /api/v1/'+name,1,false]);
startupReads.push(['GET /api/v1/operations',2,false],['GET /api/v1/operations',3,false],['GET /api/v1/view',2,false],['GET /api/v1/startup',1,true],
    ['POST /api/v1/session/exchange',1,false],['POST /api/v1/operations',1,false]);
for(const [stage,match,levels] of startupReads){
    for(const boundary of ['hide','restore'])for(const reply of stage==='POST /api/v1/operations'?['200','503','401','unknown']:['200','503','401']){
        const run=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,
            FLOE_TEST_DEFAULTS:'1',FLOE_TEST_CLIP:'1',FLOE_TEST_STARTUP:levels?'1':'0',
            FLOE_TEST_STARTUP_SUSPEND:stage,FLOE_TEST_STARTUP_MATCH:String(match),FLOE_TEST_STARTUP_BOUNDARY:boundary,FLOE_TEST_STARTUP_REPLY:reply}});
        assert.equal(run.status,0,'startup suspend '+stage+'/'+match+'/'+levels+'/'+boundary+'/'+reply+': '+run.error);
    }
}
for(const endpoint of ['catalog','operations','view']){
    for(const boundary of ['exit','exit-failure','hide'])for(const reply of ['200','503','401']){
        const run=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,
            FLOE_TEST_LAUNCH:'1',FLOE_TEST_LAUNCH_EXIT:'/api/v1/'+endpoint,FLOE_TEST_LAUNCH_BOUNDARY:boundary,
            FLOE_TEST_EXIT_FAILURE:boundary==='exit-failure'?'1':'0',FLOE_TEST_LAUNCH_REPLY:reply}});
        assert.equal(run.status,0,'launcher preflight '+endpoint+'/'+boundary+'/'+reply+': '+run.error);
    }
}
for(const stage of ['drc','index','/api/v1/operations','/api/v1/view','picker','launcher']){
    for(const boundary of ['exit','exit-failure','hide','replace']){
        for(const failure of stage.startsWith('/api/')?['0','1','401']:['0','1']){
            const run=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,
                FLOE_TEST_RESUME_FENCE:stage,FLOE_TEST_RESUME_BOUNDARY:boundary==='exit-failure'?'exit':boundary,
                FLOE_TEST_EXIT_FAILURE:boundary==='exit-failure'?'1':'0',FLOE_TEST_RESUME_FAILURE:failure}});
            assert.equal(run.status,0,'BFCache resume '+stage+' '+boundary+' '+failure+': '+run.error);
        }
    }
}
for(const endpoint of ['operations','view']){
    const run=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,
        FLOE_TEST_RESUME_FENCE:'/api/v1/'+endpoint,FLOE_TEST_RESUME_BOUNDARY:'current',FLOE_TEST_RESUME_FAILURE:'401'}});
    assert.equal(run.status,0,'current BFCache 401 '+endpoint+': '+run.error);
}
for(const endpoint of ['catalog','defaults','operations','view','startup']){
    for(const failure of ['0','1'])for(const readFailure of ['0','1','401']){
        const run=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,
            FLOE_TEST_STARTUP:'1',FLOE_TEST_DEFAULTS:'1',FLOE_TEST_EXIT_STARTUP:'/api/v1/'+endpoint,
            FLOE_TEST_EXIT_FAILURE:failure,FLOE_TEST_EXIT_STARTUP_FAILURE:readFailure}});
        assert.equal(run.status,0,'startup exit '+endpoint+'/'+failure+'/'+readFailure+': '+run.error);
    }
}
acorn.parse(fs.readFileSync(path.join(ui, 'minimap.js'), 'utf8'), options);
const minimap=spawnSync(process.execPath,[path.join(ui,'minimap.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(minimap.status,0,'minimap.test.cjs: '+minimap.error);
const minimapClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_MINIMAP:'1'}});
assert.equal(minimapClient.status,0,'minimap client: '+minimapClient.error);
acorn.parse(fs.readFileSync(path.join(ui, 'about.js'), 'utf8'), options);
acorn.parse(fs.readFileSync(path.join(ui, 'notices.js'), 'utf8'), options);
const notices = spawnSync(process.execPath, [path.join(ui, 'notices.test.cjs')], {stdio:'inherit',timeout:15000});
assert.equal(notices.status, 0, 'notices.test.cjs: ' + notices.error);
const about = spawnSync(process.execPath, [path.join(ui, 'about.test.cjs')], {stdio:'inherit',timeout:15000});
assert.equal(about.status, 0, 'about.test.cjs: ' + about.error);
const settings = spawnSync(process.execPath, [path.join(ui, 'settings.test.cjs')], {stdio:'inherit',timeout:15000});
assert.equal(settings.status, 0, 'settings.test.cjs: ' + settings.error);
acorn.parse(fs.readFileSync(path.join(ui, 'settings.js'), 'utf8'), options);
acorn.parse(fs.readFileSync(path.join(ui, 'drc-note-display.js'), 'utf8'), options);
acorn.parse(fs.readFileSync(path.join(ui, 'drc-transfer.js'), 'utf8'), options);
const transfers = spawnSync(process.execPath, [path.join(ui, 'drc-transfer.test.cjs')], {stdio:'inherit',timeout:15000});
assert.equal(transfers.status, 0, 'drc-transfer.test.cjs: ' + transfers.error);
const transferPanel = spawnSync(process.execPath, [path.join(ui, 'drc-transfer-panel.test.cjs')], {stdio:'inherit',timeout:15000});
assert.equal(transferPanel.status, 0, 'drc-transfer-panel.test.cjs: ' + transferPanel.error);
for (const file of ['drc-note-display.test.cjs', 'drc-note-display-panel.test.cjs']) {
    const run = spawnSync(process.execPath, [path.join(ui, file)], {stdio:'inherit',timeout:15000});
    assert.equal(run.status, 0, file + ': ' + run.error);
}
const readReviewer = spawnSync(process.execPath, [path.join(ui, 'drc-note-display-panel.test.cjs'), '--read-only'], {stdio:'inherit',timeout:15000});
assert.equal(readReviewer.status, 0, 'read-only reviewer panel: ' + readReviewer.error);
for (const file of ['protocol.js', 'gestures.js', 'query.js', 'inspect.js', 'measure.js', 'clip.js', 'snapshot.js', 'defaults.js', 'panel-state.js', 'rulers.js', 'drc-groups.js', 'drc-build.js', 'drc-notes.js', 'drc-waives.js', 'drc.js', 'app.js']) {
    acorn.parse(fs.readFileSync(path.join(ui, file), 'utf8'), options);
}
for (const newer of ['const x = a?.b;', 'const x = 1n;', 'const x = {...a};']) {
    assert.throws(() => acorn.parse(newer, options), 'ES2017 gate accepted newer syntax');
}
for (const file of ['protocol.test.cjs', 'gestures.test.cjs', 'client.test.cjs', 'drc.test.cjs', 'drc-navigation.test.cjs', 'drc-markers.test.cjs', 'rulers.test.cjs', 'drc-cd.test.cjs', 'drc-groups.test.cjs', 'drc-box.test.cjs', 'drc-filters.test.cjs', 'drc-svrf.test.cjs', 'drc-isolation.test.cjs', 'panel-state.test.cjs']) {
    const run = spawnSync(process.execPath, [path.join(ui, file)], {stdio: 'inherit', timeout: 15000});
    assert.equal(run.status, 0, file + ': ' + run.error);
}
for (const file of ['drc-build.test.cjs', 'drc-build-panel.test.cjs', 'drc-notes.test.cjs', 'drc-notes-panel.test.cjs', 'drc-waives.test.cjs', 'drc-waives-panel.test.cjs', 'drc-detach.test.cjs', 'clip.test.cjs', 'snapshot.test.cjs', 'defaults.test.cjs']) {
    const build = spawnSync(process.execPath, [path.join(ui, file)], {stdio:'inherit',timeout:15000});
    assert.equal(build.status, 0, file + ': ' + build.error);
}
const queries = spawnSync(process.execPath, [path.join(ui, 'query.test.cjs')], {stdio:'inherit',timeout:15000});
assert.equal(queries.status, 0, 'query.test.cjs: ' + queries.error);
const inspect = spawnSync(process.execPath, [path.join(ui, 'inspect.test.cjs')], {stdio:'inherit',timeout:15000});
assert.equal(inspect.status, 0, 'inspect.test.cjs: ' + inspect.error);
const measure = spawnSync(process.execPath, [path.join(ui, 'measure.test.cjs')], {stdio:'inherit',timeout:15000});
assert.equal(measure.status, 0, 'measure.test.cjs: ' + measure.error);
const clipClient = spawnSync(process.execPath, [path.join(ui, 'client.test.cjs')], {stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_CLIP:'1'}});
assert.equal(clipClient.status, 0, 'clip full client: ' + clipClient.error);
const snapshotClient = spawnSync(process.execPath, [path.join(ui, 'client.test.cjs')], {stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_SNAPSHOT:'1'}});
assert.equal(snapshotClient.status, 0, 'snapshot full client: ' + snapshotClient.error);
const settingsClient = spawnSync(process.execPath, [path.join(ui, 'client.test.cjs')], {stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_SETTINGS:'1'}});
assert.equal(settingsClient.status, 0, 'settings full client: ' + settingsClient.error);
const defaultsClient = spawnSync(process.execPath, [path.join(ui, 'client.test.cjs')], {stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_DEFAULTS:'1'}});
assert.equal(defaultsClient.status, 0, 'defaults full client: ' + defaultsClient.error);
const shared = spawnSync(process.execPath, [path.join(ui, 'drc-cd.test.cjs')], {stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_SHARED_RULERS:'1'}});
assert.equal(shared.status, 0, 'shared CD rulers: ' + shared.error);
const ascii = spawnSync(process.execPath, [path.join(ui, 'drc.test.cjs')], {stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_ASCII:'1'}});
if (ascii.status !== 0) { process.exit(ascii.status || 1); }
console.log('WEB UI: ALL OK (ES2017 parse + deterministic client/protocol tests)');
