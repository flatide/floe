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
const doc = {createElement: tag => new Element(tag), addEventListener(k, f) { listeners[k] = f; }, get activeElement() { return focused; }};
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
// A second bar shaped like the shell's View and Jobdeck menus: two submenus
// beside plain items, a disabled item, a hidden group between separators.
let fileOk = true, offOk = false, hideDeck = true;
el('detail').value = 'medium'; el('thin').value = 'auto'; el('frames').checked = true;
const bar2 = Menubar.bind({el, document: doc, window: {}, focus: () => focusCanvas.push(1), model: [
    {name: 'File', items: [{label: 'Open', enabled: () => fileOk, action: () => fired.push('open')}]},
    {name: 'View', items: [
        {label: 'Detail', key: 'd', sub: [{label: 'Low', radio: {id: 'detail', value: 'low'}}, {label: 'Medium', radio: {id: 'detail', value: 'medium'}}]},
        {label: 'Thin', sub: [{label: 'Auto', radio: {id: 'thin', value: 'auto'}}, {label: 'Keep', radio: {id: 'thin', value: 'keep'}}]},
        {sep: true},
        {label: 'Frames', key: 'f', toggle: 'frames'},
        {label: 'Off', enabled: () => offOk, action: () => fired.push('off')},
        {label: 'Tail', action: () => fired.push('tail')}
    ]},
    {name: 'Deck', items: [
        {label: 'Level', hidden: () => hideDeck, radio: {id: 'mode', value: 'level'}},
        {sep: true},
        {label: 'Toggle', hidden: () => hideDeck, action: () => fired.push('toggle')},
        {sep: true},
        {label: 'Levels…', action: () => fired.push('levels')},
        {sep: true},
        {label: 'Share', proxy: 'share'}
    ]},
    {name: 'Tools', items: [{label: 'Nest', sub: [{label: 'Deeper', sub: [{label: 'Leaf', action: () => fired.push('leaf')}]}, {label: 'Side', action: () => fired.push('side')}]}]}
]});
const [file2, view2, deck, tools] = el('menubar').children;
const byLabel = (panel, text) => panel.children.map(c => c.className === 'menu-subwrap' ? c.children[0] : c).find(n => n.children[1] && n.children[1].textContent === text);
const subOf = owner => owner.parent.children[1];
const vp = view2.children[1], dp = deck.children[1];
const detail = byLabel(vp, 'Detail'), thin = byLabel(vp, 'Thin'), frames = byLabel(vp, 'Frames'), off = byLabel(vp, 'Off'), tail = byLabel(vp, 'Tail');
const detailSub = subOf(detail), thinSub = subOf(thin), [low, medium] = detailSub.children;
// A submenu owner shows its key hint beside the arrow.
assert.equal(detail.children[2].textContent, 'd ▸'); assert.equal(thin.children[2].textContent, '▸');
// Hover and the arrows move one selection, as in GTK: a hover that closes the
// submenu holding the focus takes the focus, and moving the focus off an
// owner closes the submenu the pointer opened.
bar2.open('View'); key(detail, 'ArrowRight'); assert.equal(focused, low);
frames.onmouseenter(); assert.equal(detailSub.hidden, true); assert.equal(focused, frames, 'focus never stays in a panel the pointer closed');
key(frames, 'ArrowDown'); assert.equal(focused, tail);
key(tail, 'Home'); key(detail, 'ArrowRight'); thin.onmouseenter();
assert.equal(detailSub.hidden, true); assert.equal(thinSub.hidden, false); assert.equal(focused, thin, 'hovering another owner takes the focus');
detail.onmouseenter(); assert.equal(focused, detail); assert.equal(detailSub.hidden, false); assert.equal(thinSub.hidden, true);
key(detail, 'ArrowDown'); assert.equal(focused, thin); assert.equal(detailSub.hidden, true, 'the arrows close a hover-opened submenu');
detail.onmouseenter(); key(detail, 'End'); assert.equal(focused, tail); assert.equal(detailSub.hidden, true, 'End too');
detail.onmouseenter(); medium.onmouseenter(); assert.equal(focused, medium); assert.equal(detailSub.hidden, false, 'hovering inside a submenu keeps it');
off.onmouseenter(); assert.equal(focused, medium); assert.equal(detailSub.hidden, false, 'a disabled item cannot take the selection');
bar2.close(false);
const nest = byLabel(tools.children[1], 'Nest'), deeper = byLabel(subOf(nest), 'Deeper');
bar2.open('Tools'); nest.onmouseenter(); deeper.onmouseenter();
assert.equal(subOf(nest).hidden, false, 'a nested submenu keeps its parent open'); assert.equal(subOf(deeper).hidden, false);
byLabel(subOf(nest), 'Side').onmouseenter(); assert.equal(subOf(deeper).hidden, true); assert.equal(subOf(nest).hidden, false);
bar2.close(false);
// Every path that hides a submenu resets its owner's aria-expanded.
const outside = new Element();
const closers = {
    'close()': () => bar2.close(true), 'activation': () => key(medium, 'Enter'), 'ArrowRight to the next menu': () => key(low, 'ArrowRight'),
    'outside mousedown': () => listeners.mousedown({target: outside}), 'focus outside': () => listeners.focusin({target: outside}),
    'Escape outside': () => listeners.keydown({key: 'Escape', target: outside}), 'title click': () => view2.children[0].click(),
    'title hover': () => deck.children[0].onmouseenter(), 'sibling hover': () => frames.onmouseenter()
};
Object.keys(closers).forEach(name => {
    bar2.open('View'); key(detail, 'ArrowRight'); assert.equal(detail.getAttribute('aria-expanded'), 'true');
    closers[name](); assert.equal(detailSub.hidden, true, name); assert.equal(detail.getAttribute('aria-expanded'), 'false', name);
    bar2.open('View'); assert.equal(detail.getAttribute('aria-expanded'), 'false', name + ', reopened'); bar2.close(false);
});
// Separators never lead, trail or double up once hidden items resolve.
const shown = () => dp.children.filter(n => !n.hidden).map(n => n.className === 'menu-sep' ? '--' : n.children[1].textContent);
el('share').hidden = true; bar2.open('Deck');
assert.deepEqual(shown(), ['Levels…'], 'a non-deck Jobdeck menu has no bare separators'); assert.equal(focused, byLabel(dp, 'Levels…'));
hideDeck = false; el('share').hidden = false; bar2.refresh(); assert.deepEqual(shown(), ['Level', '--', 'Toggle', '--', 'Levels…', '--', 'Share']);
hideDeck = true; bar2.refresh(); assert.deepEqual(shown(), ['Levels…', '--', 'Share']);
bar2.close(false);
// The keyboard does not open a submenu with no usable item; the pointer does.
el('thin').disabled = true; bar2.open('View'); key(detail, 'ArrowDown');
key(thin, 'ArrowRight'); assert.equal(thinSub.hidden, true); assert.equal(thin.getAttribute('aria-expanded'), 'false'); assert.equal(focused, thin);
key(thin, 'Enter'); assert.equal(thinSub.hidden, true); assert.equal(focused, thin);
thin.onmouseenter(); assert.equal(thinSub.hidden, false); assert.equal(thinSub.children[0].disabled, true);
el('thin').disabled = false; bar2.close(false);
// Activation re-checks the proxy, and refresh() moves the focus off an item it
// hid or disabled; the arrows still step from such an item to its neighbours.
bar2.open('View'); key(detail, 'ArrowDown'); key(thin, 'ArrowDown'); assert.equal(focused, frames);
const firedBefore = fired.length; el('frames').disabled = true; key(frames, 'Enter');
assert.equal(el('frames').checked, true); assert.equal(fired.length, firedBefore); assert.equal(bar2.isOpen(), true, 'a stale item grants nothing');
assert.equal(frames.disabled, true); assert.equal(focused, tail);
el('frames').disabled = false; offOk = true; bar2.refresh(); assert.equal(focused, tail);
key(tail, 'ArrowUp'); key(off, 'ArrowUp'); assert.equal(focused, frames);
el('frames').disabled = true; bar2.refresh(); assert.equal(focused, off, 'refresh() moves the focus to the next usable item');
key(frames, 'ArrowUp'); assert.equal(focused, thin); key(frames, 'ArrowDown'); assert.equal(focused, off);
el('frames').disabled = false; offOk = false;
key(off, 'Home'); key(detail, 'ArrowRight'); el('detail').disabled = true; bar2.refresh();
assert.equal(focused, detail, 'an emptied submenu hands the focus back to its owner'); assert.equal(detailSub.hidden, true); assert.equal(detail.getAttribute('aria-expanded'), 'false');
el('detail').disabled = false; bar2.close(false);
hideDeck = false; bar2.open('Deck'); const toggle = byLabel(dp, 'Toggle'); key(focused, 'ArrowDown'); assert.equal(focused, toggle);
hideDeck = true; bar2.refresh(); assert.equal(focused, byLabel(dp, 'Levels…'), 'refresh() moves the focus off an item it hid');
key(toggle, 'Enter'); assert.equal(fired.includes('toggle'), false, 'a hidden item never activates'); assert.equal(bar2.isOpen(), true);
bar2.open('File'); fileOk = false; bar2.refresh(); assert.equal(focused, file2.children[0], 'with nothing usable left the title holds the focus');
bar2.open('View'); key(detail, 'ArrowLeft'); assert.equal(focused, file2.children[0], 'also when the arrows switch to such a menu');
fileOk = true; bar2.close(true);
console.log('WEB MENUBAR: ALL OK (proxies, hidden/disabled, arrows, submenu enter/leave/escape, radio/toggle, one hover/keyboard selection, aria-expanded, separators, stale items, key hints)');
