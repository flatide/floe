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
for (const file of ['protocol.js', 'gestures.js', 'query.js', 'inspect.js', 'measure.js', 'clip.js', 'snapshot.js', 'panel-state.js', 'rulers.js', 'drc-groups.js', 'drc-build.js', 'drc.js', 'app.js']) {
    acorn.parse(fs.readFileSync(path.join(ui, file), 'utf8'), options);
}
for (const newer of ['const x = a?.b;', 'const x = 1n;', 'const x = {...a};']) {
    assert.throws(() => acorn.parse(newer, options), 'ES2017 gate accepted newer syntax');
}
for (const file of ['protocol.test.cjs', 'gestures.test.cjs', 'client.test.cjs', 'drc.test.cjs', 'drc-navigation.test.cjs', 'drc-markers.test.cjs', 'rulers.test.cjs', 'drc-cd.test.cjs', 'drc-groups.test.cjs', 'drc-box.test.cjs', 'drc-filters.test.cjs', 'drc-svrf.test.cjs', 'drc-isolation.test.cjs', 'panel-state.test.cjs']) {
    const run = spawnSync(process.execPath, [path.join(ui, file)], {stdio: 'inherit', timeout: 15000});
    assert.equal(run.status, 0, file + ': ' + run.error);
}
for (const file of ['drc-build.test.cjs', 'drc-build-panel.test.cjs', 'clip.test.cjs', 'snapshot.test.cjs']) {
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
const shared = spawnSync(process.execPath, [path.join(ui, 'drc-cd.test.cjs')], {stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_SHARED_RULERS:'1'}});
assert.equal(shared.status, 0, 'shared CD rulers: ' + shared.error);
const ascii = spawnSync(process.execPath, [path.join(ui, 'drc.test.cjs')], {stdio:'inherit',timeout:15000,env:{...process.env,FLOE_TEST_ASCII:'1'}});
if (ascii.status !== 0) { process.exit(ascii.status || 1); }
console.log('WEB UI: ALL OK (ES2017 parse + deterministic client/protocol tests)');
