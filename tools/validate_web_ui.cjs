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
for (const file of ['protocol.js', 'gestures.js', 'app.js']) {
    acorn.parse(fs.readFileSync(path.join(ui, file), 'utf8'), options);
}
for (const newer of ['const x = a?.b;', 'const x = 1n;', 'const x = {...a};']) {
    assert.throws(() => acorn.parse(newer, options), 'ES2017 gate accepted newer syntax');
}
for (const file of ['protocol.test.cjs', 'gestures.test.cjs', 'client.test.cjs']) {
    const run = spawnSync(process.execPath, [path.join(ui, file)], {stdio: 'inherit', timeout: 15000});
    assert.equal(run.status, 0, file + ': ' + run.error);
}
console.log('WEB UI: ALL OK (ES2017 parse + deterministic client/protocol tests)');
