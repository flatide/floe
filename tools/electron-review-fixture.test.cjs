'use strict';
const test = require('node:test'), assert = require('node:assert/strict');
const { createHash } = require('node:crypto');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { spawnSync } = require('node:child_process');
const { oasis, drc, writeInputs } = require('./electron-review-fixture.cjs');
test('shared synthetic inputs remain byte-identical to the original Chromium recovery fixture', () => {
  assert.equal(oasis().length, 313);
  assert.equal(createHash('sha256').update(oasis()).digest('hex'), 'ff86dea6e0f1c1cc7c0e301f21e977a5614b2665cbaae131d2c41ffe1894a741');
  assert.equal(createHash('sha256').update(drc).digest('hex'), 'd84e8facf62757a88235b967cf066f35cf20c5c8e4a392a81f1a40b947aef775');
});
test('fixture writer only creates two new private files and never replaces them', t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'floe-review-fixture-'));
  t.after(() => fs.rmSync(root, { recursive: true }));
  writeInputs(root);
  assert.deepEqual(fs.readdirSync(root).sort(), ['synthetic.db', 'synthetic.oas']);
  for (const name of fs.readdirSync(root)) assert.equal(fs.statSync(path.join(root, name)).mode & 0o777, 0o600);
  assert.throws(() => writeInputs(root), { code: 'EEXIST' });
  assert.deepEqual(fs.readFileSync(path.join(root, 'synthetic.oas')), oasis());
  assert.equal(fs.readFileSync(path.join(root, 'synthetic.db'), 'utf8'), drc);
});
test('service-crash entry refuses arbitrary file inputs before starting any fixture or process', () => {
  const result = spawnSync(process.execPath, [path.join(__dirname, 'validate_electron_service_review.cjs'), '/not-an-input'], {
    env: { PATH: '' }, encoding: 'utf8', timeout: 5000
  });
  assert.equal(result.status, 1); assert.equal(result.stdout, '');
});
