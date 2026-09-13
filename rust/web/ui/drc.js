/* ES2017 read-only review panel. Geometry projection is display-only; Rust
 * computes every focus viewport. IDs/cursors stay decimal strings. */
(function (root) {
    'use strict';
    function projection(frame, origin, unit) {
        if (!frame || !origin) { return null; }
        const b = frame.bbox_dbu.map(Number), dbu = Number(unit === undefined ? frame.dbu_um : unit);
        const sx = (b[2] - b[0]) / frame.width, sy = (b[3] - b[1]) / frame.height;
        if (!(sx > 0 && sy > 0 && dbu > 0)) { return null; }
        return {bbox: b, dbu: dbu, step: [sx, sy], origin: origin.slice()};
    }
    function point(p, x, y) {
        return [(x / p.dbu - p.bbox[0]) / p.step[0] - p.origin[0],
            (p.bbox[3] - y / p.dbu) / p.step[1] - p.origin[1]];
    }
    function shifted(p, delta) {
        return p && {bbox: p.bbox, dbu: p.dbu, step: p.step,
            origin: [p.origin[0] + delta[0], p.origin[1] + delta[1]]};
    }
    function bind(o) {
        const P = o.protocol, doc = o.document, el = function (id) { return doc.getElementById(id); };
        const overlay = el('drc-canvas'), ctx = overlay.getContext('2d');
        const tasks = {}, previousRules = [], previousErrors = [];
        let registration = null, bound = '', stopped = false, timer = null, shown = true;
        let rule = null, ruleRows = [], rows = [], selected = null, points = null, pointsReady = false;
        let ruleStart = '0', ruleNext = null, errorStart = '0', errorNext = null, query = null;
        let jumpScale = null, zoomLock = false, painting = null, lastProjection = null, lastSize = null;
        function info(s) { el('drc-message').textContent = s || ''; }
        function cancel(key) { const t = tasks[key]; if (t) { t.cancelled = true; if (t.abort) { t.abort(); } delete tasks[key]; } }
        function cancelAll() { Object.keys(tasks).forEach(cancel); }
        function current() {
            const c = o.context();
            return !stopped && registration && registration.phase === 'ready' && c &&
                c.source === registration.source_id && !['closed', 'failed'].includes(c.state.status) ? c : null;
        }
        function contextKey(c) { return c && registration ? c.id + ':' + registration.revision : ''; }
        function task(key) { cancel(key); const t = {cancelled: false, abort: null}; tasks[key] = t; return t; }
        function valid(key, t, c) { return !t.cancelled && tasks[key] === t && contextKey(current()) === contextKey(c); }
        async function read(key, t, c, body, lockView) {
            const envelope = {view_id: c.id, revision: registration.revision, body: body};
            if (lockView) { envelope.state_rev = c.state.state_rev; }
            const result = await o.http('POST', '/api/v1/drc/' + registration.id + '/read', envelope, false, t);
            if (!valid(key, t, c)) { return null; }
            if (lockView && current().state.state_rev !== c.state.state_rev) { return null; }
            return result;
        }
        function failure(key, t, c, e) { if (valid(key, t, c)) { info(e.message || String(e)); } }
        function cursor(s) { P.counter(s, true); return s; }
        function bbox(b) {
            if (!Array.isArray(b) || b.length !== 4) { throw new Error('Invalid DRC bounds'); }
            const out = b.map(function (s) { return Number(P.decimal(s)); });
            if (out[0] > out[2] || out[1] > out[3]) { throw new Error('Invalid DRC bounds'); } return out;
        }
        function validateRows(list) {
            if (!Array.isArray(list) || list.length > 64) { throw new Error('DRC page limit'); }
            list.forEach(function (r) { cursor(r.check); cursor(r.local); P.counter(r.global); bbox(r.bbox_um);
                if (!['p', 'e'].includes(r.kind) || !Number.isInteger(r.status) || r.status < 0 || r.status > 255) { throw new Error('Invalid DRC row'); } });
            return list;
        }
        function remember(history, value) { if (history.length === 128) { history.shift(); } history.push(value); }
        function navigationButtons() {
            const available = !!current();
            el('drc-rule-prev').disabled = !available || !previousRules.length;
            el('drc-rule-next').disabled = !available || ruleNext === null;
            el('drc-error-prev').disabled = !available || !previousErrors.length;
            el('drc-error-next').disabled = !available || errorNext === null;
            ['drc-search', 'drc-search-submit', 'drc-waived', 'drc-in-view', 'drc-first'].forEach(function (id) { el(id).disabled = !available; });
            el('drc-frame').disabled = !available || !selected;
            el('drc-clear').disabled = !selected;
        }
        function paintLater() {
            if (stopped || painting !== null) { return; }
            painting = o.window.requestAnimationFrame(function () { painting = null; paint(lastProjection, lastSize); });
        }
        function paint(p, size) {
            lastProjection = p; lastSize = size;
            if (!ctx || !p || !size || !current() || !el('drc-markers').checked || (!rows.length && !selected)) { overlay.hidden = true; return; }
            const w = size.pixels[0], h = size.pixels[1]; P.pixels(w, h);
            if (overlay.width !== w || overlay.height !== h) { overlay.width = w; overlay.height = h; }
            overlay.style.width = w / size.dpr + 'px'; overlay.style.height = h / size.dpr + 'px';
            overlay.style.left = size.left + 'px'; overlay.style.top = size.top + 'px'; overlay.hidden = false;
            ctx.clearRect(0, 0, w, h); ctx.lineWidth = 2;
            rows.forEach(function (r) {
                if (selected && r.check === selected.check && r.local === selected.local) { return; }
                const b = bbox(r.bbox_um), xy = point(p, b[0] * .5 + b[2] * .5, b[1] * .5 + b[3] * .5);
                if (!xy.every(Number.isFinite) || xy[0] < -8 || xy[0] > w + 8 || xy[1] < -8 || xy[1] > h + 8) { return; }
                ctx.fillStyle = r.status === 1 ? '#70da9a' : '#ff6969'; ctx.fillRect(Math.round(xy[0]) - 3, Math.round(xy[1]) - 3, 7, 7);
            });
            if (!selected) { return; }
            const b = bbox(selected.bbox_um), a = point(p, b[0], b[3]), z = point(p, b[2], b[1]);
            if (![a[0], a[1], z[0], z[1]].every(Number.isFinite) || z[0] < -9 || a[0] > w + 9 || z[1] < -9 || a[1] > h + 9) { return; }
            const color = selected.status === 1 ? '#70da9a' : '#ff6969'; ctx.strokeStyle = color; ctx.fillStyle = color;
            if (z[0] - a[0] < 9 && z[1] - a[1] < 9) { ctx.fillRect(Math.round((a[0] + z[0]) / 2) - 4, Math.round((a[1] + z[1]) / 2) - 4, 9, 9); return; }
            if (!pointsReady) { ctx.setLineDash([4, 3]); ctx.strokeRect(a[0], a[1], z[0] - a[0], z[1] - a[1]); ctx.setLineDash([]); return; }
            ctx.beginPath();
            for (let i = 0; i < points.length; i += 2) {
                const v = point(p, points[i], points[i + 1]);
                if (i === 0 || (selected.kind === 'e' && i % 4 === 0)) { ctx.moveTo(v[0], v[1]); } else { ctx.lineTo(v[0], v[1]); }
            }
            if (selected.kind === 'p') { ctx.closePath(); ctx.globalAlpha = .25; ctx.fill(); ctx.globalAlpha = 1; }
            ctx.stroke();
        }
        function clearSelection() {
            cancel('geometry'); cancel('focus'); selected = null; points = null; pointsReady = false;
            jumpScale = null; zoomLock = false; el('drc-selected').textContent = 'Select an error to inspect and go to it.';
            paintLater(); navigationButtons(); renderErrors();
        }
        function renderRules() {
            el('drc-rules').textContent = '';
            ruleRows.forEach(function (r) {
                const b = doc.createElement('button'); b.className = 'drc-rule' + (rule && rule.check === r.check ? ' selected' : '');
                b.textContent = r.name + (r.name_truncated ? '…' : '') + '  ·  ' + r.errors; b.title = r.waived + ' waived';
                b.onclick = function () { chooseRule(r); }; el('drc-rules').appendChild(b);
            });
            if (!ruleRows.length) { el('drc-rules').textContent = ruleNext === null ? 'No matching rules.' : 'No match in this scan. Continue to the next page.'; }
            navigationButtons();
        }
        async function loadRules() {
            const c = current(); if (!c) { return; }
            const t = task('rules'), search = el('drc-search').value.trim();
            try {
                if (new TextEncoder().encode(search).length > 256) { throw new Error('Rule search is limited to 256 UTF-8 bytes.'); }
                const page = await read('rules', t, c, {kind: 'rules', start: ruleStart, search: search, limit: 32}); if (!page) { return; }
                if (!Array.isArray(page.rows) || page.rows.length > 32) { throw new Error('DRC rule page limit'); }
                page.rows.forEach(function (r) { cursor(r.check); cursor(r.errors); cursor(r.waived); if (typeof r.name !== 'string') { throw new Error('Invalid rule name'); } });
                ruleRows = page.rows; ruleNext = page.next === null ? null : cursor(page.next); renderRules();
                if (!rule && ruleRows.length) { chooseRule(ruleRows[0]); }
            } catch (e) { failure('rules', t, c, e); }
        }
        function chooseRule(r) {
            if (!current()) { return; }
            rule = r; query = null; errorStart = '0'; previousErrors.length = 0; rows = []; errorNext = null;
            clearSelection(); renderRules(); el('drc-rule-title').textContent = r.name; el('drc-description').textContent = '';
            const c = current(), t = task('description');
            read('description', t, c, {kind: 'rule', check: r.check}).then(function (page) {
                if (page) { el('drc-description').textContent = page.description; }
            }).catch(function (e) { failure('description', t, c, e); });
            loadErrors();
        }
        function filter() { return el('drc-waived').value === 'all' ? null : el('drc-waived').value === 'waived'; }
        function renderErrors() {
            el('drc-errors').textContent = '';
            rows.forEach(function (r, i) {
                const b = doc.createElement('button'), active = selected && selected.check === r.check && selected.local === r.local;
                b.className = 'drc-error' + (r.status === 1 ? ' waived' : '') + (active ? ' selected' : '');
                b.textContent = '#' + P.next(r.local) + '  ·  global ' + r.global + '  ·  ' + (r.kind === 'p' ? 'poly' : 'edge') + (r.status === 1 ? '  ·  waived' : '');
                b.setAttribute('aria-label', 'Error ' + P.next(r.local) + ', global ' + r.global);
                b.onclick = function () { select(r, false); }; b.ondblclick = function () { select(r, true); };
                b.onkeydown = function (e) { if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                    const j = i + (e.key === 'ArrowDown' ? 1 : -1); if (j >= 0 && j < rows.length) { e.preventDefault(); select(rows[j], false); }
                } };
                el('drc-errors').appendChild(b);
            });
            if (!rows.length) { el('drc-errors').textContent = errorNext === null ? 'No matching errors.' : 'No match in this scan. Continue to the next page.'; }
            el('drc-result-info').textContent = (query ? 'Viewport query' : 'Rule errors') + ' · ' + rows.length + ' on this page' + (errorNext === null ? ' · end' : ' · more available');
            navigationButtons(); paintLater();
        }
        async function loadErrors() {
            const c = current(); if (!c || (!rule && !query)) { return; }
            const t = task('errors'), body = query ? (query.bbox ? {kind: 'query', bbox_um: query.bbox, checks: null, cursor: errorStart, waived: filter(), limit: 64} :
                {kind: 'in_view', cursor: errorStart, waived: filter(), limit: 64}) : {kind: 'errors', check: rule.check, start: errorStart, waived: filter(), limit: 64};
            try {
                const page = await read('errors', t, c, body, body.kind === 'in_view'); if (!page) { return; }
                if (body.kind === 'in_view') { P.bbox(page.bbox_um); query.bbox = page.bbox_um; }
                rows = validateRows(page.rows); errorNext = page.next;
                if (errorNext !== null) { if (query) { cursor(errorNext.check); cursor(errorNext.error); } else { cursor(errorNext); } }
                if (JSON.stringify(errorNext) === JSON.stringify(errorStart)) { throw new Error('DRC cursor did not progress'); }
                renderErrors(); info('Read-only · review files are never changed.');
            } catch (e) { failure('errors', t, c, e); }
        }
        async function geometry(r) {
            const c = current(), t = task('geometry'); let start = '0', total = null;
            try {
                for (let pageNo = 0; pageNo < 128; ++pageNo) {
                    const page = await read('geometry', t, c, {kind: 'geometry', check: r.check, error: r.local, start: start, limit: 2048}); if (!page) { return; }
                    cursor(page.total); cursor(page.start);
                    const n = Number(page.total), unit = Number(P.decimal(page.precision));
                    if (page.check !== r.check || page.local !== r.local || page.start !== start || n < 1 || n > 262144 || !(unit > 0) || !Array.isArray(page.points_dbu) || !page.points_dbu.length || page.points_dbu.length > 2048) { throw new Error('Invalid DRC geometry page'); }
                    if (total === null) { total = n; points = new Float64Array(n * 2); } else if (n !== total) { throw new Error('DRC geometry changed'); }
                    const offset = Number(start), end = offset + page.points_dbu.length;
                    if (end > total) { throw new Error('DRC geometry bounds'); }
                    page.points_dbu.forEach(function (xy, i) {
                        if (!Array.isArray(xy) || xy.length !== 2) { throw new Error('Invalid DRC vertex'); }
                        xy.forEach(function (s, axis) { const n = Number(P.decimal(s)) / unit; if (!Number.isFinite(n)) { throw new Error('Unrepresentable DRC vertex'); } points[(offset + i) * 2 + axis] = n; });
                    });
                    el('drc-selected').textContent = 'Global ' + r.global + ' · ' + end + '/' + total + ' vertices' + (page.next === null ? '' : ' · bounding-box preview');
                    if (page.next === null) { if (end !== total) { throw new Error('Incomplete DRC outline'); } pointsReady = true; paintLater(); return; }
                    if (cursor(page.next) !== String(end)) { throw new Error('DRC geometry cursor did not progress'); } start = page.next;
                }
                throw new Error('DRC geometry page limit');
            } catch (e) { failure('geometry', t, c, e); }
        }
        async function focus(r, frame) {
            const c = current(); if (!c || !c.connected || c.pending) { info('Wait for the current view edit before going to an error.'); return; }
            const t = task('focus'), s = c.state, b = s.bbox_dbu.map(Number), scale = (b[2] - b[0]) * Number(s.dbu_um) / s.pixels[0];
            if (frame) { zoomLock = false; } else if (jumpScale !== null && Math.abs(scale / jumpScale - 1) > 1e-6) { zoomLock = true; }
            try {
                const page = await read('focus', t, c, {kind: 'focus', check: r.check, error: r.local, fit: !zoomLock}, true); if (!page) { return; }
                if (current().pending) { info('View input changed; select the error again to move.'); return; }
                jumpScale = Number(P.decimal(page.navigation.width_um)) / s.pixels[0]; o.navigate(page.navigation);
            } catch (e) { failure('focus', t, c, e); }
        }
        function select(r, frame) {
            if (!current()) { return; }
            const same = selected && selected.check === r.check && selected.local === r.local;
            if (!selected) { jumpScale = null; zoomLock = false; }
            selected = r;
            if (!same || !pointsReady) { points = null; pointsReady = false; el('drc-selected').textContent = 'Global ' + r.global + ' · bounding-box preview'; geometry(r); }
            renderErrors(); focus(r, frame);
        }
        function contextChanged() {
            const c = current(), key = contextKey(c);
            if (key !== bound) {
                cancelAll(); bound = key; rule = null; ruleRows = []; rows = []; ruleStart = errorStart = '0'; query = null;
                previousRules.length = previousErrors.length = 0; ruleNext = errorNext = null; clearSelection(); renderRules();
                el('drc-rule-title').textContent = 'Choose a rule'; el('drc-description').textContent = '';
                if (c) { loadRules(); }
            }
            navigationButtons();
            if (registration && !c && registration.phase === 'ready') { info('Open the source associated with this DRC pack.'); }
            if (query && c && query.rev !== c.state.state_rev) { el('drc-result-info').textContent = 'Results from an earlier viewport · click In view to refresh.'; }
        }
        async function refresh() {
            if (stopped) { return; }
            const t = task('catalog');
            try {
                const v = await o.http('GET', '/api/v1/drc', undefined, false, t); if (t.cancelled || stopped) { return; }
                registration = v.drc; el('drc-toggle').hidden = !registration; el('drc-panel').hidden = !registration || !shown;
                if (registration) {
                    el('drc-title').textContent = registration.title;
                    el('drc-summary').textContent = registration.metadata ? registration.metadata.checks + ' rules · ' + registration.metadata.errors + ' errors' : registration.phase;
                    info(registration.error || (registration.phase === 'opening' ? 'Opening DRC metadata…' : 'Read-only review'));
                    if (registration.phase === 'opening') { timer = setTimeout(refresh, 500); }
                }
                contextChanged();
                if (selected && !pointsReady && current()) { geometry(selected); }
            } catch (e) { if (!t.cancelled) { info(e.message); } }
        }
        el('drc-search-form').onsubmit = function (e) { e.preventDefault(); ruleStart = '0'; previousRules.length = 0; loadRules(); };
        el('drc-rule-prev').onclick = function () { if (previousRules.length) { ruleStart = previousRules.pop(); loadRules(); } };
        el('drc-rule-next').onclick = function () { if (ruleNext !== null) { remember(previousRules, ruleStart); ruleStart = ruleNext; loadRules(); } };
        el('drc-error-prev').onclick = function () { if (previousErrors.length) { errorStart = previousErrors.pop(); loadErrors(); } };
        el('drc-error-next').onclick = function () { if (errorNext !== null) { remember(previousErrors, errorStart); errorStart = errorNext; loadErrors(); } };
        el('drc-first').onclick = function () { previousErrors.length = 0; errorStart = query ? {check: '0', error: '0'} : '0'; loadErrors(); };
        el('drc-waived').onchange = function () { clearSelection(); el('drc-first').onclick(); };
        el('drc-in-view').onclick = function () {
            const c = current(); if (!c || c.pending) { info('Wait for the current view edit.'); return; }
            query = {bbox: null, rev: c.state.state_rev}; errorStart = {check: '0', error: '0'}; previousErrors.length = 0; clearSelection(); loadErrors();
        };
        el('drc-frame').onclick = function () { if (selected) { focus(selected, true); } };
        el('drc-clear').onclick = clearSelection;
        el('drc-markers').onchange = paintLater;
        el('drc-toggle').onclick = function () { shown = !shown; el('drc-panel').hidden = !shown; el('drc-toggle').setAttribute('aria-expanded', String(shown)); o.resize(); };
        return {init: refresh, contextChanged: contextChanged, paint: paint, clear: clearSelection,
            key: function (key) {
                if (key === 'Escape' && selected) { clearSelection(); return true; }
                if ((key === 'n' || key === 'p') && selected) {
                    const i = rows.findIndex(function (r) { return r.check === selected.check && r.local === selected.local; });
                    const j = i + (key === 'n' ? 1 : -1);
                    if (i >= 0 && j >= 0 && j < rows.length) { select(rows[j], false); }
                    else { info('End of this error page. Use the page arrows to continue.'); }
                    return true;
                } return false;
            },
            stop: function () { stopped = true; cancelAll(); clearTimeout(timer); if (painting !== null) { o.window.cancelAnimationFrame(painting); painting = null; } overlay.hidden = true; },
            resume: function () { stopped = false; return refresh(); }};
    }
    const api = {bind: bind, projection: projection, point: point, shifted: shifted};
    if (typeof module === 'object' && module.exports) { module.exports = api; } else { root.FloeDRC = api; }
}(typeof window === 'object' ? window : this));
