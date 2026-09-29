'use strict';
// Deterministic cell tree UI: server answers are scripted, no DOM library.
const assert = require('node:assert/strict');
const Cells = require('./cells.js');
const tick = () => new Promise(r => setImmediate(r));
class Element {
    constructor(tag) { this.tag = tag || 'div'; this.children = []; this.style = {}; this.dataset = {}; this.attributes = {}; this.hidden = false; this.disabled = false; this.value = ''; this.checked = false; this.className = ''; this.width = this.height = 1; }
    appendChild(c) { this.children.push(c); return c; }
    setAttribute(k, v) { this.attributes[k] = String(v); }
    getAttribute(k) { return this.attributes[k] === undefined ? null : this.attributes[k]; }
    querySelectorAll(tag) { return this.children.flatMap(c => [...(c.tag === tag ? [c] : []), ...c.querySelectorAll(tag)]); }
    getContext() { const ops = this.ops = this.ops || []; return {clearRect() { ops.push('clear'); }, strokeRect(x, y, w, h) { ops.push(['rect', x, y, w, h]); }}; }
    focus() { focused = this; } select() {}
    get textContent() { return this._text || ''; } set textContent(v) { this._text = v; this.children = []; }
}
let focused = null;
function harness() {
    const nodes = new Map(), requests = [], edits = [], timers = [];
    const el = id => { if (!nodes.has(id)) { nodes.set(id, new Element(id === 'cells-canvas' ? 'canvas' : 'div')); } return nodes.get(id); };
    el('cells-highlight').checked = true;
    let context = null, raised = 0, focusedCanvas = 0;
    const state = {status: 'idle', bbox_dbu: ['0', '0', '1000', '500'], pixels: [200, 100], dbu_um: '0.001', capabilities: {cell_root: true}, root_name: ''};
    const c = Cells.bind({
        el, document: {createElement: tag => new Element(tag)},
        http(method, path, body) { return new Promise((resolve, reject) => { requests.push({method, path, body, resolve, reject}); }); },
        edit(body, done) { edits.push(body); if (done) { done(null); } },
        context: () => context, size: () => ({pixels: state.pixels}), unit: () => 0.001,
        focus: () => { focusedCanvas++; }, raise: () => { raised++; },
        rootAllowed: () => true, buildAllowed: () => false,
        setTimeout: (f, ms) => { timers.push({f, ms}); return timers.length; }, clearTimeout: n => { if (timers[n - 1]) { timers[n - 1].f = null; } }
    });
    return {c, el, requests, edits, timers, state,
        connect() { context = {id: 'v1', state, connected: true}; c.changed(); },
        disconnect() { context = null; c.changed(); },
        reply(i, value) { const r = requests[i]; r.resolve(value); return tick(); },
        fail(i, error) { const r = requests[i]; r.reject(error); return tick(); },
        rows() { return el('cells-tree').children.map(n => n.children.length ? n.children.map(x => x.textContent).join('|') : n.textContent); },
        raised: () => raised, focusedCanvas: () => focusedCanvas};
}
(async () => {
    const h = harness();
    assert.equal(h.el('cells-search').disabled, true);
    h.connect();
    assert.deepEqual(h.requests.map(r => [r.method, r.path, r.body.kind]), [['POST', '/api/v1/views/v1/cells', 'sources']]);
    await h.reply(0, {sources: [{src: 0, placements: 1, name: 'valmini.oas'}]});
    assert.equal(h.requests[1].body.kind, 'children'); assert.equal(h.requests[1].body.src, 0); assert.equal(h.requests[1].body.cell, undefined);
    await h.reply(1, {cell: 7, name: 'TOP', insts: 1, height: 3, unit: 0.001, bbox: [0, 0, 1000, 500], n: 2, total: 2,
        children: [{ci: 1, members: 4, leaf: false, name: 'BLK'}, {ci: 2, members: 1, leaf: true, name: 'VIA'}]});
    assert.deepEqual(h.rows(), ['▾|TOP|', '▸|BLK|×4', '|VIA|']);
    assert.equal(h.el('cells-info').textContent, 'TOP: 2 children');
    assert.equal(h.el('cells-search').disabled, false); assert.equal(h.el('cells-zoom').disabled, true);
    // Expanding a row loads its children once, inserted before the placeholder goes.
    const blk = h.el('cells-tree').children[1];
    blk.onclick({target: blk.children[0]});
    assert.deepEqual(h.rows(), ['▾|TOP|', '▾|BLK|×4', '…', '|VIA|']);
    assert.equal(h.requests[2].body.cell, 1);
    await h.reply(2, {cell: 1, name: 'BLK', insts: 4, height: 2, unit: 0.001, bbox: [0, 0, 100, 50], n: 1, total: 5, children: [{ci: 3, members: 2, leaf: true, name: 'M1'}]});
    assert.deepEqual(h.rows(), ['▾|TOP|', '▾|BLK|×4', '|M1|×2', '… 4 more (find by name)', '|VIA|']);
    // Selecting asks for the extent and the instances inside the current view.
    const m1 = h.el('cells-tree').children[2];
    m1.onclick({target: m1.children[1]});
    assert.equal(h.requests[3].body.kind, 'bbox'); assert.equal(h.requests[3].body.cell, 3);
    await h.reply(3, {insts: 2, approx: false, bbox: [10, 10, 30, 20]});
    assert.match(h.el('cells-info').textContent, /^M1: 2 instances · 0\.020000 × 0\.010000 um$/);
    assert.equal(h.requests[4].body.kind, 'insts'); assert.deepEqual(h.requests[4].body.view, [0, 0, 1000, 500]); assert.equal(h.requests[4].body.cap, 4096);
    await h.reply(4, {n: 1, more: false, visited: 3, boxes: [[10, 10, 30, 20]]});
    assert.equal(h.el('cells-canvas').hidden, false);
    const rect = h.el('cells-canvas').ops.find(o => Array.isArray(o));
    assert.deepEqual(rect, ['rect', 1.5, 94.5, 7, 7], 'sub-7px instance becomes a 7px square at its centre');
    assert.equal(h.el('cells-zoom').disabled, false); assert.equal(h.el('cells-root').disabled, false);
    // The same view does not re-ask; a moved view does.
    h.c.changed(); assert.equal(h.requests.length, 5);
    h.state.bbox_dbu = ['100', '0', '1100', '500']; h.c.changed();
    assert.equal(h.requests.length, 6); assert.equal(h.requests[5].body.kind, 'insts');
    await h.reply(5, {n: 0, more: true, visited: 1, boxes: []});
    // Zoom frames the selected cell at 80% of the view width.
    h.el('cells-zoom').onclick();
    assert.deepEqual(h.edits.at(-1), {navigation: {kind: 'goto', center_um: ['0.02', '0.015'], width_um: '0.025'}});
    assert.equal(h.focusedCanvas(), 1);
    // The view root is an explicit edit; the top button clears it.
    h.el('cells-root').onclick();
    assert.deepEqual(h.edits.at(-1), {root: {src: 0, cell: 3}}); assert.equal(h.c.rootName(), 'M1'); assert.equal(h.el('cells-top').disabled, false);
    h.el('cells-top').onclick(); assert.deepEqual(h.edits.at(-1), {root: null}); assert.equal(h.c.rootName(), '');
    // Search replaces the tree after the debounce and Escape restores it.
    h.el('cells-search').value = 'VI*'; h.el('cells-search').oninput();
    assert.equal(h.timers.at(-1).ms, 150); h.timers.at(-1).f();
    assert.equal(h.requests[6].body.kind, 'find'); assert.equal(h.requests[6].body.pattern, 'VI*'); assert.equal(h.requests[6].body.src, -1);
    await h.reply(6, {total: 1, n: 1, matches: [{src: 0, ci: 2, insts: 1, name: 'VIA'}]});
    assert.deepEqual(h.rows(), ['|VIA|']); assert.equal(h.el('cells-info').textContent, '1 match');
    h.el('cells-search').onkeydown({key: 'Escape', preventDefault() {}});
    assert.deepEqual(h.rows()[0], '▾|TOP|'); assert.equal(h.el('cells-search').value, '');
    // t raises the page and focuses the search field; Escape clears the highlight.
    assert.equal(h.c.key('t', {ctrlKey: false}), true); assert.equal(h.raised(), 1); assert.equal(focused, h.el('cells-search'));
    assert.equal(h.c.key('Escape', {}), true); assert.equal(h.el('cells-canvas').hidden, true);
    // Disconnect clears the tree; a missing hierarchy summary reports nohier.
    h.disconnect(); assert.deepEqual(h.rows(), []); assert.equal(h.el('cells-search').disabled, true);
    h.connect(); await h.fail(h.requests.length - 1, Object.assign(new Error('no hierarchy summary'), {code: 'nohier'}));
    assert.match(h.el('cells-info').textContent, /No cell index/); assert.equal(h.el('cells-build').hidden, false);
    console.log('WEB CELLS: ALL OK (lazy tree, search debounce, extent/instances per view, zoom, view root, keys, nohier)');
})().catch(e => { console.error(e); process.exitCode = 1; });
