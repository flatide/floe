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
acorn.parse(fs.readFileSync(path.join(ui, 'presets.js'), 'utf8'), options);
const presets=spawnSync(process.execPath,[path.join(ui,'presets.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(presets.status,0,'presets.test.cjs: '+presets.error);
acorn.parse(fs.readFileSync(path.join(ui, 'palette.js'), 'utf8'), options);
const palette=spawnSync(process.execPath,[path.join(ui,'palette.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(palette.status,0,'palette.test.cjs: '+palette.error);
const paletteClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_PALETTE:'1'}});
assert.equal(paletteClient.status,0,'palette client: '+paletteClient.error);
const gotoClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_GOTO:'1'}});
assert.equal(gotoClient.status,0,'goto client: '+gotoClient.error);
acorn.parse(fs.readFileSync(path.join(ui, 'index-open.js'), 'utf8'), options);
const indexOpen=spawnSync(process.execPath,[path.join(ui,'index-open.test.cjs')],{stdio:'inherit',timeout:15000});
assert.equal(indexOpen.status,0,'index-open.test.cjs: '+indexOpen.error);
const indexClient=spawnSync(process.execPath,[path.join(ui,'client.test.cjs')],{stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_INDEX_OPEN:'1'}});
assert.equal(indexClient.status,0,'index/open client: '+indexClient.error);
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
for (const file of ['drc-build.test.cjs', 'drc-build-panel.test.cjs', 'drc-notes.test.cjs', 'drc-notes-panel.test.cjs', 'drc-waives.test.cjs', 'drc-waives-panel.test.cjs', 'clip.test.cjs', 'snapshot.test.cjs', 'defaults.test.cjs']) {
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
