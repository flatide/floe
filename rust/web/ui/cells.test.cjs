'use strict';
// Deterministic cell tree UI: server answers are scripted, no DOM library.
const assert = require('node:assert/strict');
const Cells = require('./cells.js');
const tick = () => new Promise(r => setImmediate(r));
// Focus follows the browser rules the tree relies on: only a connected,
// focusable node takes it, and removing the focused node leaves it on body.
class Element {
    constructor(tag, doc, id) { this.tag = tag || 'div'; this.doc = doc; this.id = id; this.parent = null; this.children = []; this.style = {}; this.dataset = {}; this.attributes = {}; this.hidden = false; this.disabled = false; this.value = ''; this.checked = false; this.className = ''; this.width = this.height = 1; }
    get isConnected() { return !!this.id || (!!this.parent && this.parent.isConnected); }
    appendChild(c) { c.parent = this; this.children.push(c); return c; }
    contains(n) { return this === n || this.children.some(c => c.contains(n)); }
    setAttribute(k, v) { this.attributes[k] = String(v); }
    getAttribute(k) { return this.attributes[k] === undefined ? null : this.attributes[k]; }
    querySelectorAll(tag) { return this.children.flatMap(c => [...(c.tag === tag ? [c] : []), ...c.querySelectorAll(tag)]); }
    getContext() { const ops = this.ops = this.ops || []; return {clearRect() { ops.push('clear'); }, strokeRect(x, y, w, h) { ops.push(['rect', x, y, w, h]); }}; }
    focus() { if (this.isConnected && (this.id || typeof this.tabIndex === 'number')) { this.doc.activeElement = focused = this; } } select() {}
    get textContent() { return this._text || ''; }
    set textContent(v) {
        this._text = v; if (this.children.some(c => c.contains(this.doc.activeElement))) { this.doc.activeElement = this.doc.body; }
        this.children.forEach(c => { c.parent = null; }); this.children = [];
    }
}
let focused = null;
const SOURCES = {sources: [{src: 0, placements: 1, name: 'valmini.oas'}]};
const TOP = {cell: 7, name: 'TOP', insts: 1, height: 3, unit: 0.001, bbox: [0, 0, 1000, 500], n: 2, total: 2,
    children: [{ci: 1, members: 4, leaf: false, name: 'BLK'}, {ci: 2, members: 1, leaf: true, name: 'VIA'}]};
