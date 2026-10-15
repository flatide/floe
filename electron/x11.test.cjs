'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { configureX11 } = require('./x11.cjs');
test('only the explicit Linux ETX entry selects X11 without other switches', () => {
  for (const platform of ['linux', 'darwin']) {
    for (const value of [undefined, '', '0', '1']) {
      const calls = [], env = Object.freeze({ FLOE_ELECTRON_X11: value, DISPLAY: 'synthetic:1' });
      configureX11({ appendSwitch: (...args) => calls.push(args) }, platform, env);
      assert.deepEqual(calls, platform === 'linux' && value === '1' ? [['ozone-platform', 'x11']] : []);
    }
  }
});
