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
        let jumpActive = false, focusVisible = false, stepBusy = false, stepContinuation = null, rowFocus = false;
        let markerHits = [], hitStamp = '';
        let restoring = false;
        const persistence = o.stateStore.bind({http: o.http, protocol: P,
            setTimeout: function (fn, delay) { return setTimeout(fn, delay); },
            clearTimeout: function (id) { clearTimeout(id); },
            apply: restorePanel, status: function (s) { el('drc-sync-status').textContent = s; }});
        function info(s) { el('drc-message').textContent = s || ''; }
        function cancel(key) { const t = tasks[key]; if (t) { t.cancelled = true; if (t.abort) { t.abort(); } delete tasks[key]; } }
        function cancelAll() { Object.keys(tasks).forEach(cancel); }
        function cancelStep() { cancel('step'); stepBusy = false; stepContinuation = null; el('drc-step-continue').hidden = true; }
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
            const available = !!current() && !restoring;
            el('drc-rule-prev').disabled = !available || !previousRules.length;
            el('drc-rule-next').disabled = !available || ruleNext === null;
            el('drc-error-prev').disabled = !available || !previousErrors.length;
            el('drc-error-next').disabled = !available || errorNext === null;
            ['drc-search', 'drc-search-submit', 'drc-waived', 'drc-in-view', 'drc-first'].forEach(function (id) { el(id).disabled = !available; });
            el('drc-frame').disabled = !available || !selected;
            el('drc-clear').disabled = restoring || !selected;
            el('drc-reload').disabled = !current();
            ['drc-step-prev', 'drc-step-next'].forEach(function (id) { el(id).disabled = !available || !rule || stepBusy || !!(query && !query.bbox); });
            el('drc-step-continue').disabled = !available || stepBusy || !stepContinuation;
        }
        function paintLater() {
            markerHits = []; hitStamp = '';
            if (stopped || painting !== null) { return; }
            painting = o.window.requestAnimationFrame(function () { painting = null; paint(lastProjection, lastSize); });
        }
        function marker(r, xy, side, w, h) {
            if (!xy.every(Number.isFinite)) { return; }
            const x = Math.round(xy[0]), y = Math.round(xy[1]), half = Math.floor(side / 2);
            if (x - half >= w || x + half < 0 || y - half >= h || y + half < 0) { return; }
            ctx.fillStyle = r.status === 1 ? '#70da9a' : '#ff6969'; ctx.fillRect(x - half, y - half, side, side);
            markerHits.push({x: x, y: y, row: r});
        }
        function paint(p, size) {
            markerHits = []; hitStamp = ''; lastProjection = p; lastSize = size;
            const c = current();
            if (!ctx || !p || !size || !c || !el('drc-markers').checked || (!rows.length && (!selected || !focusVisible))) { overlay.hidden = true; return; }
            const w = size.pixels[0], h = size.pixels[1]; P.pixels(w, h);
            if (overlay.width !== w || overlay.height !== h) { overlay.width = w; overlay.height = h; }
            overlay.style.width = w / size.dpr + 'px'; overlay.style.height = h / size.dpr + 'px';
            overlay.style.left = size.left + 'px'; overlay.style.top = size.top + 'px'; overlay.hidden = false;
            hitStamp = contextKey(c) + ':' + c.state.state_rev;
            ctx.clearRect(0, 0, w, h); ctx.lineWidth = 2;
            rows.forEach(function (r) {
                if (focusVisible && selected && r.check === selected.check && r.local === selected.local) { return; }
                const b = bbox(r.bbox_um), xy = point(p, b[0] * .5 + b[2] * .5, b[1] * .5 + b[3] * .5);
                marker(r, xy, 7, w, h);
            });
            if (!selected || !focusVisible) { return; }
            const b = bbox(selected.bbox_um), a = point(p, b[0], b[3]), z = point(p, b[2], b[1]);
            if (![a[0], a[1], z[0], z[1]].every(Number.isFinite) || z[0] < -9 || a[0] > w + 9 || z[1] < -9 || a[1] > h + 9) { return; }
            const color = selected.status === 1 ? '#70da9a' : '#ff6969'; ctx.strokeStyle = color; ctx.fillStyle = color;
            // Keep a visible anchor when selection reveals an outline. The
            // second release of a double-click must still have a drawn target.
            marker(selected, [(a[0] + z[0]) / 2, (a[1] + z[1]) / 2], 9, w, h);
            if (z[0] - a[0] < 9 && z[1] - a[1] < 9) { return; }
            if (!pointsReady) { ctx.setLineDash([4, 3]); ctx.strokeRect(a[0], a[1], z[0] - a[0], z[1] - a[1]); ctx.setLineDash([]); return; }
            ctx.beginPath();
            for (let i = 0; i < points.length; i += 2) {
                const v = point(p, points[i], points[i + 1]);
                if (i === 0 || (selected.kind === 'e' && i % 4 === 0)) { ctx.moveTo(v[0], v[1]); } else { ctx.lineTo(v[0], v[1]); }
            }
            if (selected.kind === 'p') { ctx.closePath(); ctx.globalAlpha = .25; ctx.fill(); ctx.globalAlpha = 1; }
            ctx.stroke();
        }
        function click(clientX, clientY, twice) {
            const c = current();
            if (!c || !c.connected || c.pending || restoring || overlay.hidden || !el('drc-markers').checked ||
                hitStamp !== contextKey(c) + ':' + c.state.state_rev || !markerHits.length ||
                !Number.isFinite(clientX) || !Number.isFinite(clientY)) { return false; }
            // Use the *painted* overlay's DOM rectangle, including fractional
            // CSS origins/DPR/margin translations. Do not unproject through a
            // newer requested viewport or issue an all-error spatial scan.
            const rect = overlay.getBoundingClientRect();
            if (!(rect.width > 0 && rect.height > 0) || clientX < rect.left || clientX >= rect.right || clientY < rect.top || clientY >= rect.bottom) { return false; }
            let best = null, distance = 36 + 1;
            markerHits.forEach(function (hit) {
                const dx = (hit.x / overlay.width * rect.width + rect.left) - clientX;
                const dy = (hit.y / overlay.height * rect.height + rect.top) - clientY, d = dx * dx + dy * dy;
                if (d <= 36 && d < distance) { best = hit.row; distance = d; }
            });
            if (!best) { return false; }
            // Like GTK's canvas marker pick, a single click only selects,
            // even in jump mode. A double click explicitly requests focus.
            cancelStep(); rowFocus = false; select(best, !!twice, !!twice); return true;
        }
        function clearSelection() {
            cancelStep(); jumpActive = focusVisible = rowFocus = false;
            cancel('geometry'); cancel('focus'); selected = null; points = null; pointsReady = false;
            jumpScale = null; zoomLock = false; el('drc-selected').textContent = 'Click to inspect · double-click to go to an error.';
            paintLater(); navigationButtons(); renderErrors(); savePanel();
        }
        function endFocus() {
            const hadWork = stepBusy || stepContinuation || (selected && (focusVisible || jumpActive));
            if (!hadWork) { return false; }
            cancelStep(); cancel('focus'); cancel('geometry'); points = null; pointsReady = false;
            jumpActive = focusVisible = rowFocus = false;
            if (selected) { el('drc-selected').textContent = 'Global ' + selected.global + ' · focus cleared; n/p continues without moving the view.'; }
            info('Focus cleared · pending error search cancelled.');
            paintLater(); navigationButtons(); savePanel(); return true;
        }
        function renderRules() {
            el('drc-rules').textContent = '';
            ruleRows.forEach(function (r) {
                const b = doc.createElement('button'); b.className = 'drc-rule' + (rule && rule.check === r.check ? ' selected' : '');
                b.textContent = r.name + (r.name_truncated ? '…' : '') + '  ·  ' + r.errors; b.title = r.waived + ' waived';
                b.disabled = restoring;
                b.onclick = function () { chooseRule(r); }; el('drc-rules').appendChild(b);
            });
            if (!ruleRows.length) { el('drc-rules').textContent = ruleNext === null ? 'No matching rules.' : 'No match in this scan. Continue to the next page.'; }
            navigationButtons();
        }
        async function loadRules(autoSelect, strict) {
            const c = current(); if (!c) { return; }
            const t = task('rules'), search = el('drc-search').value.trim();
            try {
                if (new TextEncoder().encode(search).length > 256) { throw new Error('Rule search is limited to 256 UTF-8 bytes.'); }
                const page = await read('rules', t, c, {kind: 'rules', start: ruleStart, search: search, limit: 32}); if (!page) { return; }
                if (!Array.isArray(page.rows) || page.rows.length > 32) { throw new Error('DRC rule page limit'); }
                page.rows.forEach(function (r) { cursor(r.check); cursor(r.errors); cursor(r.waived); if (typeof r.name !== 'string') { throw new Error('Invalid rule name'); } });
                ruleRows = page.rows; ruleNext = page.next === null ? null : cursor(page.next); renderRules();
                if (autoSelect !== false && !rule && ruleRows.length) { chooseRule(ruleRows[0]); }
                savePanel();
            } catch (e) { if (strict) { throw e; } failure('rules', t, c, e); }
        }
        function chooseRule(r) {
            if (!current()) { return; }
            rule = r; query = null; errorStart = '0'; previousErrors.length = 0; rows = []; errorNext = null;
            clearSelection(); renderRules(); el('drc-rule-title').textContent = r.name; el('drc-description').textContent = '';
            const c = current(), t = task('description');
            read('description', t, c, {kind: 'rule', check: r.check}).then(function (page) {
                if (page) { el('drc-description').textContent = page.description; }
            }).catch(function (e) { failure('description', t, c, e); });
            loadErrors(); savePanel();
        }
        function filter() { return el('drc-waived').value === 'all' ? null : el('drc-waived').value === 'waived'; }
        function markErrors() {
            Array.prototype.forEach.call(el('drc-errors').children, function (b, i) {
                const r = rows[i], active = r && selected && selected.check === r.check && selected.local === r.local;
                b.className = 'drc-error' + (r && r.status === 1 ? ' waived' : '') + (active ? ' selected' : '');
                if (active && rowFocus) { rowFocus = false; b.focus(); b.scrollIntoView({block: 'nearest'}); }
            });
        }
        function renderErrors() {
            if (selected && doc.activeElement && el('drc-errors').contains(doc.activeElement)) { rowFocus = true; }
            el('drc-errors').textContent = '';
            rows.forEach(function (r) {
                const b = doc.createElement('button');
                b.disabled = restoring;
                b.textContent = '#' + P.next(r.local) + '  ·  global ' + r.global + '  ·  ' + (r.kind === 'p' ? 'poly' : 'edge') + (r.status === 1 ? '  ·  waived' : '');
                b.setAttribute('aria-label', 'Error ' + P.next(r.local) + ', global ' + r.global);
                b.onclick = function () { cancelStep(); select(r, false); };
                b.ondblclick = function () { cancelStep(); select(r, true); };
                b.onkeydown = function (e) {
                    if (e.ctrlKey || e.metaKey || e.altKey || e.isComposing) { return; }
                    if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'n' || e.key === 'p') {
                        e.preventDefault(); step(e.key === 'ArrowUp' || e.key === 'p', false, true);
                    } else if (e.key === 'Escape' && endFocus()) { e.preventDefault(); }
                };
                el('drc-errors').appendChild(b);
            });
            if (!rows.length) { el('drc-errors').textContent = errorNext === null ? 'No matching errors.' : 'No match in this scan. Continue to the next page.'; }
            el('drc-result-info').textContent = (query ? 'Viewport query' : 'Rule errors') + ' · ' + rows.length + ' on this page' + (errorNext === null ? ' · end' : ' · more available');
            markErrors(); navigationButtons(); paintLater();
        }
        async function loadErrors(strict) {
            const c = current(); if (!c || (!rule && !query)) { return; }
            const t = task('errors'), body = query ? (query.bbox ? {kind: 'query', bbox_um: query.bbox, checks: null, cursor: errorStart, waived: filter(), limit: 64} :
                {kind: 'in_view', cursor: errorStart, waived: filter(), limit: 64}) : {kind: 'errors', check: rule.check, start: errorStart, waived: filter(), limit: 64};
            try {
                const page = await read('errors', t, c, body, body.kind === 'in_view'); if (!page) { return; }
                if (body.kind === 'in_view') { P.bbox(page.bbox_um); query.bbox = page.bbox_um; }
                rows = validateRows(page.rows); errorNext = page.next;
                if (errorNext !== null) { if (query) { cursor(errorNext.check); cursor(errorNext.error); } else { cursor(errorNext); } }
                if (JSON.stringify(errorNext) === JSON.stringify(errorStart)) { throw new Error('DRC cursor did not progress'); }
                renderErrors(); info('Read-only · review files are never changed.'); savePanel();
            } catch (e) { if (strict) { throw e; } failure('errors', t, c, e); }
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
                jumpScale = Number(P.decimal(page.navigation.width_um)) / s.pixels[0]; o.navigate(page.navigation); savePanel();
            } catch (e) { failure('focus', t, c, e); }
        }
        function syncRule(check) {
            if (rule && rule.check === check) { return; }
            rule = {check: check, name: 'Rule ' + P.next(check)};
            renderRules(); el('drc-rule-title').textContent = rule.name; el('drc-description').textContent = '';
            const c = current(), t = task('description');
            read('description', t, c, {kind: 'rule', check: check}).then(function (v) {
                if (!v || !rule || rule.check !== check) { return; }
                cursor(v.errors); cursor(v.waived); rule = {check: check, name: v.name, errors: v.errors, waived: v.waived};
                el('drc-rule-title').textContent = v.name; el('drc-description').textContent = v.description; renderRules();
            }).catch(function (e) { failure('description', t, c, e); });
        }
        function select(r, frame, allowFocus) {
            if (!current()) { return; }
            cancel('focus'); syncRule(r.check);
            const same = selected && selected.check === r.check && selected.local === r.local;
            if (!selected) { jumpScale = null; zoomLock = false; }
            selected = r; focusVisible = true; if (frame) { jumpActive = true; }
            if (!same || !pointsReady) { points = null; pointsReady = false; el('drc-selected').textContent = 'Global ' + r.global + ' · bounding-box preview'; geometry(r); }
            // Keep the button node alive between click and double-click.
            markErrors(); navigationButtons(); paintLater();
            if (jumpActive && allowFocus !== false) { focus(r, frame); }
            savePanel();
        }
        async function step(backwards, resume, focusRow) {
            const c = current(); if (!c || restoring || !rule || stepBusy || (query && !query.bbox)) { return; }
            let body;
            if (resume) { if (!stepContinuation) { return; } body = stepContinuation; }
            else {
                stepContinuation = null; el('drc-step-continue').hidden = true;
                let after = selected && selected.check === rule.check && (filter() === null || (selected.status === 1) === filter()) ? selected.local : null;
                if (after !== null && query) {
                    const a = bbox(selected.bbox_um), b = bbox(query.bbox);
                    if (a[0] > b[2] || a[2] < b[0] || a[1] > b[3] || a[3] < b[1]) { after = null; }
                }
                body = {kind: 'step', check: rule.check, backwards: backwards, after: after, cursor: null, waived: filter(), bbox_um: query ? query.bbox : null};
            }
            cancel('focus'); const t = task('step'); stepBusy = true; rowFocus = false;
            navigationButtons(); info('Searching the current rule…');
            try {
                const page = await read('step', t, c, body); if (!page) { return; }
                cursor(page.scanned);
                if (P.compare(page.scanned, '262144') > 0 || page.hit === undefined || page.next === undefined || (page.hit !== null && page.next !== null)) { throw new Error('Invalid DRC step response'); }
                if (page.next !== null) {
                    cursor(page.next.next); P.counter(page.next.remaining);
                    if (page.scanned === '0' || (body.cursor && P.compare(page.next.remaining, body.cursor.remaining) >= 0)) { throw new Error('DRC step cursor did not progress'); }
                    stepContinuation = Object.assign({}, body, {after: null, cursor: page.next});
                    el('drc-step-continue').hidden = false;
                    info('Search incomplete · ' + page.scanned + ' slots checked. Continue search to resume; no error was skipped.');
                    return;
                }
                stepContinuation = null; el('drc-step-continue').hidden = true;
                if (page.hit === null) { info('No matching errors in the current rule.'); return; }
                const r = validateRows([page.hit])[0];
                cursor(r.points);
                if (r.check !== body.check || (body.waived !== null && (r.status === 1) !== body.waived)) { throw new Error('DRC step filter mismatch'); }
                const existing = rows.some(function (v) { return v.check === r.check && v.local === r.local; });
                rowFocus = !!focusRow;
                if (!existing) {
                    remember(previousErrors, errorStart); errorStart = query ? {check: r.check, error: r.local} : r.local;
                    rows = [r]; errorNext = null; renderErrors(); loadErrors();
                }
                const allowFocus = !c.pending && !current().pending && current().state.state_rev === c.state.state_rev;
                select(r, false, allowFocus);
                info(allowFocus || !jumpActive ? 'Read-only · review files are never changed.' : 'View changed during search; selected without moving.');
            } catch (e) { failure('step', t, c, e); }
            finally { if (valid('step', t, c)) { stepBusy = false; navigationButtons(); } }
        }
        function panelData() {
            if (query && !query.bbox) { return null; }
            return {search: el('drc-search').value.trim(), rule_start: ruleStart, check: rule ? rule.check : null,
                error_start: query ? '0' : errorStart, query: query ? {bbox_um: query.bbox, state_rev: query.rev, cursor: errorStart} : null,
                waived: filter(), selected: selected ? {check: selected.check, error: selected.local} : null,
                markers: el('drc-markers').checked, shown: shown, jump_scale: jumpScale === null ? null : String(jumpScale), zoom_lock: zoomLock,
                jump_active: jumpActive, focus_visible: focusVisible};
        }
        function savePanel() {
            if (restoring || !current()) { return; }
            const data = panelData(); if (data) { persistence.change(data); }
        }
        async function restorePanel(data) {
            const c = current(); if (!c) { return; }
            const t = task('restore'); restoring = true;
            cancel('rules'); cancel('errors'); cancel('description'); clearSelection();
            rule = null; rows = []; query = null; previousRules.length = previousErrors.length = 0;
            ruleStart = errorStart = '0'; ruleNext = errorNext = null;
            navigationButtons();
            try {
                if (!data) { await loadRules(undefined, true); return; }
                if (typeof data.search !== 'string' || new TextEncoder().encode(data.search).length > 256 ||
                    (data.waived !== null && typeof data.waived !== 'boolean') ||
                    !['markers','shown','zoom_lock','jump_active','focus_visible'].every(function (k) { return typeof data[k] === 'boolean'; }) ||
                    (data.jump_active && !data.focus_visible) || ((data.jump_active || data.focus_visible) && !data.selected)) { throw new Error('Invalid saved review state'); }
                ruleStart = cursor(data.rule_start); errorStart = cursor(data.error_start);
                el('drc-search').value = data.search; el('drc-waived').value = data.waived === null ? 'all' : data.waived ? 'waived' : 'unwaived';
                el('drc-markers').checked = data.markers;
                if (shown !== data.shown) { shown = data.shown; el('drc-panel').hidden = !shown; el('drc-toggle').setAttribute('aria-expanded', String(shown)); o.resize(); }
                jumpScale = data.jump_scale === null ? null : Number(P.decimal(data.jump_scale)); zoomLock = data.zoom_lock;
                jumpActive = data.jump_active; focusVisible = data.focus_visible;
                if (jumpScale !== null && !(jumpScale > 0)) { throw new Error('Invalid saved zoom scale'); }
                if (data.query) { P.bbox(data.query.bbox_um); P.counter(data.query.state_rev);
                    cursor(data.query.cursor.check); cursor(data.query.cursor.error);
                    query = {bbox: data.query.bbox_um, rev: data.query.state_rev}; errorStart = data.query.cursor;
                }
                await loadRules(false, true); if (!valid('restore', t, c)) { return; }
                if (data.check !== null) {
                    cursor(data.check);
                    const page = await read('restore', t, c, {kind: 'rule', check: data.check}); if (!page) { return; }
                    rule = {check: data.check, name: page.name, errors: page.errors, waived: page.waived};
                    el('drc-rule-title').textContent = page.name; el('drc-description').textContent = page.description;
                } else { el('drc-rule-title').textContent = 'Choose a rule'; el('drc-description').textContent = ''; }
                await loadErrors(true); if (!valid('restore', t, c)) { return; }
                if (data.selected) {
                    const ref = data.selected; cursor(ref.check); cursor(ref.error);
                    const page = await read('restore', t, c, {kind: 'geometry', check: ref.check, error: ref.error, start: '0', limit: 1}); if (!page) { return; }
                    selected = validateRows([page])[0]; points = null; pointsReady = false;
                    el('drc-selected').textContent = 'Global ' + selected.global + (focusVisible ? ' · bounding-box preview' : ' · focus cleared; n/p continues without moving the view.');
                    if (focusVisible) { geometry(selected); }
                }
            } finally {
                if (valid('restore', t, c)) { restoring = false; renderRules(); renderErrors(); contextChanged(); }
            }
        }
        function restoreState() {
            const c = current(); if (!c) { return; }
            // Cancel immediately, not after the panel GET completes. A slow
            // previous focus/step must not move the view during restoration.
            cancelStep(); ['focus', 'geometry', 'rules', 'errors', 'description', 'restore'].forEach(cancel);
            const key = contextKey(c); restoring = true; navigationButtons(); renderRules(); renderErrors();
            return persistence.attach({path: '/api/v1/drc/' + registration.id + '/views/' + c.id + '/panel',
                revision: registration.revision, view: c.id}).then(function () {
                if (contextKey(current()) === key) { restoring = false; renderRules(); renderErrors(); contextChanged(); }
            });
        }
        function contextChanged() {
            markerHits = []; hitStamp = '';
            const c = current(), key = contextKey(c);
            if (key !== bound) {
                persistence.close(); cancelAll(); bound = key; restoring = false; rule = null; ruleRows = []; rows = []; ruleStart = errorStart = '0'; query = null;
                previousRules.length = previousErrors.length = 0; ruleNext = errorNext = null; clearSelection(); renderRules();
                el('drc-rule-title').textContent = 'Choose a rule'; el('drc-description').textContent = '';
                if (c) { restoreState(); }
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
                if (focusVisible && selected && !pointsReady && current()) { geometry(selected); }
            } catch (e) { if (!t.cancelled) { info(e.message); } }
        }
        el('drc-search-form').onsubmit = function (e) { e.preventDefault(); ruleStart = '0'; previousRules.length = 0; loadRules(); };
        el('drc-rule-prev').onclick = function () { if (previousRules.length) { ruleStart = previousRules.pop(); loadRules(); } };
        el('drc-rule-next').onclick = function () { if (ruleNext !== null) { remember(previousRules, ruleStart); ruleStart = ruleNext; loadRules(); } };
        el('drc-error-prev').onclick = function () { cancelStep(); if (previousErrors.length) { errorStart = previousErrors.pop(); loadErrors(); } };
        el('drc-error-next').onclick = function () { cancelStep(); if (errorNext !== null) { remember(previousErrors, errorStart); errorStart = errorNext; loadErrors(); } };
        el('drc-first').onclick = function () { cancelStep(); previousErrors.length = 0; errorStart = query ? {check: '0', error: '0'} : '0'; loadErrors(); };
        el('drc-waived').onchange = function () { clearSelection(); el('drc-first').onclick(); };
        el('drc-in-view').onclick = function () {
            const c = current(); if (!c || c.pending) { info('Wait for the current view edit.'); return; }
            query = {bbox: null, rev: c.state.state_rev}; errorStart = {check: '0', error: '0'}; previousErrors.length = 0; clearSelection(); loadErrors();
        };
        el('drc-frame').onclick = function () { if (selected) { cancelStep(); select(selected, true); } };
        el('drc-step-prev').onclick = function () { step(true, false, true); };
        el('drc-step-next').onclick = function () { step(false, false, true); };
        el('drc-step-continue').onclick = function () { step(false, true, true); };
        el('drc-clear').onclick = clearSelection;
        el('drc-markers').onchange = function () { paintLater(); savePanel(); };
        el('drc-toggle').onclick = function () { shown = !shown; el('drc-panel').hidden = !shown; el('drc-toggle').setAttribute('aria-expanded', String(shown)); o.resize(); savePanel(); };
        el('drc-reload').onclick = restoreState;
        return {init: refresh, contextChanged: contextChanged, paint: paint, click: click, clear: clearSelection,
            key: function (key) {
                if (key === 'Escape') { return endFocus(); }
                if ((key === 'n' || key === 'p') && rule && current()) { step(key === 'p', false, false); return true; }
                return false;
            },
            stop: function () { stopped = true; persistence.close(); bound = ''; cancelAll(); clearTimeout(timer); if (painting !== null) { o.window.cancelAnimationFrame(painting); painting = null; } overlay.hidden = true; },
            resume: function () { stopped = false; return refresh(); }};
    }
    const api = {bind: bind, projection: projection, point: point, shifted: shifted};
    if (typeof module === 'object' && module.exports) { module.exports = api; } else { root.FloeDRC = api; }
}(typeof window === 'object' ? window : this));