const BLK = {cell: 1, name: 'BLK', insts: 4, height: 2, unit: 0.001, bbox: [0, 0, 100, 50], n: 1, total: 5, children: [{ci: 3, members: 2, leaf: true, name: 'M1'}]};
const NONE = {n: 0, more: false, visited: 0, boxes: []}, key = k => ({key: k, preventDefault() {}});
// opts.offered false and opts.noBuild model the public demo's port.
function harness(opts) {
    opts = opts || {};
    const nodes = new Map(), requests = [], edits = [], timers = [], builds = [];
    const doc = {createElement: tag => new Element(tag, doc)};
    doc.body = doc.activeElement = new Element('body', doc, 'body');
    const el = id => { if (!nodes.has(id)) { nodes.set(id, new Element(id === 'cells-canvas' ? 'canvas' : 'div', doc, id)); } return nodes.get(id); };
    el('cells-highlight').checked = true;
    let context = null, raised = 0, focusedCanvas = 0;
    const state = {status: 'idle', bbox_dbu: ['0', '0', '1000', '500'], pixels: [200, 100], dbu_um: '0.001', capabilities: {cell_root: true}, root_name: ''};
    const c = Cells.bind({
        el, document: doc,
        http(method, path, body) { return new Promise((resolve, reject) => { requests.push({method, path, body, resolve, reject}); }); },
        edit(body, done) { edits.push(body); if (done) { done(null); } },
        context: () => context, size: () => ({pixels: state.pixels}), unit: () => h.unit,
        focus: () => { focusedCanvas++; el('viewport').focus(); }, raise: () => { raised++; },
        rootAllowed: () => true, buildAllowed: () => h.allowBuild, buildOffered: opts.offered,
        build: opts.noBuild ? undefined : () => new Promise((resolve, reject) => { builds.push({resolve, reject}); }),
        setTimeout: (f, ms) => { timers.push({f, ms}); return timers.length; }, clearTimeout: n => { if (timers[n - 1]) { timers[n - 1].f = null; } }
    });
    const h = {c, el, doc, requests, edits, timers, builds, state, unit: 0.001, allowBuild: false, context: () => context,
        connect(id, st) { context = {id: id || 'v1', state: st || state, connected: true}; c.changed(); },
        disconnect() { context = null; c.changed(); },
        reply(i, value) { const r = requests[i]; r.done = true; r.resolve(value); return tick(); },
        fail(i, error) { const r = requests[i]; r.done = true; r.reject(error); return tick(); },
        rows() { return el('cells-tree').children.map(n => n.children.length ? n.children.map(x => x.textContent).join('|') : n.textContent); },
        raised: () => raised, focusedCanvas: () => focusedCanvas,
        // Scenario helpers: the last unanswered request of a kind, the row of
        // a cell by name, clicks that honour a disabled button.
        last(kind) { return requests.findLastIndex(r => !r.done && r.body.kind === kind); },
        pending(kind) { return requests.filter(r => !r.done && r.body.kind === kind); },
        kinds(from) { return requests.slice(from).map(r => r.body.kind); },
        row(name) { return el('cells-tree').children.find(n => n.children[1] && n.children[1].textContent === name); },
        click(id) { if (!el(id).disabled) { el(id).onclick(); } },
        async load() { h.connect(); await h.reply(h.last('sources'), SOURCES); await h.reply(h.last('children'), TOP); },
        async expand(name, answer) { const r = h.row(name); r.onclick({target: r.children[0]}); await h.reply(h.last('children'), answer); },
        async select(name, bbox) {
            const r = h.row(name); r.onclick({target: r.children[1]});
            await h.reply(h.last('bbox'), {insts: 1, approx: false, bbox});
            await h.reply(h.last('insts'), {n: 1, more: false, visited: 1, boxes: [bbox]});
        },
        type(text) { el('cells-search').value = text; el('cells-search').oninput(); timers.at(-1).f(); },
        async nohier() { h.connect(); await h.fail(h.last('sources'), Object.assign(new Error('no hierarchy summary'), {code: 'nohier'})); },
        shown(id) { return !el(id).hidden; }};
    return h;
}
// Review follow-ups, each on a fresh view.
const scenarios = [
    ['C3 frame at open and after a view switch', async () => {
        // The view's root is its frame from the first pass: the first
        // snapshot after the tree is no root change, so a double-click in
        // between asks once and frames.
        const h = harness(); h.state.root = {cell: 1, name: 'BLK'}; h.state.root_name = 'BLK';
        await h.load();
        let from = h.requests.length; h.row('VIA').ondblclick(); h.c.changed();
        assert.equal(h.kinds(from).filter(k => k === 'bbox').length, 1, 'an unchanged root asks the extent once');
        await h.reply(h.last('bbox'), {insts: 9, approx: false, bbox: [200, 200, 240, 220]});
        assert.deepEqual(h.edits, [{navigation: {kind: 'goto', center_um: ['0.22', '0.21'], width_um: '0.05'}}], 'and the double-click frames');
        // reset() forgets the old view's frame.
        h.connect('v2', Object.assign({}, h.state, {root: null, root_name: ''}));
        await h.reply(h.last('sources'), SOURCES); await h.reply(h.last('children'), TOP);
        from = h.requests.length; h.row('VIA').ondblclick(); h.c.changed();
        assert.equal(h.kinds(from).filter(k => k === 'bbox').length, 1, 'a view switch is no root change');
        await h.reply(h.last('bbox'), {insts: 9, approx: false, bbox: [200, 200, 240, 220]});
        assert.equal(h.edits.length, 2);
    }],
    ['C2 root change before the view can answer', async () => {
        // The old frame's extent goes at once and is asked again, once, when
        // the view is settled.
        const h = harness(); await h.load(); await h.expand('BLK', BLK); await h.select('M1', [10, 10, 30, 20]);
        assert.equal(h.el('cells-zoom').disabled, false);
        h.state.status = 'cancelling'; h.state.root = {cell: 1, name: 'BLK'}; h.state.root_name = 'BLK';
        const from = h.requests.length; h.c.changed();
        assert.equal(h.el('cells-zoom').disabled, true); assert.equal(h.el('cells-root').disabled, true);
        assert.equal(h.el('cells-info').textContent, 'M1: …', 'the old extent leaves the info line');
        assert(!h.kinds(from).includes('bbox'), 'not asked while cancelling');
        h.state.status = 'rendering'; h.c.changed(); h.state.status = 'idle'; h.c.changed();
        assert.deepEqual(h.requests.slice(from).filter(r => r.body.kind === 'bbox').map(r => r.body.cell), [3], 'asked once settled');
        await h.reply(h.last('bbox'), {insts: 2, approx: false, bbox: [0, 0, 20, 10]});
        assert.match(h.el('cells-info').textContent, /^M1: 2 instances · 0\.020000 × 0\.010000 um/);
        h.click('cells-zoom'); assert.deepEqual(h.edits.at(-1).navigation.center_um, ['0.01', '0.005']);
    }],
    ['C2 selection outside the tree', async () => {
        // A search result kept after Escape is in neither list: its extent
        // goes too, so nothing navigates or roots with the old frame's.
        const h = harness(); await h.load();
        h.type('M*'); await h.reply(h.last('find'), {total: 1, n: 1, matches: [{src: 0, ci: 3, insts: 1, name: 'M1'}]});
        await h.select('M1', [10, 10, 30, 20]);
        h.el('cells-search').onkeydown(key('Escape'));
        assert.equal(h.rows()[0], '▾|TOP|'); assert.equal(h.c.hasSelection(), true); assert.equal(h.el('cells-zoom').disabled, false);
        h.state.status = 'cancelling'; h.state.root = {cell: 1, name: 'BLK'}; h.state.root_name = 'BLK'; h.c.changed();
        assert.equal(h.el('cells-zoom').disabled, true); assert.equal(h.el('cells-root').disabled, true);
        h.click('cells-zoom'); h.click('cells-root'); h.c.setRoot(); h.c.key('t', {ctrlKey: true, shiftKey: false});
        assert.deepEqual(h.edits, [], 'no goto or root from the old frame');
        h.state.status = 'idle'; h.c.changed();
        await h.reply(h.last('bbox'), {insts: 1, approx: false, bbox: [0, 0, 20, 10]});
        h.click('cells-zoom'); assert.deepEqual(h.edits.at(-1).navigation.center_um, ['0.01', '0.005']);
    }],
    ['C2 root change while unavailable', async () => {
        const h = harness(); await h.load(); await h.select('VIA', [100, 100, 130, 130]);
        h.context().connected = false; h.state.root = {cell: 1, name: 'BLK'}; h.state.root_name = 'BLK';
        const from = h.requests.length; h.c.changed();
        assert.deepEqual(h.kinds(from), []); assert.equal(h.el('cells-zoom').disabled, true);
        h.context().connected = true; h.c.changed();
        assert.deepEqual(h.requests.slice(from).map(r => r.body.kind + '#' + r.body.cell), ['bbox#2', 'insts#2'], 'asked once available');
    }],
    ['C0 focus of a zoom that lands later', async () => {
        // The canvas takes the focus only from where the gesture left it;
        // the zoom button always hands it over.
        const h = harness(); await h.load(); await h.expand('BLK', BLK);
        const search = h.el('cells-search'), form = h.el('cells-search-form'), at = {insts: 9, approx: false, bbox: [200, 200, 240, 220]};
        h.row('VIA').focus(); h.row('VIA').onkeydown(key('Enter')); search.focus();
        await h.reply(h.last('bbox'), at);
        assert.equal(h.edits.length, 1, 'the zoom lands'); assert.equal(h.focusedCanvas(), 0); assert.equal(h.doc.activeElement, search, 'the field the user moved into keeps the focus');
        h.row('VIA').focus(); h.row('VIA').ondblclick(); await h.reply(h.last('bbox'), at);
        assert.equal(h.edits.length, 2); assert.equal(h.focusedCanvas(), 1, 'an undisturbed double-click hands it to the canvas');
        h.row('VIA').focus(); h.row('VIA').onkeydown(key('Enter')); h.row('BLK').focus(); h.row('BLK').onkeydown(key('ArrowLeft'));
        await h.reply(h.last('bbox'), at);
        assert.equal(h.edits.length, 3); assert.equal(h.focusedCanvas(), 2, 'focus still in the rebuilt tree counts');
        // Search Enter with the extent still out: only from the search box.
        h.row('VIA').onclick({target: h.row('VIA').children[1]}); search.focus(); form.onsubmit(key('Enter')); h.row('VIA').focus();
        await h.reply(h.last('bbox'), at);
        assert.equal(h.edits.length, 4); assert.equal(h.focusedCanvas(), 2); assert.equal(h.doc.activeElement, h.row('VIA'));
        h.row('VIA').onclick({target: h.row('VIA').children[1]}); search.focus(); form.onsubmit(key('Enter'));
        await h.reply(h.last('bbox'), at);
        assert.equal(h.edits.length, 5); assert.equal(h.focusedCanvas(), 3);
        h.click('cells-zoom'); assert.equal(h.focusedCanvas(), 4, 'the zoom button always does');
    }],
    ['R2/R4 keyboard in the tree', async () => {
        // One Tab stop (the selected row, else the first), arrows over cell
        // rows only, and a rebuilt row keeps the focus.
        const h = harness(); await h.load();
        const tabs = () => h.el('cells-tree').children.filter(n => typeof n.tabIndex === 'number').map(n => n.tabIndex), move = k => h.doc.activeElement.onkeydown(key(k));
        assert.deepEqual(tabs(), [0, -1, -1], 'the first row is the Tab stop');
        const blk = h.row('BLK'); blk.focus(); move('ArrowRight');
        assert.deepEqual(h.rows(), ['▾|TOP|', '▾|BLK|×4', '…', '|VIA|']);
        assert.notEqual(h.row('BLK'), blk); assert.equal(h.doc.activeElement, h.row('BLK'), 'the rebuilt row keeps the focus');
        await h.reply(h.last('children'), BLK); assert.equal(h.doc.activeElement, h.row('BLK'));
        move('ArrowDown'); assert.equal(h.doc.activeElement, h.row('M1'));
        move('ArrowDown'); assert.equal(h.doc.activeElement, h.row('VIA'), 'ArrowDown skips the "… 4 more" row');
        move('ArrowUp'); assert.equal(h.doc.activeElement, h.row('M1'), 'so does ArrowUp');
        const m1 = h.row('M1'); move('Enter');
        assert.equal(h.row('M1'), m1, 'selecting marks the row in place'); assert.equal(h.doc.activeElement, m1);
        assert.deepEqual(tabs(), [-1, -1, 0, -1], 'the selected row is the Tab stop');
        await h.reply(h.last('bbox'), {insts: 2, approx: false, bbox: [10, 10, 30, 20]});
        assert.equal(h.doc.activeElement, h.el('viewport'), 'the zoom hands the focus to the canvas');
        h.row('BLK').focus(); move('ArrowLeft');
        assert.deepEqual(h.rows(), ['▾|TOP|', '▸|BLK|×4', '|VIA|']); assert.equal(h.doc.activeElement, h.row('BLK'));
        assert.deepEqual(tabs(), [0, -1, -1], 'a hidden selection leaves the stop on the first row');
        const via = h.row('VIA'); via.focus(); via.onclick({target: via.children[1]});
        assert.equal(h.row('VIA'), via, 'a click keeps the row (and a double-click target)'); assert.equal(h.doc.activeElement, via);
    }],
    ['R3/R12 search next to the tree load and an expansion', async () => {
        const h = harness(), busy = () => h.el('cells-tree').getAttribute('aria-busy');
        h.connect(); assert.equal(busy(), 'true');
        h.type('VI*'); await h.reply(0, SOURCES);
        assert.equal(h.last('children'), 2, 'a search does not drop the tree load');
        await h.reply(2, TOP); assert.deepEqual(h.rows(), ['▾|TOP|', '▸|BLK|×4', '|VIA|']);
        await h.reply(1, {total: 1, n: 1, matches: [{src: 0, ci: 2, insts: 1, name: 'VIA'}]});
        assert.deepEqual(h.rows(), ['|VIA|']); assert.equal(h.el('cells-info').textContent, '1 match'); assert.equal(busy(), 'false');
        h.el('cells-search').onkeydown(key('Escape')); assert.deepEqual(h.rows(), ['▾|TOP|', '▸|BLK|×4', '|VIA|']);
        const blk = h.row('BLK'); blk.onclick({target: blk.children[0]}); h.type('M*');
        await h.reply(h.last('children'), BLK); await h.reply(h.last('find'), {total: 1, n: 1, matches: [{src: 0, ci: 3, insts: 2, name: 'M1'}]});
        assert.deepEqual(h.rows(), ['|M1|×2']);
        h.el('cells-search').onkeydown(key('Escape'));
        assert.deepEqual(h.rows(), ['▾|TOP|', '▾|BLK|×4', '|M1|×2', '… 4 more (find by name)', '|VIA|'], 'nor an expansion');
        assert.equal(busy(), 'false');
    }],
    ['R3/R12 suspend and resume', async () => {
        // What suspend drops is not stranded: resume loads the tree, and an
        // expansion folds back instead of keeping its '…'.
        const h = harness(); h.connect(); h.c.suspend(); await h.reply(0, SOURCES);
        h.c.resume(); assert.equal(h.last('sources'), 1, 'the tree is asked again');
        await h.reply(1, SOURCES); await h.reply(h.last('children'), TOP);
        const blk = h.row('BLK'); blk.onclick({target: blk.children[0]}); const out = h.last('children');
        h.c.suspend(); assert.deepEqual(h.rows(), ['▾|TOP|', '▸|BLK|×4', '|VIA|']); await h.reply(out, BLK);
        h.c.resume(); assert.deepEqual(h.rows(), ['▾|TOP|', '▸|BLK|×4', '|VIA|']); assert.equal(h.el('cells-tree').getAttribute('aria-busy'), 'false');
        h.row('BLK').onclick({target: h.row('BLK').children[0]}); assert.equal(h.last('children'), out + 1, 'expanding again asks again');
    }],
    ['R0/R6/R13 Enter with a search pending', async () => {
        // A search still waiting for its debounce (or one that failed) runs
        // now; only a searched box frames the selection.
        const h = harness(); await h.load(); await h.select('VIA', [100, 100, 130, 130]);
        const search = h.el('cells-search'), form = h.el('cells-search-form');
        search.focus(); search.value = 'BL*'; search.oninput();
        let from = h.requests.length; form.onsubmit(key('Enter'));
        assert.equal(h.timers.at(-1).f, null, 'Enter takes over the debounce');
        assert.deepEqual(h.requests.slice(from).map(r => r.body.kind + ':' + r.body.pattern), ['find:BL*']); assert.deepEqual(h.edits, []);
        await h.reply(h.last('find'), {total: 1, n: 1, matches: [{src: 0, ci: 1, insts: 4, name: 'BLK'}]});
        assert.deepEqual(h.rows(), ['|BLK|×4']);
        form.onsubmit(key('Enter'));
        assert.deepEqual(h.edits.at(-1).navigation.center_um, ['0.115', '0.115'], 'a searched box frames the selection'); assert.equal(h.focusedCanvas(), 1);
        h.row('BLK').onclick({target: h.row('BLK').children[1]}); search.focus(); search.value = 'VI*'; search.oninput();
        from = h.requests.length; form.onsubmit(key('Enter'));
        assert.deepEqual(h.kinds(from), ['find'], 'an extent still out does not take the Enter either');
        await h.fail(h.last('find'), new Error('busy'));
        from = h.requests.length; form.onsubmit(key('Enter'));
        assert.deepEqual(h.kinds(from), ['find'], 'a failed search is asked again');
    }],
    ['Enter while the sent search is out', async () => {
        // The debounce fired and the request is out: Enter must neither frame
        // the earlier selection nor send the search again; its answer shows.
        const h = harness(); await h.load(); await h.select('VIA', [100, 100, 130, 130]);
        const search = h.el('cells-search'), form = h.el('cells-search-form');
        search.focus(); search.value = 'BL*'; search.oninput(); h.timers.at(-1).f();
        assert.equal(h.pending('find').length, 1);
        let from = h.requests.length; form.onsubmit(key('Enter'));
        assert.deepEqual(h.kinds(from), [], 'no second search, no extent'); assert.deepEqual(h.edits, [], 'no move to the earlier selection');
        await h.reply(h.last('find'), {total: 1, n: 1, matches: [{src: 0, ci: 1, insts: 4, name: 'BLK'}]});
        assert.deepEqual(h.rows(), ['|BLK|×4']); assert.deepEqual(h.edits, []);
        // Typing on while a search is out: Enter searches the new text now.
        search.value = 'VI*'; search.oninput(); h.timers.at(-1).f(); search.value = 'VIA'; search.oninput();
        from = h.requests.length; form.onsubmit(key('Enter'));
        assert.deepEqual(h.requests.slice(from).map(r => r.body.pattern), ['VIA']); assert.deepEqual(h.edits, []);
        // A failed answer leaves the earlier results searched: Enter asks again.
        await h.fail(h.last('find'), new Error('busy'));
        from = h.requests.length; form.onsubmit(key('Enter'));
        assert.deepEqual(h.requests.slice(from).map(r => r.body.pattern), ['VIA'], 'a failed search is asked again'); assert.deepEqual(h.edits, []);
        await h.reply(h.last('find'), {total: 1, n: 1, matches: [{src: 0, ci: 2, insts: 1, name: 'VIA'}]});
        // Once its results are in, Enter frames the selection as before.
        form.onsubmit(key('Enter')); assert.equal(h.edits.at(-1).navigation.kind, 'goto');
    }],
    ['R14 the root takes the root button guard; no Ctrl+T', async () => {
        const h = harness(); await h.load();
        const via = h.row('VIA'); via.onclick({target: via.children[1]});
        h.c.setRoot(); assert.deepEqual(h.edits, [], 'not while the extent is asked');
        await h.reply(h.last('bbox'), {insts: 0, approx: false, bbox: null});
        assert.equal(h.el('cells-root').disabled, true); h.c.setRoot(); assert.deepEqual(h.edits, [], 'not for a cell not placed in this frame');
        await h.select('BLK', [0, 0, 100, 50]);
        // Ctrl+T / Ctrl+Shift+T are not shortcuts (browsers reserve them).
        assert.equal(h.c.key('t', {ctrlKey: true, shiftKey: false}), false); assert.equal(h.c.key('T', {ctrlKey: true, shiftKey: true}), false);
        let prevented = 0; h.row('BLK').onkeydown({key: 't', ctrlKey: true, preventDefault() { prevented++; }});
        assert.deepEqual(h.edits, []); assert.equal(prevented, 0);
        assert.equal(h.el('cells-root').disabled, false); h.click('cells-root');
        assert.deepEqual(h.edits, [{root: {src: 0, cell: 1}}]); assert.equal(h.c.rootName(), 'BLK');
        h.click('cells-top'); assert.deepEqual(h.edits.at(-1), {root: null});
    }],
    ['B1 build cell index: confirm, one build, the tree reloads', async () => {
        const h = harness(); h.allowBuild = true; await h.nohier();
        assert.equal(h.el('cells-info').textContent, 'No cell index (design.ovh) for this source.');
        assert(h.shown('cells-build')); assert.equal(h.el('cells-build').disabled, false); assert(!h.shown('cells-build-confirm'));
        // The button (or its menu proxy) only asks; Cancel builds nothing.
        h.click('cells-build');
        assert(h.shown('cells-build-confirm')); assert(!h.shown('cells-build')); assert.equal(h.doc.activeElement, h.el('cells-build-run'));
        assert.equal(h.builds.length, 0); assert.equal(h.raised(), 1, 'the cells page is raised for the note');
        h.click('cells-build-cancel'); assert(!h.shown('cells-build-confirm')); assert(h.shown('cells-build')); assert.equal(h.builds.length, 0);
        h.click('cells-build'); h.el('cells-build-confirm').onkeydown({key: 'Escape', preventDefault() {}});
        assert(!h.shown('cells-build-confirm')); assert.equal(h.builds.length, 0);
        h.click('cells-build'); h.click('cells-build-run'); h.click('cells-build-run');
        assert.equal(h.builds.length, 1, 'one build per approval');
        assert.equal(h.el('cells-info').textContent, 'Building the cell index (design.ovh)…');
        assert(!h.shown('cells-build-confirm')); assert(!h.shown('cells-build'));
        let from = h.requests.length; h.c.changed(); assert.deepEqual(h.kinds(from), [], 'no tree read while building');
        h.builds[0].resolve({phase: 'succeeded', built: 1}); await tick();
        assert.deepEqual(h.kinds(from), ['sources'], 'the tree is read again');
        await h.reply(h.last('sources'), SOURCES); await h.reply(h.last('children'), TOP);
        assert.deepEqual(h.rows(), ['▾|TOP|', '▸|BLK|×4', '|VIA|']); assert.equal(h.el('cells-info').textContent, 'TOP: 2 children');
        assert(!h.shown('cells-build')); assert(!h.shown('cells-build-confirm'));
        assert.equal(h.el('cells-zoom').disabled, true); assert.equal(h.el('cells-search').disabled, false);
    }],
    ['B2 a failed build keeps the note and the offer', async () => {
        const h = harness(); h.allowBuild = true; await h.nohier();
        h.click('cells-build'); h.click('cells-build-run');
        const from = h.requests.length; h.builds[0].reject(new Error('Resources or cache are busy.')); await tick();
        assert.equal(h.el('cells-info').textContent, 'Cell index not built: Resources or cache are busy.');
        assert.deepEqual(h.kinds(from), [], 'no tree read after a failure');
        assert(h.shown('cells-build')); assert.equal(h.el('cells-build').disabled, false); assert(!h.shown('cells-build-confirm'));
        // Not allowed (operation pending, sealed revision): nothing to confirm.
        h.allowBuild = false; h.c.changed(); assert.equal(h.el('cells-build').disabled, true);
        h.el('cells-build').onclick(); assert(!h.shown('cells-build-confirm'));
        h.allowBuild = true; h.c.changed(); h.click('cells-build'); h.allowBuild = false; h.c.changed();
        assert.equal(h.el('cells-build-run').disabled, true); h.el('cells-build-run').onclick(); assert.equal(h.builds.length, 1);
        // A build answered after the view changed does not touch the new tree.
        h.allowBuild = true; h.c.changed(); h.click('cells-build-run'); assert.equal(h.builds.length, 2);
        h.connect('v2'); const next = h.requests.length; h.builds[1].resolve({phase: 'succeeded'}); await tick();
        assert.deepEqual(h.kinds(next), []); assert.equal(h.pending('sources').length, 1, 'only the new view asks');
    }],
    ['B3 a deck source without its summary offers the build', async () => {
        const h = harness(); h.allowBuild = true; h.connect();
        await h.reply(h.last('sources'), {sources: [{src: 0, placements: 2, name: 'a.oas'}, {src: 1, placements: 1, name: 'b.oas'}]});
        assert.deepEqual(h.rows(), ['▸|a.oas|×2', '▸|b.oas|']); assert(!h.shown('cells-build'));
        const a = h.row('a.oas'); a.onclick({target: a.children[0]});
        await h.fail(h.last('children'), Object.assign(new Error('nohier'), {code: 'nohier'}));
        assert.equal(h.el('cells-info').textContent, 'No cell index (design.ovh) for this source.');
        assert(h.shown('cells-build')); assert.equal(h.el('cells-search').disabled, false);
        h.click('cells-build'); h.click('cells-build-run'); const from = h.requests.length;
        h.builds[0].resolve({phase: 'incomplete'}); await tick();
        assert.deepEqual(h.kinds(from), ['sources']); assert(!h.shown('cells-build'));
    }],
    ['B4 the demo shell never offers or confirms a build', async () => {
        const h = harness({offered: false, noBuild: true}); h.allowBuild = true; await h.nohier();
        assert.match(h.el('cells-info').textContent, /No cell index/);
        assert(!h.shown('cells-build')); assert(!h.shown('cells-build-confirm'));
        h.el('cells-build').onclick(); h.el('cells-build-run').onclick();
        assert(!h.shown('cells-build-confirm')); assert.equal(h.builds.length, 0);
        const offered = harness({offered: false}); offered.allowBuild = true; await offered.nohier();
        offered.el('cells-build').onclick(); offered.el('cells-build-run').onclick();
        assert(!offered.shown('cells-build-confirm')); assert.equal(offered.builds.length, 0, 'buildOffered:false wins over a port.build');
    }],
    ['B5 operation line: counts, never paths', async () => {
        const t = Cells.operationText, m = e => 'M:' + e;
        assert.equal(t({kind: 'cell_index', phase: 'queued'}, m), 'cell index · queued');
        assert.equal(t({kind: 'cell_index', phase: 'running', stage: 'building', current: 2, total: 3, built: 1, kept: 0, skipped: 0, failed: 0, error: null}, m), 'cell index · running · building 2/3');
        assert.equal(t({kind: 'cell_index', phase: 'succeeded', total: 1, built: 1, kept: 0, skipped: 0, failed: 0, error: null}, m), 'cell index · succeeded · 1 built, 0 kept');
        assert.equal(t({kind: 'cell_index', phase: 'incomplete', total: 3, built: 1, kept: 0, skipped: 1, failed: 1, error: 'worker_failed'}, m), 'cell index · incomplete · 1 built, 0 kept, 1 skipped, 1 failed · M:worker_failed');
        assert.equal(t({kind: 'cell_index', phase: 'failed', error: 'busy'}, m), 'cell index · failed · M:busy');
    }],
    ['R5 one highlight walk per view and selection', async () => {
        const h = harness(); await h.load(); await h.expand('BLK', BLK);
        const canvas = h.el('cells-canvas'), pick = name => h.row(name).onclick({target: h.row(name).children[1]});
        pick('M1'); await h.reply(h.last('bbox'), {insts: 2, approx: false, bbox: [10, 10, 30, 20]});
        h.c.changed(); h.state.status = 'rendering'; h.c.changed(); h.state.status = 'idle'; h.c.changed();
        assert.equal(h.pending('insts').length, 1, 'snapshots of the same view wait for the walk out');
        await h.reply(h.last('insts'), {n: 1, more: false, visited: 1, boxes: [[10, 10, 30, 20]]});
        assert.equal(canvas.hidden, false); assert.equal(h.el('cells-info').textContent, 'M1: 2 instances · 0.020000 × 0.010000 um · 1 in view');
        pick('VIA'); assert.equal(canvas.hidden, true, "the previous selection's boxes go at once");
        await h.reply(h.last('bbox'), {insts: 1, approx: false, bbox: [100, 100, 130, 130]});
        const walk = h.last('insts'); pick('M1');
        await h.reply(walk, {n: 1, more: false, visited: 1, boxes: [[100, 100, 130, 130]]});
        assert.equal(canvas.hidden, true, "a walk for the previous selection does not paint");
        await h.reply(h.last('bbox'), {insts: 2, approx: false, bbox: [10, 10, 30, 20]}); await h.reply(h.last('insts'), {n: 1, more: false, visited: 1, boxes: [[10, 10, 30, 20]]});
        // A root change asks the extent and the walk once each, in either answer order.
        let from = h.requests.length; h.state.root = {cell: 3, name: 'M1'}; h.state.root_name = 'M1'; h.c.changed();
        await h.reply(h.last('bbox'), {insts: 1, approx: false, bbox: [0, 0, 20, 10]}); await h.reply(h.last('insts'), {n: 1, more: false, visited: 1, boxes: [[0, 0, 20, 10]]});
        assert.deepEqual(h.kinds(from), ['bbox', 'insts']); assert.equal(h.el('cells-info').textContent, 'M1: 1 instance · 0.020000 × 0.010000 um · 1 in view');
        from = h.requests.length; h.state.root = null; h.state.root_name = ''; h.c.changed();
        await h.reply(h.last('insts'), {n: 1, more: false, visited: 1, boxes: [[10, 10, 30, 20]]}); await h.reply(h.last('bbox'), {insts: 2, approx: false, bbox: [10, 10, 30, 20]});
        assert.deepEqual(h.kinds(from), ['bbox', 'insts']); assert.equal(h.el('cells-info').textContent, 'M1: 2 instances · 0.020000 × 0.010000 um · 1 in view');
        assert.equal(canvas.hidden, false);
    }]
];
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
    // A root change is another coordinate frame: the placed extent and the
    // highlight are dropped and asked again in the new frame.
    const asked = h.requests.length;
    h.state.root = {cell: 3, name: 'M1'}; h.state.root_name = 'M1'; h.c.changed();
    assert.equal(h.el('cells-canvas').hidden, true); assert.equal(h.requests[asked].body.kind, 'bbox'); assert.equal(h.requests[asked].body.cell, 3);
    await h.reply(asked, {insts: 1, approx: false, bbox: [0, 0, 20, 10]});
    assert.equal(h.requests.at(-1).body.kind, 'insts', 'the extent reply re-asks the highlight'); await h.reply(h.requests.length - 1, {n: 1, more: false, visited: 1, boxes: [[0, 0, 20, 10]]});
    assert.equal(h.el('cells-canvas').hidden, false);
    h.el('cells-zoom').onclick();
    assert.deepEqual(h.edits.at(-1).navigation.center_um, ['0.01', '0.005'], 'zoom uses the extent of the new frame');
    // Every cached extent belongs to the old frame: reselecting another cell
    // asks again, and zoom stays off until that answer lands.
    const via = h.el('cells-tree').children.find(n => n.children[1] && n.children[1].textContent === 'VIA');
    via.onclick({target: via.children[1]});
    assert.equal(h.el('cells-zoom').disabled, true, 'no zoom before the extent of this frame');
    const editsBefore = h.edits.length; h.el('cells-zoom').onclick(); assert.equal(h.edits.length, editsBefore, 'zoom is a no-op without an extent');
    assert.equal(h.requests.at(-1).body.kind, 'bbox'); assert.equal(h.requests.at(-1).body.cell, 2);
    await h.reply(h.requests.length - 1, {insts: 1, approx: false, bbox: [100, 100, 130, 130]});
    assert.equal(h.el('cells-zoom').disabled, false); h.el('cells-zoom').onclick();
    assert.deepEqual(h.edits.at(-1).navigation.center_um, ['0.115', '0.115'], 'zoom uses the freshly asked extent');
    await h.reply(h.requests.length - 1, {n: 1, more: false, visited: 1, boxes: [[100, 100, 130, 130]]});
    m1.onclick({target: m1.children[1]}); await h.reply(h.requests.length - 1, {insts: 2, approx: false, bbox: [0, 0, 20, 10]});
    await h.reply(h.requests.length - 1, {n: 1, more: false, visited: 1, boxes: [[0, 0, 20, 10]]});
    h.state.root = null; h.state.root_name = ''; h.c.changed();
    const back = h.requests.findIndex((r, i) => i >= asked + 2 && r.body.kind === 'bbox' && r.body.cell === 3 && !r.done);
    assert(back > 0); await h.reply(back, {insts: 1, approx: false, bbox: [10, 10, 30, 20]});
    await h.reply(h.requests.length - 1, {n: 1, more: false, visited: 1, boxes: [[10, 10, 30, 20]]});
    // Placed extents are in the view's unit (a deck's DBU), never the source's.
    h.state.dbu_um = '0.01'; h.unit = 0.01;
    h.el('cells-zoom').onclick();
    assert.deepEqual(h.edits.at(-1).navigation, {kind: 'goto', center_um: ['0.2', '0.15'], width_um: '0.25'});
    h.state.dbu_um = '0.001'; h.unit = 0.001;
    // Double-click and Enter frame the cell after its extent lands; a root
    // change in between cancels the pending zoom.
    const via2 = h.el('cells-tree').children.find(n => n.children[1] && n.children[1].textContent === 'VIA');
    let edits0 = h.edits.length; via2.ondblclick();
    assert.equal(h.requests.at(-1).body.kind, 'bbox'); assert.equal(h.requests.at(-1).body.cell, 2, 'VIA extent asked first');
    assert.equal(h.edits.length, edits0, 'no navigation before the extent');
    await h.reply(h.requests.length - 1, {insts: 9, approx: false, bbox: [200, 200, 240, 220]});
    assert.equal(h.edits.at(-1).navigation.kind, 'goto'); assert.deepEqual(h.edits.at(-1).navigation.center_um, ['0.22', '0.21'], 'double-click frames after the answer');
    await h.reply(h.requests.length - 1, {n: 0, more: false, visited: 0, boxes: []});
    edits0 = h.edits.length; via2.onkeydown({key: 'Enter', preventDefault() {}});
    assert.equal(h.requests.at(-1).body.kind, 'bbox'); assert.equal(h.edits.length, edits0);
    await h.reply(h.requests.length - 1, {insts: 9, approx: false, bbox: [200, 200, 240, 220]});
    assert.equal(h.edits.length, edits0 + 1, 'Enter frames after the answer');
    await h.reply(h.requests.length - 1, {n: 0, more: false, visited: 0, boxes: []});
    edits0 = h.edits.length; via2.ondblclick(); const staleZoom = h.requests.length - 1;
    h.state.root = {cell: 1, name: 'BLK'}; h.state.root_name = 'BLK'; h.c.changed();
    await h.reply(staleZoom, {insts: 9, approx: false, bbox: [200, 200, 240, 220]});
    await h.reply(h.requests.length - 1, {insts: 9, approx: false, bbox: [0, 0, 40, 20]});
    assert.equal(h.edits.length, edits0, 'a root change cancels the pending double-click zoom');
    await h.reply(h.requests.length - 1, {n: 0, more: false, visited: 0, boxes: []});
    h.state.root = null; h.state.root_name = ''; h.c.changed();
    await h.reply(h.requests.length - 1, {insts: 9, approx: false, bbox: [200, 200, 240, 220]});
    await h.reply(h.requests.length - 1, {n: 0, more: false, visited: 0, boxes: []});
    // Enter in the search field frames the selection, asking for its extent when unknown.
    via2.onclick({target: via2.children[1]}); edits0 = h.edits.length;
    h.el('cells-search-form').onsubmit({preventDefault() {}});
    assert.equal(h.requests.at(-1).body.kind, 'bbox'); assert.equal(h.edits.length, edits0);
    await h.reply(h.requests.length - 1, {insts: 9, approx: false, bbox: [200, 200, 240, 220]});
    assert.equal(h.edits.length, edits0 + 1, 'search Enter frames the selected cell');
    await h.reply(h.requests.length - 1, {n: 0, more: false, visited: 0, boxes: []});
    // Search replaces the tree after the debounce and Escape restores it.
    h.el('cells-search').value = 'VI*'; h.el('cells-search').oninput();
    assert.equal(h.timers.at(-1).ms, 150); h.timers.at(-1).f();
    const find = h.requests.at(-1);
    assert.equal(find.body.kind, 'find'); assert.equal(find.body.pattern, 'VI*'); assert.equal(find.body.src, -1);
    await h.reply(h.requests.length - 1, {total: 1, n: 1, matches: [{src: 0, ci: 2, insts: 1, name: 'VIA'}]});
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
    for (const [, run] of scenarios) { await run(); }
    console.log('WEB CELLS: ALL OK (lazy tree, search debounce, extent/instances per view, zoom, view root, keys, nohier; root frames, zoom focus, tree keys, flights, search Enter, root guard, walks; cell index confirm/build/reload, failure, deck source, demo)');
})().catch(e => { console.error(e); process.exitCode = 1; });
