'use strict';
const fs = require('node:fs');
const path = require('node:path');
// Shared fixed synthetic input, not an OASIS/DRC implementation. Encoding mirrors
// rust/oasis/src/write.rs and the existing desktop review QA rectangle.
function oasis() {
  const header = Buffer.from('%SEMI-OASIS\r\n', 'ascii');
  const start = Buffer.from([1, 3, 49, 46, 48, 7, 0, 0, 0, 0, 0, 64, 143, 64]);
  const rect = Buffer.from([14, 3, 84, 79, 80, 20, 0x7b, 1, 0, 0xc0, 0xb8, 2, 0xb0, 0xea, 1, 0, 0]);
  return Buffer.concat([header, start, Buffer.alloc(13), rect, Buffer.from([2, 252, 1]), Buffer.alloc(253)]);
}
const drc = 'TOP 1000\nSYNTHETIC.SPACE\n2 2 1 Sep 21 00:00:00 2026\nSynthetic native recovery only.\np 1 4\n10000 10000\n11000 10000\n11000 11000\n10000 11000\np 2 4\n20000 10000\n21000 10000\n21000 11000\n20000 11000\n';
function writeInputs(root) {
  for (const [name, bytes] of [['synthetic.oas', oasis()], ['synthetic.db', drc]]) {
    fs.writeFileSync(path.join(root, name), bytes, { flag: 'wx', mode: 0o600 });
  }
}
module.exports = { oasis, drc, writeInputs };
