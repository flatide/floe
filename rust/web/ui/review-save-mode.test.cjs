'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),S=require('./review-save-mode.js');
const toggle={},hint={textContent:''};let changes=0;
const s=S.bind({toggle,hint,kind:'note',changed(){++changes;}});
function set(v){toggle.checked=v;toggle.onchange();}
assert(toggle.disabled);set(true);assert(!s.capture());
s.sync('fixed-owner','session-1',true);assert(!s.on());toggle.checked=true;assert(!s.capture(),'restored checkbox became permission');
s.sync('fixed-owner','session-1',true);assert(!toggle.checked);set(true);const first=s.capture();assert(first);assert(s.valid(first));
assert.match(hint.textContent,/fixed-owner/);assert(!s.valid({}));s.sync('fixed-owner','session-1',false);
assert(s.on());assert(!s.valid(first));assert(!toggle.disabled,'opt-out blocked during IO');set(false);assert(!s.on());assert(toggle.disabled);
s.sync('fixed-owner','session-1',true);set(true);assert(!s.valid(first),'off/on resurrected an older edit');
const next=s.capture();s.sync('other-owner','session-1',true);assert(!s.valid(next));assert(!s.on());set(true);
s.sync('other-owner','session-2',true);assert(!s.on());set(true);s.reset();assert(!s.on());assert(!s.capture());
assert.equal(changes,6);const html=fs.readFileSync(__dirname+'/index.html','utf8');
assert(html.indexOf('/review-save-mode.js')>=0);assert(html.indexOf('/review-save-mode.js')<html.indexOf('/drc-notes.js'));
assert(html.indexOf('/review-save-mode.js')<html.indexOf('/drc-waives.js'));
console.log('REVIEW SAVE MODE: ALL OK (default off, fixed reviewer/session, identity grants, pending opt-out, no resurrection/persistence)');
