'use strict';
const test = require('node:test'), assert = require('node:assert/strict');
const { TerminalExit } = require('./terminal-exit.cjs');
test('error stays until acknowledged, then exits exactly once with failure', () => {
  const out = [], t = new TerminalExit(code => out.push(code));
  t.begin(true); t.complete(1); assert.deepEqual(out, []);
  t.request(); t.request(); t.complete(1); assert.deepEqual(out, [1]);
});
test('close, Quit, or signal before cleanup cannot skip its result', () => {
  const out = [], t = new TerminalExit(code => out.push(code));
  t.begin(true); t.request(); t.request(); assert.deepEqual(out, []);
  t.complete(1); assert.deepEqual(out, [1]);
});
test('normal and smoke shutdown do not require an error acknowledgement', () => {
  for (const code of [0, 1]) {
    const out = [], t = new TerminalExit(value => out.push(value));
    t.begin(false); assert.deepEqual(out, []); t.complete(code);
    assert.deepEqual(out, [code]);
  }
});
