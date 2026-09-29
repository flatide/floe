'use strict';
// Deterministic menu bar: proxies are plain stub controls, no DOM library.
const assert = require('node:assert/strict');
const Menubar = require('./menubar.js');
class Element {
    constructor(tag) { this.tag = tag || 'div'; this.children = []; this.attributes = {}; this.hidden = false; this.disabled = false; this.value = ''; this.checked = false; this.className = ''; this.style = {}; }
    appendChild(c) { c.parent = this; this.children.push(c); return c; }
    setAttribute(k, v) { this.attributes[k] = String(v); }
    getAttribute(k) { return this.attributes[k] === undefined ? null : this.attributes[k]; }
    contains(n) { return this === n || this.children.some(c => c.contains(n)); }
    focus() { focused = this; }
    click() { this.clicks = (this.clicks || 0) + 1; if (this.onclick) { this.onclick({preventDefault() {}}); } }
    get textContent() { return this._text || ''; } set textContent(v) { this._text = v; this.children = []; }
}
let focused = null;
const nodes = new Map(), listeners = {};
const el = id => { if (!nodes.has(id)) { nodes.set(id, new Element('button')); } return nodes.get(id); };
const doc = {createElement: tag => new Element(tag), addEventListener(k, f) { listeners[k] = f; }};
el('fit').disabled = false; el('detail').value = 'medium'; el('frames').checked = true; el('hidden-proxy').hidden = true;
let fired = [];
el('detail').onchange = () => fired.push('detail=' + el('detail').value);
el('frames').onchange = () => fired.push('frames=' + el('frames').checked);
el('fit').onclick = () => fired.push('fit');
const focusCanvas = [];
const bar = Menubar.bind({el, document: doc, window: {}, focus: () => focusCanvas.push(1), model: [
    {name: 'File', items: [{label: 'Fit', proxy: 'fit', key: 'Ctrl+A'}, {sep: true}, {label: 'Gone', proxy: 'hidden-proxy'}, {label: 'Custom', action: () => fired.push('custom')}]},
    {name: 'View', items: [
        {label: 'Detail', sub: [{label: 'Low', radio: {id: 'detail', value: 'low'}}, {label: 'Medium', radio: {id: 'detail', value: 'medium'}}]},
        {label: 'Frames', key: 'f', toggle: 'frames'},
        {label: 'Off', enabled: () => false, action: () => fired.push('never')}
    ]}
]});
const menus = el('menubar').children;
assert.deepEqual(menus.map(m => m.children[0].textContent), ['File', 'View']);
const [file, view] = menus;
const key = (node, k) => node.onkeydown({key: k, preventDefault() {}});
// Opening a menu focuses its first usable item; hidden proxies hide their items.
file.children[0].click();
assert.equal(file.children[1].hidden, false); assert.equal(bar.isOpen(), true);
const fileItems = file.children[1].children;
assert.equal(fileItems[0].children[1].textContent, 'Fit'); assert.equal(focused, fileItems[0]);
assert.equal(fileItems[2].hidden, true, 'hidden proxy hides its item');
key(fileItems[0], 'ArrowDown'); assert.equal(focused, fileItems[3], 'separator and hidden item are skipped');
key(fileItems[3], 'ArrowUp'); assert.equal(focused, fileItems[0]);
key(fileItems[0], 'Enter'); assert.deepEqual(fired, ['fit']); assert.equal(bar.isOpen(), false); assert.equal(focusCanvas.length, 1);
// Arrow keys move between top menus; a submenu is entered, navigated and left by keyboard.
file.children[0].click(); key(fileItems[0], 'ArrowRight');
assert.equal(file.children[1].hidden, true); assert.equal(view.children[1].hidden, false);
const viewItems = view.children[1].children, detailWrap = viewItems[0], detailNode = detailWrap.children[0], subPanel = detailWrap.children[1];
assert.equal(focused, detailNode); assert.equal(subPanel.hidden, true);
key(detailNode, 'ArrowRight');
assert.equal(subPanel.hidden, false, 'submenu opens'); assert.equal(focused, subPanel.children[0], 'focus enters the submenu');
assert.equal(subPanel.children[1].attributes['aria-checked'], 'true', 'radio state read from the proxy on open');
key(subPanel.children[0], 'ArrowDown'); assert.equal(focused, subPanel.children[1]);
key(subPanel.children[1], 'ArrowLeft'); assert.equal(subPanel.hidden, true); assert.equal(focused, detailNode, 'ArrowLeft returns to the owner');
key(detailNode, 'Enter'); assert.equal(focused, subPanel.children[0]);
key(subPanel.children[0], 'Escape'); assert.equal(subPanel.hidden, true); assert.equal(focused, detailNode); assert.equal(bar.isOpen(), true, 'Escape in a submenu closes only the submenu');
key(detailNode, 'ArrowRight'); key(subPanel.children[0], 'Enter');
assert.deepEqual(fired.slice(-1), ['detail=low']); assert.equal(el('detail').value, 'low'); assert.equal(bar.isOpen(), false);
// Toggle items flip the proxied checkbox; disabled items never activate.
view.children[0].click(); assert.equal(viewItems[1].attributes['aria-checked'], 'true');
viewItems[1].onclick({preventDefault() {}}); assert.deepEqual(fired.slice(-1), ['frames=false']); assert.equal(el('frames').checked, false);
view.children[0].click(); assert.equal(viewItems[2].disabled, true);
key(viewItems[1], 'End'); assert.equal(focused, viewItems[1], 'End skips the disabled last item');
key(viewItems[1], 'Escape'); assert.equal(bar.isOpen(), false); assert.equal(fired.includes('never'), false);
// Custom actions run after the menu closes; refresh re-reads proxies while open.
bar.open('File'); fileItems[3].onclick({preventDefault() {}}); assert.deepEqual(fired.slice(-1), ['custom']);
el('fit').disabled = true; bar.open('File'); assert.equal(fileItems[0].disabled, true); bar.close(true);
console.log('WEB MENUBAR: ALL OK (proxies, hidden/disabled, arrows, submenu enter/leave/escape, radio/toggle)');
