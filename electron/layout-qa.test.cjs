'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { ownedMemory } = require('./layout-qa.cjs');
test('margin-dependent QA explicitly opts in without changing ordinary launch', () => {
  const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
  const main = fs.readFileSync(path.join(__dirname, 'main.cjs'), 'utf8');
  const start = main.indexOf('  const viewArgs =');
  const end = main.indexOf('  if (emptySmoke || signalSmoke || downloadSmoke) {', start);
  assert.ok(start >= 0 && end > start);
  const flags = ['layoutSmoke', 'paritySmoke', 'clipboardSmoke', 'clipSmoke', 'recoverySmoke',
    'emptySmoke', 'signalSmoke', 'downloadSmoke'];
  const script = main.slice(start, end) + '\nJSON.stringify(viewArgs);';
  for (const mode of ['normal', ...flags]) {
    const context = { args: mode === 'normal' ? ['view', '/synthetic.oas'] : ['--qa', '/synthetic.oas'],
      smoke: mode !== 'normal', ...Object.fromEntries(flags.map(flag => [flag, flag === mode])) };
    const args = JSON.parse(vm.runInNewContext(script, context));
    const margin = args.indexOf('--margin');
    if (['layoutSmoke', 'paritySmoke', 'recoverySmoke'].includes(mode)) {
      assert.ok(margin > 0);
      assert.equal(args[margin + 1], 'on');
    } else assert.equal(margin, -1);
    if (mode === 'normal') assert.deepEqual(args, ['/synthetic.oas']);
    else assert.ok(args.includes('--multi'));
  }
});
test('numeric memory sample includes only the live owned service tree, in any row order', () => {
  const text = ' 22 21 300\n99 1 9000\n21 20 200\n20 10 100\n10 1 50\n';
  assert.deepEqual(ownedMemory(text, 20), { process_count: 3, rss_sum_kib: 600 });
  assert.deepEqual(ownedMemory(text, 22), { process_count: 1, rss_sum_kib: 300 });
  assert.deepEqual(ownedMemory(text, [20, 22, 99]), { process_count: 4, rss_sum_kib: 9600 });
});
test('missing service and malformed metrics fail, never report zero-cost success', () => {
  for (const value of ['', '20 1 NaN', '20 1 -1', '20 1 0.5', '20 1 1 extra', '21 1 1']) {
    assert.throws(() => ownedMemory(value, 20));
  }
  assert.throws(() => ownedMemory('20 1 10', 0));
});
