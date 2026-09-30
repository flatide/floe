/* Calibre-style cell tree over the owner cell queries. Rust owns the
 * hierarchy summary (design.ovh), name search, extents and instance walks;
 * this module keeps only the expanded rows, the selection and the highlight
 * boxes it was last given for the displayed frame. */
(function (root) {
    'use strict';
    // Children answers may carry 20 000 rows (renderd cap) - well over the
    // 1 MiB default reply limit of the owner HTTP helper.
    const FIND_LIMIT = 2000, INSTS_CAP = 4096, SEARCH_DELAY = 150, FRAME_FRACTION = 0.8, REPLY_LIMIT = 8 * 1024 * 1024;
    function bind(port) {
        const el = port.el, doc = port.document, tree = el('cells-tree'), search = el('cells-search'), canvas = el('cells-canvas');
        let identity = '', roots = [], results = null, selected = null, shown = [], timer = null, stopped = false, suspended = false;
        // The tree load, each expansion and the search are separate flights:
        // a search never drops a tree answer. `epoch` (reset, suspend) drops
        // them all; `busy` counts the tree questions still out (aria-busy).
        // `searched` is the text whose results are shown; `seeking` the text
        // of the search still out (null when none).
        let epoch = 0, busy = 0, findFlight = 0, searched = '', seeking = null;
        // Selection info (bbox) and the per-view highlight (insts) are
        // separate flights: a view change must not drop a pending extent.
        let highlight = null, highlightKey = '', highlightAsked = '', highlightFlight = 0, bboxFlight = 0, info = '', rootName = '', hier = true;
        let rootsFlight = false, rootsFailedAt = 0, rootKey = '', reframe = false;
        function context() { return stopped || suspended ? null : port.context(); }
        function available() { const c = context(); return !!c && c.connected && !!c.id; }
        function note(text) { info = text || ''; el('cells-info').textContent = info; }
        // Answers still out belong to another view or to a suspended panel:
        // drop them and forget that they are pending.
        function abandon() { ++epoch; ++bboxFlight; busy = 0; rootsFlight = false; searched = ''; seeking = null; if (timer) { port.clearTimeout(timer); timer = null; } }
        function dropHighlight() { ++highlightFlight; highlight = null; highlightKey = ''; highlightAsked = ''; }
        function reset() {
            roots = []; results = null; selected = null; shown = []; rootName = ''; hier = true; rootKey = ''; reframe = false;
            tree.textContent = ''; search.value = ''; note(''); abandon(); dropHighlight(); paintHighlight(); update();
        }
        async function query(body) {
            const c = context(); if (!c) { throw new Error('No open view.'); }
            body.view_id = c.id;
            return port.http('POST', '/api/v1/views/' + c.id + '/cells', body, false, undefined, {limit: REPLY_LIMIT});
        }
        // Tree questions (sources, children, find) keep the tree aria-busy.
        async function ask(body) {
            const ep = epoch; ++busy; update();
            try { return await query(body); } finally { if (ep === epoch) { --busy; update(); } }
        }
        function label(cell) { return cell.name + (cell.members > 1 ? '  ×' + cell.members : ''); }
        function row(cell, depth, container) {
            const node = doc.createElement('div'); node.className = 'cell-row'; node.setAttribute('role', 'treeitem');
            node.style.paddingLeft = (8 + depth * 14) + 'px';
            const twist = doc.createElement('span'); twist.className = 'cell-twist';
            twist.textContent = cell.leaf ? '' : (cell.open ? '▾' : '▸');
            const name = doc.createElement('span'); name.className = 'cell-name'; name.textContent = cell.name; name.title = cell.name;
            const count = doc.createElement('span'); count.className = 'cell-count'; count.textContent = cell.members > 1 ? '×' + cell.members : '';
            node.appendChild(twist); node.appendChild(name); node.appendChild(count);
            node.setAttribute('aria-expanded', cell.leaf ? 'false' : String(!!cell.open));
            node.tabIndex = -1;
            node.onclick = function (e) { if (!cell.leaf && e.target === twist) { expand(cell); } else { choose(cell); } };
            node.ondblclick = function () { chooseAndZoom(cell, gesture()); };
            node.onkeydown = function (e) {
                if (e.isComposing || e.keyCode === 229) { return; }
                if (e.key === 'ArrowRight' && !cell.leaf && !cell.open) { e.preventDefault(); expand(cell); }
                else if (e.key === 'ArrowLeft' && cell.open) { e.preventDefault(); cell.open = false; paint(); }
                else if (e.key === 'Enter') { e.preventDefault(); chooseAndZoom(cell, gesture()); }
                // The root chords work where a click leaves the focus, too.
                else if ((e.key === 't' || e.key === 'T') && e.ctrlKey && !e.metaKey && !e.altKey && !e.repeat) { if (key(e.key, e)) { e.preventDefault(); } }
                // Only cell rows are stops: '…' and '… N more' are skipped.
                else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); const next = shown[shown.indexOf(cell) + (e.key === 'ArrowDown' ? 1 : -1)]; if (next) { next.node.focus(); } }
            };
            cell.node = node; shown.push(cell); container.appendChild(node);
            if (cell.open && cell.children) {
                cell.children.forEach(function (ch) { row(ch, depth + 1, container); });
                if (cell.more) { const m = doc.createElement('div'); m.className = 'cell-row cell-more'; m.style.paddingLeft = (8 + (depth + 1) * 14) + 'px'; m.textContent = '… ' + cell.more + ' more (find by name)'; container.appendChild(m); }
            } else if (cell.open && cell.loading) {
                const m = doc.createElement('div'); m.className = 'cell-row cell-more'; m.style.paddingLeft = (8 + (depth + 1) * 14) + 'px'; m.textContent = '…'; container.appendChild(m);
            }
        }
        function paint() {
            // Rebuilt rows are new nodes: the focused row's cell keeps the
            // focus (or the Tab stop takes it when that cell went away).
            const active = doc.activeElement, had = shown.find(function (cell) { return cell.node === active; });
            tree.textContent = ''; shown = [];
            if (results) {
                results.forEach(function (cell) { row(cell, 0, tree); });
                if (!results.length) { const m = doc.createElement('div'); m.className = 'cell-row cell-more'; m.textContent = 'no matching cell'; tree.appendChild(m); }
            } else { roots.forEach(function (cell) { row(cell, 0, tree); }); }
            const home = mark();
            if (had) { const next = shown.indexOf(had) >= 0 ? had : home; if (next) { next.node.focus(); } }
        }
        // Selection and the one Tab stop (the selected row, else the first)
        // change in place: a rebuild would drop the focused row and the
        // target of a double-click.
        function mark() {
            const home = shown.indexOf(selected) >= 0 ? selected : shown[0];
            shown.forEach(function (cell) {
                cell.node.className = 'cell-row' + (cell === selected ? ' selected' : '');
                cell.node.setAttribute('aria-selected', String(cell === selected));
                cell.node.tabIndex = cell === home ? 0 : -1;
            });
            update();
            return home;
        }
        function update() {
            const ok = available() && hier;
            // Zoom and root need the extent of the current frame: a reselected
            // or re-rooted cell waits for its bbox answer.
            el('cells-zoom').disabled = !ok || !selected || !selected.bbox;
            el('cells-root').disabled = !rootable();
            el('cells-top').disabled = !available() || !rootName;
            search.disabled = !available();
            el('cells-highlight').disabled = !available();
            // A shell without an index path (the public demo) never offers one.
            el('cells-build').hidden = !available() || hier || port.buildOffered === false;
            el('cells-build').disabled = !available() || !port.buildAllowed();
            tree.setAttribute('aria-busy', String(busy > 0));
        }
        function rootable() { return available() && hier && !!selected && !!selected.bbox && port.rootAllowed() && !!selected.hasShapes; }
        function cellFrom(src, ci, name, members, leaf) { return {src: src, ci: ci, name: name, members: members, leaf: leaf, open: false, children: null, loading: false, more: 0}; }
        function settled(c) { return !!(c && c.state && ['idle', 'rendering'].includes(c.state.status)); }
        async function loadRoots() {
            const ep = epoch, c = context(); if (!c) { return; }
            rootsFlight = true;
            try {
                const r = await ask({kind: 'sources'});
                if (ep !== epoch || !available()) { return; }
                hier = true;
                // A search answered first keeps its line.
                if (r.sources.length === 1) {
                    const top = await ask({kind: 'children', src: r.sources[0].src});
                    if (ep !== epoch || !available()) { return; }
                    const cell = cellFrom(r.sources[0].src, top.cell, top.name, 1, top.children.length === 0);
                    cell.hasShapes = true; cell.open = true; fill(cell, top);
                    roots = [cell];
                    if (!results) { note(top.name + ': ' + top.total + ' children'); }
                } else {
                    roots = r.sources.map(function (s) { const cell = cellFrom(s.src, null, s.name || ('source ' + s.src), s.placements, false); cell.source = true; return cell; });
                    if (!results) { note(r.sources.length + ' jobdeck sources'); }
                }
                paint();
            } catch (e) {
                if (ep !== epoch) { return; }
                if (e && /nohier/.test(String(e.code || e.message))) { hier = false; note('No cell index (design.ovh) for this source.'); }
                else { rootsFailedAt = port.now ? port.now() : Date.now(); note(String(e.message || e)); }
                update();
            } finally { if (ep === epoch) { rootsFlight = false; } }
        }
        function fill(cell, answer) {
            cell.ci = answer.cell; cell.children = answer.children.map(function (ch) { return cellFrom(cell.src, ch.ci, ch.name, ch.members, ch.leaf); });
            cell.more = Math.max(0, answer.total - answer.n); cell.loading = false; cell.leaf = cell.children.length === 0 && !cell.more;
            cell.localBbox = answer.bbox; cell.insts = answer.insts; cell.height = answer.height; cell.unit = answer.unit;
        }
        async function expand(cell) {
            if (cell.open) { cell.open = false; paint(); return; }
            cell.open = true;
            // An answer still out fills the row when it lands.
            if (cell.children || cell.loading) { paint(); return; }
            cell.loading = true; paint();
            const ep = epoch;
            try {
                const body = {kind: 'children', src: cell.src}; if (cell.ci !== null && cell.ci !== undefined) { body.cell = cell.ci; }
                const r = await ask(body);
                if (ep !== epoch) { return; }
                fill(cell, r); paint();
            } catch (e) { if (ep === epoch) { cell.loading = false; cell.open = false; note(String(e.message || e)); paint(); } }
        }
        function forget(cell) {
            cell.bbox = null; cell.hasShapes = undefined;
            if (cell.children) { cell.children.forEach(forget); }
        }
        // Abandoned expansions fold back; expanding again asks again.
        function fold(cell) {
            if (cell.loading) { cell.loading = false; cell.open = false; }
            if (cell.children) { cell.children.forEach(fold); }
        }
        // Resolves true once this cell's extent in the current frame landed
        // and the cell is still the selection; false when superseded/failed.
        async function choose(cell) {
            // The previous selection's boxes (and its pending walk) go now.
            if (selected !== cell) { dropHighlight(); paintHighlight(); }
            // Extents are placed in the current frame: never trust a cached
            // one across selections, ask again and keep zoom/root off meanwhile.
            cell.bbox = null; cell.hasShapes = undefined;
            selected = cell; reframe = false; mark();
            if (cell.source || cell.ci === null || cell.ci === undefined) { note(cell.name + ': ' + cell.members + ' placements'); return false; }
            const token = ++bboxFlight;
            try {
                const r = await query({kind: 'bbox', src: cell.src, cell: cell.ci});
                if (token !== bboxFlight || selected !== cell) { return false; }
                cell.bbox = r.bbox; cell.hasShapes = !!r.bbox;
                if (r.bbox) {
                    const u = Number(port.unit()), w = (r.bbox[2] - r.bbox[0]) * u, h = (r.bbox[3] - r.bbox[1]) * u;
                    note(cell.name + ': ' + r.insts + ' instance' + (r.insts === 1 ? '' : 's') + ' · ' + w.toPrecision(5) + ' × ' + h.toPrecision(5) + ' um' + (r.approx ? ' (approx)' : ''));
                } else { note(cell.name + ': not placed under the top cell'); }
                update(); inView(); refreshHighlight(false);
                return !!r.bbox;
            } catch (e) { if (token === bboxFlight) { note(String(e.message || e)); } return false; }
        }
        // Double-click / Enter: frame the cell once its extent in THIS frame
        // is known; a root change or another selection in between cancels.
        function chooseAndZoom(cell, still) {
            const frame = rootKey;
            return choose(cell).then(function (ok) { if (ok && selected === cell && rootKey === frame && cell.bbox) { zoom(still()); } });
        }
        // Whether a zoom answered later may take the focus to the canvas: only
        // while the focus is where the gesture left it - on that row, or for a
        // row still in the tree (rebuilt) or on body - never out of a field.
        function gesture() {
            const from = doc.activeElement, row = !!from && from !== tree && tree.contains(from);
            return function () { const now = doc.activeElement; return now === from || (row && (!now || now === doc.body || tree.contains(now))); };
        }
        async function refreshHighlight(force) {
            const c = context(); const on = el('cells-highlight').checked;
            if (!c || !selected || !on || selected.ci === null || selected.ci === undefined || !c.state) { if (highlight || highlightAsked) { dropHighlight(); paintHighlight(); } return; }
            const cell = selected, key = c.id + ':' + c.state.bbox_dbu.join(',') + ':' + c.state.pixels.join('x') + ':' + cell.src + '/' + cell.ci;
            // One walk per view and selection, the pending one included.
            if (key === highlightAsked || (!force && key === highlightKey)) { return; }
            const token = ++highlightFlight; highlightAsked = key;
            try {
                const r = await query({kind: 'insts', src: cell.src, cell: cell.ci, view: c.state.bbox_dbu.map(Number), cap: INSTS_CAP});
                if (token !== highlightFlight) { return; }
                highlightAsked = ''; highlightKey = key;
                highlight = {cell: cell, n: r.n, boxes: r.boxes, more: r.more, bbox: c.state.bbox_dbu.map(Number), pixels: c.state.pixels.slice()};
                inView(); paintHighlight();
            } catch (e) { if (token === highlightFlight) { dropHighlight(); paintHighlight(); } }
        }
        // The info line counts the drawn instances once the extent line is in.
        function inView() {
            if (highlight && highlight.cell === selected && selected.bbox) { el('cells-info').textContent = info + ' · ' + highlight.n + ' in view' + (highlight.more ? ' (more - zoom in)' : ''); }
        }
        function paintHighlight() {
            const c = context(), size = port.size();
            if (!canvas || !size) { return; }
            if (!highlight || !c || !c.state || !el('cells-highlight').checked) { canvas.hidden = true; return; }
            // Device pixels like the other overlays: the bitmap is DPR-sized
            // and the element is laid out at CSS size over the viewport.
            const w = size.pixels[0], h = size.pixels[1], dpr = size.dpr || 1;
            if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
            canvas.style.width = (w / dpr) + 'px'; canvas.style.height = (h / dpr) + 'px';
            canvas.style.left = (size.left || 0) + 'px'; canvas.style.top = (size.top || 0) + 'px';
            const ctx = canvas.getContext('2d'); ctx.clearRect(0, 0, w, h);
            const b = highlight.bbox, sx = w / (b[2] - b[0]), sy = h / (b[3] - b[1]), min = 7 * dpr;
            ctx.strokeStyle = '#40e0ff'; ctx.lineWidth = 2 * dpr;
            highlight.boxes.forEach(function (box) {
                let x0 = (box[0] - b[0]) * sx, y0 = (b[3] - box[3]) * sy, x1 = (box[2] - b[0]) * sx, y1 = (b[3] - box[1]) * sy;
                if (x1 - x0 < min) { const m = (x0 + x1) / 2; x0 = m - min / 2; x1 = m + min / 2; }
                if (y1 - y0 < min) { const m = (y0 + y1) / 2; y0 = m - min / 2; y1 = m + min / 2; }
                ctx.strokeRect(Math.round(x0) + 0.5, Math.round(y0) + 0.5, Math.max(1, Math.round(x1 - x0)), Math.max(1, Math.round(y1 - y0)));
            });
            canvas.hidden = false;
        }
        function zoom(focus) {
            if (!selected || !selected.bbox || !available()) { return; }
            const u = Number(port.unit()), b = selected.bbox;
            const cx = (b[0] + b[2]) / 2 * u, cy = (b[1] + b[3]) / 2 * u, size = port.size();
            const aspect = size ? size.pixels[0] / size.pixels[1] : 1;
            const w = Math.max((b[2] - b[0]) * u, (b[3] - b[1]) * u * aspect) / FRAME_FRACTION;
            // Bounded decimals: the goto wire takes decimal strings, not floats.
            const fmt = function (n) { return String(Number(n.toPrecision(12))); };
            port.edit({navigation: {kind: 'goto', center_um: [fmt(cx), fmt(cy)], width_um: fmt(w > 0 ? w : 1)}});
            if (focus) { port.focus(); }
        }
        function setRoot() {
            // Button, menu and Ctrl+T alike: only with this frame's extent.
            if (!rootable() || selected.ci === null || selected.ci === undefined) { return; }
            const cell = selected;
            port.edit({root: {src: cell.src, cell: cell.ci}}, function (error) { if (error) { note(String(error)); } else { rootName = cell.name; update(); } });
        }
        function clearRoot() { if (!available() || !rootName) { return; } port.edit({root: null}, function (error) { if (!error) { rootName = ''; update(); } }); }
        async function find() {
            const text = search.value.trim(), token = ++findFlight, ep = epoch;
            if (!text) { searched = text; seeking = null; results = null; paint(); return; }
            // The shown results stay `searched` until this answer replaces them.
            seeking = text;
            try {
                const r = await ask({kind: 'find', src: -1, pattern: text, limit: FIND_LIMIT});
                if (token !== findFlight || ep !== epoch || search.value.trim() !== text) { return; }
                results = r.matches.map(function (m) { const cell = cellFrom(m.src, m.ci, m.name, m.insts, true); cell.hasShapes = true; return cell; });
                searched = text;
                note(r.total + ' match' + (r.total === 1 ? '' : 'es') + (r.total > r.n ? ' (showing ' + r.n + ')' : ''));
                paint();
            } catch (e) { if (token === findFlight && ep === epoch) { note(String(e.message || e)); } }
            finally { if (token === findFlight) { seeking = null; } }
        }
        search.oninput = function () { if (timer) { port.clearTimeout(timer); } timer = port.setTimeout(function () { timer = null; find(); }, SEARCH_DELAY); };
        // Enter runs a search still waiting for its debounce (or one that did
        // not answer); while the search for this text is out it waits for
        // those results instead of framing the earlier selection. Otherwise
        // it frames the selection; the canvas takes the focus only from the
        // search box.
        el('cells-search-form').onsubmit = function (e) {
            e.preventDefault();
            const text = search.value.trim(), here = function () { return doc.activeElement === search; };
            const typed = !!timer || text !== searched;
            if (timer) { port.clearTimeout(timer); timer = null; }
            if (seeking !== null && seeking === text) { return; }
            if (typed || !selected || selected.source) { find(); } else if (selected.bbox) { zoom(here()); } else { chooseAndZoom(selected, here); }
        };
        search.onkeydown = function (e) {
            if (e.key !== 'Escape' || e.isComposing || e.keyCode === 229) { return; }
            e.preventDefault(); if (timer) { port.clearTimeout(timer); timer = null; }
            ++findFlight; search.value = ''; searched = ''; seeking = null; results = null; paint(); port.focus();
        };
        el('cells-zoom').onclick = function () { zoom(true); };
        el('cells-root').onclick = setRoot;
        el('cells-top').onclick = clearRoot;
        el('cells-highlight').onchange = function () { refreshHighlight(true); };
        el('cells-build').onclick = function () { if (port.build) { port.build(); } };
        function frameOf(c) { return c && c.state && c.state.root ? String(c.state.root.cell) : ''; }
        function changed() {
            const c = context();
            const id = c && c.id ? c.id : '';
            // A new view starts in its own frame: its root is no change.
            if (id !== identity) { identity = id; reset(); rootsFailedAt = 0; rootKey = frameOf(c); }
            if (c && c.state && c.state.root_name !== undefined) { const name = c.state.root_name || ''; if (name !== rootName) { rootName = name; } }
            // A root change moves the coordinate frame: placed extents and
            // highlights of the old frame are dropped at once, the selection's
            // own too (in the tree or not); it is asked again once the view
            // can answer.
            const key = frameOf(c);
            if (key !== rootKey) {
                rootKey = key; ++bboxFlight; dropHighlight(); paintHighlight();
                roots.forEach(forget); if (results) { results.forEach(forget); }
                if (selected && !selected.source) { forget(selected); reframe = true; note(selected.name + ': …'); }
            }
            // The tree waits for a settled view: while the worker is still
            // opening, a query answers busy. One retry per later settled state.
            if (id && hier && !roots.length && !rootsFlight && settled(c) && available()) {
                const now = port.now ? port.now() : Date.now();
                if (!rootsFailedAt || now - rootsFailedAt > 2000) { loadRoots(); }
            }
            if (reframe && selected && settled(c) && available()) { choose(selected); }
            update();
            if (available()) { refreshHighlight(false); } else { paintHighlight(); }
        }
        function unselect() { selected = null; dropHighlight(); mark(); paintHighlight(); }
        function key(k, e) {
            if (!available()) { return false; }
            if (k === 't' && !e.ctrlKey && !e.metaKey) { port.raise(); search.focus(); search.select(); return true; }
            if ((k === 't' || k === 'T') && e.ctrlKey && !e.shiftKey) { setRoot(); return true; }
            if ((k === 'T' || k === 't') && e.ctrlKey && e.shiftKey) { clearRoot(); return true; }
            if (k === 'Escape' && highlight) { unselect(); return true; }
            return false;
        }
        function suspend() {
            suspended = true; abandon(); dropHighlight(); roots.forEach(fold);
            // An extent still out is asked again on resume.
            if (selected && !selected.source && selected.hasShapes === undefined) { reframe = true; }
            paint(); paintHighlight();
        }
        function resume() { suspended = false; changed(); }
        function stop() { stopped = true; suspend(); }
        update();
        return Object.freeze({changed: changed, key: key, paint: paintHighlight, zoom: function () { zoom(true); }, setRoot: setRoot, clearRoot: clearRoot,
            clearHighlight: unselect, hasSelection: function () { return !!selected; }, rootName: function () { return rootName; },
            suspend: suspend, resume: resume, stop: stop});
    }
    const api = {bind: bind};
    if (typeof module !== 'undefined' && module.exports) { module.exports = api; } else { root.FloeCells = api; }
}(typeof window === 'undefined' ? this : window));
