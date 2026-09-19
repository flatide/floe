'use strict';
const assert = require('assert');
const fs = require('fs');
const vm = require('vm');
const script = '(' + fs.readFileSync(__dirname + '/menu-action.js', 'utf8') + ')';
let clicks = 0;
const button = {disabled: false, hidden: false, click() { clicks++; }};
let modal = false, missing = false;
const document = {
    hidden: false,
    querySelector(selector) {
        assert.strictEqual(selector, '[role="dialog"]:not([hidden])');
        return modal ? {} : null;
    },
    getElementById() { return missing ? null : button; }
};
const invoke = vm.runInNewContext(script, {document});
for (const id of ['browse-open', 'drc-open', 'about-open']) {
    assert.strictEqual(invoke(id), 'opened');
}
assert.strictEqual(clicks, 3);
for (const id of ['logout', 'index-confirm', 'save', 'review-save', "');evil('"]) {
    assert.strictEqual(invoke(id), 'unavailable');
}
modal = true;
assert.strictEqual(invoke('browse-open'), 'busy');
modal = false;
for (const target of [button, document]) {
    target.hidden = true;
    assert.strictEqual(invoke('browse-open'), 'unavailable');
    target.hidden = false;
}
button.disabled = true;
assert.strictEqual(invoke('browse-open'), 'unavailable');
button.disabled = false;
missing = true;
assert.strictEqual(invoke('browse-open'), 'unavailable');
assert.strictEqual(clicks, 3);
console.log('desktop menu: fixed actions, busy dialogs, visibility and availability OK');
