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
        let identity = '', roots = [], results = null, selected = null, flight = 0, timer = null, stopped = false, suspended = false;
        // Selection info (bbox) and the per-view highlight (insts) are
        // separate flights: a view change must not drop a pending extent.
        let highlight = null, highlightKey = '', highlightFlight = 0, bboxFlight = 0, info = '', rootName = '', hier = true;
        let rootsFlight = false, rootsFailedAt = 0, rootKey = '';
        function context() { return stopped || suspended ? null : port.context(); }
        function available() { const c = context(); return !!c && c.connected && !!c.id; }
        function note(text) { info = text || ''; el('cells-info').textContent = info; }
        function reset() {
            roots = []; results = null; selected = null; highlight = null; highlightKey = ''; rootName = ''; hier = true;
            tree.textContent = ''; search.value = ''; note(''); ++flight; ++highlightFlight; ++bboxFlight; paintHighlight(); update();
        }
        async function query(body) {
            const c = context(); if (!c) { throw new Error('No open view.'); }
            body.view_id = c.id;
            return port.http('POST', '/api/v1/views/' + c.id + '/cells', body, false, undefined, {limit: REPLY_LIMIT});
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
            node.setAttribute('aria-selected', String(selected === cell));
            if (selected === cell) { node.className += ' selected'; }
            node.tabIndex = -1;
            node.onclick = function (e) { if (!cell.leaf && e.target === twist) { expand(cell); } else { choose(cell); } };
            node.ondblclick = function () { choose(cell); zoom(); };
            node.onkeydown = function (e) {
                if (e.isComposing || e.keyCode === 229) { return; }
                if (e.key === 'ArrowRight' && !cell.leaf && !cell.open) { e.preventDefault(); expand(cell); }
                else if (e.key === 'ArrowLeft' && cell.open) { e.preventDefault(); cell.open = false; paint(); }
                else if (e.key === 'Enter') { e.preventDefault(); choose(cell); zoom(); }
                else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); const all = tree.querySelectorAll('div'); const i = Array.prototype.indexOf.call(all, node); const next = all[i + (e.key === 'ArrowDown' ? 1 : -1)]; if (next) { next.focus(); } }
            };
            cell.node = node; container.appendChild(node);
            if (cell.open && cell.children) {
                cell.children.forEach(function (ch) { row(ch, depth + 1, container); });
                if (cell.more) { const m = doc.createElement('div'); m.className = 'cell-row cell-more'; m.style.paddingLeft = (8 + (depth + 1) * 14) + 'px'; m.textContent = '… ' + cell.more + ' more (find by name)'; container.appendChild(m); }
            } else if (cell.open && cell.loading) {
                const m = doc.createElement('div'); m.className = 'cell-row cell-more'; m.style.paddingLeft = (8 + (depth + 1) * 14) + 'px'; m.textContent = '…'; container.appendChild(m);
            }
        }
        function paint() {
            tree.textContent = '';
            if (results) {
                results.forEach(function (cell) { row(cell, 0, tree); });
                if (!results.length) { const m = doc.createElement('div'); m.className = 'cell-row cell-more'; m.textContent = 'no matching cell'; tree.appendChild(m); }
            } else { roots.forEach(function (cell) { row(cell, 0, tree); }); }
            update();
        }
        function update() {
            const ok = available() && hier;
            // Zoom and root need the extent of the current frame: a reselected
            // or re-rooted cell waits for its bbox answer.
            el('cells-zoom').disabled = !ok || !selected || !selected.bbox;
            el('cells-root').disabled = !ok || !selected || !selected.bbox || !port.rootAllowed() || !selected.hasShapes;
            el('cells-top').disabled = !available() || !rootName;
            search.disabled = !available();
            el('cells-highlight').disabled = !available();
            el('cells-build').hidden = !available() || hier;
            el('cells-build').disabled = !available() || !port.buildAllowed();
            tree.setAttribute('aria-busy', String(flight > 0));
        }
        function cellFrom(src, ci, name, members, leaf) { return {src: src, ci: ci, name: name, members: members, leaf: leaf, open: false, children: null, loading: false, more: 0}; }
        function settled(c) { return !!(c && c.state && ['idle', 'rendering'].includes(c.state.status)); }
        async function loadRoots() {
            const token = ++flight, c = context(); if (!c) { return; }
            rootsFlight = true;
            try {
                const r = await query({kind: 'sources'});
                if (token !== flight || !available()) { return; }
                hier = true;
                if (r.sources.length === 1) {
                    const top = await query({kind: 'children', src: r.sources[0].src});
                    if (token !== flight || !available()) { return; }
                    const cell = cellFrom(r.sources[0].src, top.cell, top.name, 1, top.children.length === 0);
                    cell.hasShapes = true; cell.open = true; fill(cell, top);
                    roots = [cell];
                    note(top.name + ': ' + top.total + ' children');
                } else {
                    roots = r.sources.map(function (s) { const cell = cellFrom(s.src, null, s.name || ('source ' + s.src), s.placements, false); cell.source = true; return cell; });
                    note(r.sources.length + ' jobdeck sources');
                }
                paint();
            } catch (e) {
                if (token !== flight) { return; }
                if (e && /nohier/.test(String(e.code || e.message))) { hier = false; note('No cell index (design.ovh) for this source.'); }
                else { rootsFailedAt = port.now ? port.now() : Date.now(); note(String(e.message || e)); }
                update();
            } finally { if (token === flight) { rootsFlight = false; } }
        }
        function fill(cell, answer) {
            cell.ci = answer.cell; cell.children = answer.children.map(function (ch) { return cellFrom(cell.src, ch.ci, ch.name, ch.members, ch.leaf); });
            cell.more = Math.max(0, answer.total - answer.n); cell.loading = false; cell.leaf = cell.children.length === 0 && !cell.more;
            cell.localBbox = answer.bbox; cell.insts = answer.insts; cell.height = answer.height; cell.unit = answer.unit;
        }
        async function expand(cell) {
            if (cell.open) { cell.open = false; paint(); return; }
            cell.open = true;
            if (cell.children) { paint(); return; }
            cell.loading = true; paint();
            const token = flight;
            try {
                const body = {kind: 'children', src: cell.src}; if (cell.ci !== null && cell.ci !== undefined) { body.cell = cell.ci; }
                const r = await query(body);
                if (token !== flight || !available()) { return; }
                fill(cell, r); paint();
            } catch (e) { if (token === flight) { cell.loading = false; cell.open = false; note(String(e.message || e)); paint(); } }
        }
        function forget(cell) {
            cell.bbox = null; cell.hasShapes = undefined;
            if (cell.children) { cell.children.forEach(forget); }
        }
        async function choose(cell) {
            // Extents are placed in the current frame: never trust a cached
            // one across selections, ask again and keep zoom/root off meanwhile.
            cell.bbox = null; cell.hasShapes = undefined;
            selected = cell; paint();
            if (cell.source || cell.ci === null || cell.ci === undefined) { note(cell.name + ': ' + cell.members + ' placements'); return; }
            const token = ++bboxFlight;
            try {
                const r = await query({kind: 'bbox', src: cell.src, cell: cell.ci});
                if (token !== bboxFlight || selected !== cell) { return; }
                cell.bbox = r.bbox; cell.hasShapes = !!r.bbox;
                if (r.bbox) {
                    const u = Number(port.unit()), w = (r.bbox[2] - r.bbox[0]) * u, h = (r.bbox[3] - r.bbox[1]) * u;
                    note(cell.name + ': ' + r.insts + ' instance' + (r.insts === 1 ? '' : 's') + ' · ' + w.toPrecision(5) + ' × ' + h.toPrecision(5) + ' um' + (r.approx ? ' (approx)' : ''));
                } else { note(cell.name + ': not placed under the top cell'); }
                update(); refreshHighlight(true);
            } catch (e) { if (token === bboxFlight) { note(String(e.message || e)); } }
        }
        async function refreshHighlight(force) {
            const c = context(); const on = el('cells-highlight').checked;
            if (!c || !selected || !on || selected.ci === null || selected.ci === undefined || !c.state) { if (highlight) { highlight = null; highlightKey = ''; paintHighlight(); } return; }
            const key = c.id + ':' + c.state.bbox_dbu.join(',') + ':' + c.state.pixels.join('x') + ':' + selected.src + '/' + selected.ci;
            if (!force && key === highlightKey) { return; }
            const token = ++highlightFlight;
            try {
                const r = await query({kind: 'insts', src: selected.src, cell: selected.ci, view: c.state.bbox_dbu.map(Number), cap: INSTS_CAP});
                if (token !== highlightFlight) { return; }
                highlightKey = key; highlight = {boxes: r.boxes, more: r.more, bbox: c.state.bbox_dbu.map(Number), pixels: c.state.pixels.slice()};
                if (info.indexOf('instances in view') < 0 && selected.bbox) { el('cells-info').textContent = info + ' · ' + r.n + ' in view' + (r.more ? ' (more - zoom in)' : ''); }
                paintHighlight();
            } catch (e) { if (token === highlightFlight) { highlight = null; paintHighlight(); } }
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
        function zoom() {
            if (!selected || !selected.bbox || !available()) { return; }
            const u = Number(port.unit()), b = selected.bbox;
            const cx = (b[0] + b[2]) / 2 * u, cy = (b[1] + b[3]) / 2 * u, size = port.size();
            const aspect = size ? size.pixels[0] / size.pixels[1] : 1;
            const w = Math.max((b[2] - b[0]) * u, (b[3] - b[1]) * u * aspect) / FRAME_FRACTION;
            // Bounded decimals: the goto wire takes decimal strings, not floats.
            const fmt = function (n) { return String(Number(n.toPrecision(12))); };
            port.edit({navigation: {kind: 'goto', center_um: [fmt(cx), fmt(cy)], width_um: fmt(w > 0 ? w : 1)}});
            port.focus();
        }
        function setRoot() {
            if (!selected || !available() || !port.rootAllowed() || selected.ci === null || selected.ci === undefined) { return; }
            const cell = selected;
            port.edit({root: {src: cell.src, cell: cell.ci}}, function (error) { if (error) { note(String(error)); } else { rootName = cell.name; update(); } });
        }
        function clearRoot() { if (!available() || !rootName) { return; } port.edit({root: null}, function (error) { if (!error) { rootName = ''; update(); } }); }
        async function find() {
            const text = search.value.trim();
            if (!text) { results = null; paint(); return; }
            const token = ++flight;
            try {
                const r = await query({kind: 'find', src: -1, pattern: text, limit: FIND_LIMIT});
                if (token !== flight || search.value.trim() !== text) { return; }
                results = r.matches.map(function (m) { const cell = cellFrom(m.src, m.ci, m.name, m.insts, true); cell.hasShapes = true; return cell; });
                note(r.total + ' match' + (r.total === 1 ? '' : 'es') + (r.total > r.n ? ' (showing ' + r.n + ')' : ''));
                paint();
            } catch (e) { if (token === flight) { note(String(e.message || e)); } }
        }
        search.oninput = function () { if (timer) { port.clearTimeout(timer); } timer = port.setTimeout(function () { timer = null; find(); }, SEARCH_DELAY); };
        el('cells-search-form').onsubmit = function (e) { e.preventDefault(); if (timer) { port.clearTimeout(timer); timer = null; } if (selected && selected.bbox) { zoom(); } else { find(); } };
        search.onkeydown = function (e) { if (e.key === 'Escape' && !e.isComposing && e.keyCode !== 229) { e.preventDefault(); search.value = ''; results = null; paint(); port.focus(); } };
        el('cells-zoom').onclick = zoom;
        el('cells-root').onclick = setRoot;
        el('cells-top').onclick = clearRoot;
        el('cells-highlight').onchange = function () { refreshHighlight(true); };
        el('cells-build').onclick = function () { if (port.build) { port.build(); } };
        function changed() {
            const c = context();
            const id = c && c.id ? c.id : '';
            if (id !== identity) { identity = id; reset(); rootsFlight = false; rootsFailedAt = 0; }
            // The tree waits for a settled view: while the worker is still
            // opening, a query answers busy. One retry per later settled state.
            if (id && hier && !roots.length && !results && !rootsFlight && settled(c) && available()) {
                const now = port.now ? port.now() : Date.now();
                if (!rootsFailedAt || now - rootsFailedAt > 2000) { loadRoots(); return; }
            }
            if (id !== identity) { return; }
            if (c && c.state && c.state.root_name !== undefined) { const name = c.state.root_name || ''; if (name !== rootName) { rootName = name; } }
            // A root change moves the coordinate frame: placed extents and
            // highlights of the old frame are dropped and asked again.
            const key = c && c.state ? (c.state.root ? String(c.state.root.cell) : '') : '';
            if (key !== rootKey) {
                rootKey = key; ++highlightFlight; ++bboxFlight; highlight = null; highlightKey = '';
                roots.forEach(forget); if (results) { results.forEach(forget); }
                paintHighlight();
                if (selected && available() && settled(c)) { choose(selected); }
            }
            update();
            if (available()) { refreshHighlight(false); } else { paintHighlight(); }
        }
        function key(k, e) {
            if (!available()) { return false; }
            if (k === 't' && !e.ctrlKey && !e.metaKey) { port.raise(); search.focus(); search.select(); return true; }
            if ((k === 't' || k === 'T') && e.ctrlKey && !e.shiftKey) { setRoot(); return true; }
            if ((k === 'T' || k === 't') && e.ctrlKey && e.shiftKey) { clearRoot(); return true; }
            if (k === 'Escape' && highlight) { selected = null; highlight = null; highlightKey = ''; paint(); paintHighlight(); return true; }
            return false;
        }
        function suspend() { suspended = true; ++flight; ++highlightFlight; ++bboxFlight; if (timer) { port.clearTimeout(timer); timer = null; } paintHighlight(); update(); }
        function resume() { suspended = false; changed(); }
        function stop() { stopped = true; suspend(); }
        update();
        return Object.freeze({changed: changed, key: key, paint: paintHighlight, zoom: zoom, setRoot: setRoot, clearRoot: clearRoot,
            clearHighlight: function () { selected = null; highlight = null; highlightKey = ''; paint(); paintHighlight(); },
            hasSelection: function () { return !!selected; }, rootName: function () { return rootName; },
            suspend: suspend, resume: resume, stop: stop});
    }
    const api = {bind: bind};
    if (typeof module !== 'undefined' && module.exports) { module.exports = api; } else { root.FloeCells = api; }
}(typeof window === 'undefined' ? this : window));
