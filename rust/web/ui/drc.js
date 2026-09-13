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
    function vertices(page, format, P) {
        const um = page.points_um !== undefined, dbu = page.points_dbu !== undefined;
        if (um === dbu || (format === 'ascii' && !um) || (format === 'ice' && !dbu)) { throw new Error('Invalid DRC geometry units'); }
        const precision = Number(P.decimal(page.precision)), rows = um ? page.points_um : page.points_dbu;
        if (!(precision > 0) || !Number.isFinite(precision) || !Array.isArray(rows) || !rows.length || rows.length > 2048) { throw new Error('Invalid DRC geometry page'); }
        const values = new Float64Array(rows.length * 2);
        rows.forEach(function (xy, i) {
            if (!Array.isArray(xy) || xy.length !== 2) { throw new Error('Invalid DRC vertex'); }
            xy.forEach(function (s, axis) {
                const n = Number(P.decimal(s)) / (um ? 1 : precision);
                if (!Number.isFinite(n)) { throw new Error('Unrepresentable DRC vertex'); }
                values[i * 2 + axis] = n;
            });
        });
        return {mode: um ? 'um' : 'dbu', precision: precision, points: values};
    }
    // These formatters never interpret rule text as markup or compute a DRC
    // measurement in JS. Exact server scalars remain available in the text.
    function metricName(s) {
        if (typeof s !== 'string' || !s || new TextEncoder().encode(s).length > 64) { throw new Error('Invalid SVRF type'); }
        return s;
    }
    function metadataText(v) {
        if (!v) { return 'No SVRF metadata for this rule.'; }
        const r = v.rule, lines = [], seen = new Set();
        if (r.desc) { lines.push(r.desc); }
        r.constraints.forEach(function (c) {
            if (c.text && !seen.has(c.text)) { seen.add(c.text); lines.push('constraint: ' + c.text); }
            if (c.value === null && c.raw) { lines.push('unresolved bound: ' + c.raw); }
        });
        if (r.layers.length) { lines.push('layers: ' + r.layers.join(', ')); }
        if (r.source_gds.length) { lines.push('GDS: ' + r.source_gds.map(function (p) { return p[0] + '/' + (p[1] === null ? '*' : p[1]); }).join(', ')); }
        if (r.unresolved.length) { lines.push('unresolved layers: ' + r.unresolved.join(', ')); }
        if (v.derivations.length) {
            lines.push('derivation:'); v.derivations.forEach(function (d) { lines.push('  ' + d.name + ' = ' + d.rhs); });
            if (v.derivations_more) { lines.push('  … more derivations'); }
        }
        return lines.length ? lines.join('\n') : 'Matched SVRF rule has no parsed constraints.';
    }
    function comparisonText(v, r, P) {
        if (!v || v.check !== r.check || v.local !== r.local || v.global !== r.global) { throw new Error('SVRF comparison belongs to another error'); }
        const c = v.comparison;
        if (c === null) { return 'No unambiguous measurement for a parsed bound.'; }
        P.counter(c.constraint, true); metricName(c.metric);
        if (!['<','<=','==','>','>=','!='].includes(c.op) || !['um','um2'].includes(c.unit)) { throw new Error('Invalid SVRF comparison'); }
        ['measured','bound','delta'].forEach(function (k) { P.decimal(c[k]); });
        if (c.percent !== null) { P.decimal(c.percent); }
        const unit = c.unit === 'um2' ? 'µm²' : 'µm', signed = function (s) { return Number(s) >= 0 ? '+' + s : s; };
        return 'Global ' + r.global + ' · ' + c.metric + ' · constraint ' + P.next(c.constraint) + '\n' +
            'Measured ' + c.measured + ' ' + unit + ' vs ' + c.op + ' ' + c.bound + ' ' + unit + '\n' +
            'Δ ' + signed(c.delta) + ' ' + unit + (c.percent === null ? '' : ' (' + signed(c.percent) + '%)');
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
        let boxMode = false, boxStart = null, boxEnd = null, pageReady = false;
        let groupRows = [], groupStamp = '';
        let filterTimer = null, filterStamp = '', restoreTurn = 0, hoverText = '';
        let metric = null, typeStart = '0', typeNext = null, typeReady = false, typeBusy = false;
        const previousTypes = [];
        // A canvas click selects without moving. Auto CD belongs to the last
        // accepted focus navigation, not necessarily the selected row.
        let cdTarget = null, cdGlobal = null, cdSegments = null, cdRemaining = 0, cdError = '';
        let restoring = false;
        let isolationNotice = '';
        const groups = o.groups.bind({http: o.http, protocol: P, changed: groupsChanged,
            status: function (s) { el('drc-group-status').textContent = s; }});
        const persistence = o.stateStore.bind({http: o.http, protocol: P,
            setTimeout: function (fn, delay) { return setTimeout(fn, delay); },
            clearTimeout: function (id) { clearTimeout(id); },
            apply: restorePanel, status: function (s) { el('drc-sync-status').textContent = s; }});
        const builds = o.builds ? o.builds.bind({document: doc, protocol: P, http: o.http, context: o.context,
            setTimeout: function (fn, delay) { return setTimeout(fn, delay); },
            clearTimeout: function (id) { clearTimeout(id); }, changed: applyCatalog,
            retire: function () {
                registration = null; contextChanged(); overlay.hidden = true;
                el('drc-summary').textContent = 'Review paused · checking pack build state';
                info('Previous DRC selection and outlines are not active.');
            }}) : null;
        function info(s) { el('drc-message').textContent = s || ''; }
        function cancel(key) { const t = tasks[key]; if (t) { t.cancelled = true; if (t.abort) { t.abort(); } delete tasks[key]; } }
        function cancelAll() { Object.keys(tasks).forEach(cancel); }
        function cancelStep() { cancel('step'); stepBusy = false; stepContinuation = null; el('drc-step-continue').hidden = true; }
        function current() {
            const c = o.context();
            return !stopped && !(builds && builds.suspended()) && registration && registration.phase === 'ready' && c &&
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
        function hasMetadata() { return !!(registration && registration.metadata && registration.metadata.svrf); }
        function renderTypes(list) {
            const select = el('drc-type'); select.textContent = '';
            function option(value, label) { const opt = doc.createElement('option'); opt.value = value; opt.textContent = label; select.appendChild(opt); }
            option('', 'All types');
            if (metric !== null && !list.some(function (t) { return t.metric === metric; })) { option(metric, metric + ' · selected'); }
            list.forEach(function (t) { option(t.metric, t.metric + ' (' + t.checks + ')'); });
            select.value = metric === null ? '' : metric; navigationButtons();
        }
        async function loadTypes(strict) {
            typeReady = false;
            if (!hasMetadata()) {
                typeNext = null; el('drc-type-info').textContent = 'No SVRF metadata registered.'; renderTypes([]); return;
            }
            const c = current(); if (!c) { return; }
            const t = task('types'), start = typeStart; typeBusy = true; navigationButtons();
            try {
                const v = await read('types', t, c, {kind: 'types', start: start, limit: 32}); if (!v) { return; }
                cursor(v.total);
                if (v.available !== true || v.total !== registration.metadata.svrf.type_count || !Array.isArray(v.rows) || v.rows.length > 32) { throw new Error('Invalid SVRF type page'); }
                let end = start; v.rows.forEach(function () { end = P.next(end); });
                if (P.compare(end, v.total) > 0 || v.next === null && end !== v.total ||
                    v.next !== null && (!v.rows.length || cursor(v.next) !== end || P.compare(end, v.total) >= 0)) { throw new Error('Invalid SVRF type cursor'); }
                const seen = new Set(); v.rows.forEach(function (r) { metricName(r.metric); cursor(r.checks); if (seen.has(r.metric)) { throw new Error('Duplicate SVRF type'); } seen.add(r.metric); });
                typeNext = v.next; typeReady = v.total !== '0'; renderTypes(v.rows);
                el('drc-type-info').textContent = v.total === '0' ? 'Metadata has no classified types.' :
                    v.rows.length + '/' + v.total + ' types on this page · ' + registration.metadata.svrf.matched + '/' + registration.metadata.svrf.checks + ' rules matched';
            } catch (e) {
                if (valid('types', t, c)) { typeNext = null; renderTypes([]); el('drc-type-info').textContent = 'Type catalog unavailable · ' + e.message; }
                if (strict) { throw e; }
            } finally { if (valid('types', t, c)) { typeBusy = false; navigationButtons(); } }
        }
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
            const filtered = available && filtersReady(current());
            el('drc-rule-prev').disabled = !available || !previousRules.length;
            el('drc-rule-next').disabled = !available || ruleNext === null;
            el('drc-error-prev').disabled = !filtered || !pageReady || !previousErrors.length;
            el('drc-error-next').disabled = !filtered || !pageReady || errorNext === null;
            ['drc-search', 'drc-search-submit', 'drc-waived', 'drc-in-view', 'drc-first'].forEach(function (id) { el(id).disabled = !available; });
            el('drc-selected-only').disabled = !available || !groups.ready();
            el('drc-first').disabled = !filtered || !rule;
            el('drc-frame').disabled = !available || !selected;
            el('drc-clear').disabled = restoring || !selected;
            el('drc-reload').disabled = !current();
            ['drc-step-prev', 'drc-step-next'].forEach(function (id) { el(id).disabled = !filtered || !rule || stepBusy || !!(query && !query.bbox); });
            el('drc-step-continue').disabled = !filtered || stepBusy || !stepContinuation;
            ['drc-cd-pop', 'drc-cd-clear'].forEach(function (id) { el(id).disabled = !available || !hasCD(); });
            el('drc-box').disabled = !available || !rule || !pageReady || !groups.ready() || !el('drc-markers').checked;
            el('drc-group-clear').disabled = !available || !rule || !groups.ready() || !groups.ids(rule.check).length;
            el('drc-waived').disabled = !available || !groups.ready();
            el('drc-type').disabled = !available || !typeReady || typeBusy;
            el('drc-type-prev').disabled = !available || typeBusy || !previousTypes.length;
            el('drc-type-next').disabled = !available || typeBusy || typeNext === null;
            const c = current(), isolated = !!c && c.state.layers_isolated === true;
            el('drc-restore-layers').disabled = !available || !isolated || !c.connected || c.pending;
            el('drc-layer-status').textContent = (isolated ? 'Layers isolated · Restore returns to the first visibility snapshot. ' : '') + isolationNotice;
        }
        function boxReset(off) {
            boxStart = boxEnd = null; if (off) { boxMode = false; }
            el('drc-box').setAttribute('aria-pressed', String(boxMode));
            el('drc-box-status').textContent = boxMode ? 'Click the first box corner · drag still pans.' : 'e: two corners · Shift adds · Ctrl/Cmd toggles. Current rule and page only.';
            if (o.cursor) { o.cursor(); } paintLater();
        }
        function toggleBox() {
            if (boxMode) { boxReset(true); return true; }
            if (!current() || restoring || !rule || !pageReady || !groups.ready() || !el('drc-markers').checked) {
                info('Box selection needs a ready rule page with markers on.'); return false;
            }
            boxMode = true; boxReset(false); return true;
        }
        function groupsChanged() {
            const count = rule ? groups.ids(rule.check).length : 0;
            el('drc-group-count').textContent = count + ' selected in this rule · ' + groups.total() + '/5000 across all rules';
            markErrors(); navigationButtons(); paintLater(); loadGroupMarkers(); followFilters();
        }
        async function loadGroupMarkers() {
            const c = current(), ci = rule && rule.check, ids = ci === null ? [] : groups.ids(ci);
            const stamp = contextKey(c) + ':' + ci + ':' + ids.join(',');
            if (!c || !groups.ready() || stamp === groupStamp) { return; }
            groupStamp = stamp; const t = task('group-metadata');
            const cache = new Map(); groupRows.concat(rows).forEach(function (r) { if (r.check === ci) { cache.set(r.local, r); } });
            groupRows = groupRows.filter(function (r) { return r.check === ci && groups.contains(ci, r.local); });
            const missing = ids.filter(function (id) { return !cache.has(id); });
            try {
                for (let i = 0; i < missing.length; i += 64) {
                    const chunk = missing.slice(i, i + 64), v = await read('group-metadata', t, c, {kind: 'records', check: ci, errors: chunk});
                    if (!v || groupStamp !== stamp) { return; }
                    const list = validateRows(v.rows);
                    if (list.length !== chunk.length || list.some(function (r, j) { return r.check !== ci || r.local !== chunk[j]; })) { throw new Error('Selected marker metadata mismatch'); }
                    list.forEach(function (r) { cache.set(r.local, r); });
                }
                if (valid('group-metadata', t, c) && groupStamp === stamp) { groupRows = ids.map(function (id) { return cache.get(id); }); paintLater(); }
            } catch (e) {
                if (valid('group-metadata', t, c)) { el('drc-group-status').textContent = 'Selected markers incomplete · ' + e.message + ' · Reload review to retry.'; }
            }
        }
        function groupApply(ids, mode, bounds) {
            const c = current(); if (!c || !c.connected || c.pending || restoring || !rule || !groups.ready()) { info('Wait for the view and selection synchronization.'); return false; }
            const body = {kind: 'apply', check: rule.check, errors: ids, mode: mode};
            if (bounds) { body.bbox_um = bounds.map(String); body.waived = filter(); }
            groups.change(body, bounds ? c.state.state_rev : undefined); return true;
        }
        function groupClear() { return !!rule && groups.ids(rule.check).length > 0 && groupApply([], 'replace'); }
        function unproject(clientX, clientY) {
            const c = current();
            if (!c || !c.connected || c.pending || restoring || !pageReady || !lastProjection || overlay.hidden ||
                !el('drc-markers').checked || hitStamp !== contextKey(c) + ':' + c.state.state_rev ||
                !Number.isFinite(clientX) || !Number.isFinite(clientY)) { return null; }
            const r = overlay.getBoundingClientRect();
            if (!(r.width > 0 && r.height > 0) || clientX < r.left || clientX >= r.right || clientY < r.top || clientY >= r.bottom) { return null; }
            const p = lastProjection, x = (clientX - r.left) / r.width * overlay.width + p.origin[0], y = (clientY - r.top) / r.height * overlay.height + p.origin[1];
            const xy = [(p.bbox[0] + x * p.step[0]) * p.dbu, (p.bbox[3] - y * p.step[1]) * p.dbu];
            return xy.every(Number.isFinite) ? xy : null;
        }
        function boxClick(x, y, modifiers) {
            if (!groups.ready()) { return false; }
            const xy = unproject(x, y); if (!xy) { return false; }
            if (!boxStart) {
                boxStart = boxEnd = xy; el('drc-box-status').textContent = 'Click the opposite corner · Shift adds · Ctrl/Cmd toggles · Esc cancels corner.'; paintLater(); return true;
            }
            const a = boxStart, b = xy, mode = modifiers && (modifiers.ctrlKey || modifiers.metaKey) ? 'toggle' : modifiers && modifiers.shiftKey ? 'add' : 'replace';
            const ids = rows.filter(function (r) { return r.check === rule.check; }).map(function (r) { return r.local; });
            boxReset(false); return groupApply(ids, mode, [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])]);
        }
        function boxMove(x, y) {
            if (!boxMode || !boxStart) { return; }
            const xy = unproject(x, y); if (xy) { boxEnd = xy; paintLater(true); }
        }
        function tooltip(text) {
            if (text === hoverText) { return; }
            hoverText = text; el('viewport').title = text;
        }
        function move(x, y) {
            if (boxMode) { tooltip(''); boxMove(x, y); return; }
            const r = hitRow(x, y);
            tooltip(r ? (rule && rule.check === r.check ? rule.name : 'Rule ' + P.next(r.check)) +
                ' #' + P.next(r.local) + ' (' + r.global + ')' + (r.status === 1 ? ' · waived' : '') : '');
        }
        function paintBox(p, dpr) {
            if (!boxMode || !boxStart || !boxEnd) { return; }
            const a = point(p, boxStart[0], boxStart[1]), b = point(p, boxEnd[0], boxEnd[1]);
            if (!a.concat(b).every(Number.isFinite)) { return; }
            ctx.strokeStyle = '#f4cd64'; ctx.lineWidth = dpr; ctx.setLineDash([5 * dpr, 3 * dpr]);
            ctx.strokeRect(a[0], a[1], b[0] - a[0], b[1] - a[1]); ctx.setLineDash([]);
            // Make the first world-space corner visible even before movement.
            const arm = 4 * dpr; ctx.beginPath(); ctx.moveTo(a[0] - arm, a[1]); ctx.lineTo(a[0] + arm, a[1]);
            ctx.moveTo(a[0], a[1] - arm); ctx.lineTo(a[0], a[1] + arm); ctx.stroke();
        }
        function hasCD() { return !!cdTarget && cdRemaining > 0 && (cdSegments === null || cdSegments.length > 0); }
        function showCD() {
            const title = el('drc-cd-title'), list = el('drc-cd-values'); list.textContent = '';
            if (!cdTarget) { title.textContent = 'Go to an error to measure it.'; }
            else if (!cdRemaining) { title.textContent = 'CD rulers cleared.'; }
            else if (cdError) { title.textContent = 'CD unavailable · ' + cdError; }
            else if (cdSegments === null) { title.textContent = 'Reading CD measurements…'; }
            else {
                title.textContent = 'CD · jumped global ' + cdGlobal + (cdSegments.length ? '' : ' · no supported measurement');
                cdSegments.slice(0, cdRemaining).forEach(function (s) {
                    const li = doc.createElement('li'); li.textContent = s.label; li.title = s.role + ': ' + s.distance + ' µm'; list.appendChild(li);
                });
            }
            navigationButtons(); paintLater();
        }
        function resetCD() { cancel('cd'); cdTarget = null; cdGlobal = null; cdSegments = null; cdRemaining = 0; cdError = ''; showCD(); }
        async function loadCD() {
            const c = current(); if (!c || !cdTarget || !cdRemaining) { return; }
            const ref = cdTarget, t = task('cd');
            try {
                const v = await read('cd', t, c, {kind: 'measurements', check: ref.check, error: ref.error});
                if (!v || cdTarget !== ref || !cdRemaining) { return; }
                const decoded = o.rulers.decode(v, ref, P); cdGlobal = decoded.global; cdSegments = decoded.segments; cdError = ''; showCD();
            } catch (e) {
                if (valid('cd', t, c) && cdTarget === ref) { cdSegments = []; cdError = e.message || String(e); showCD(); }
            }
        }
        function jumpCD(r) {
            const same = cdTarget && cdTarget.check === r.check && cdTarget.error === r.local;
            if (!same || cdSegments === null || cdError) {
                resetCD(); cdTarget = {check: r.check, error: r.local}; cdGlobal = r.global; cdRemaining = 3; loadCD();
            } else { cdRemaining = 3; }
            showCD();
        }
        function popCD(all) {
            if (!hasCD() || restoring) { return false; }
            // Ruler dismissal also cancels a pending jump so a late response
            // cannot create a replacement after Escape/k. Jump mode remains.
            cancelStep(); cancel('focus'); cancel('cd');
            cdRemaining = all || cdSegments === null ? 0 : Math.max(0, Math.min(cdRemaining, cdSegments.length) - 1);
            showCD(); savePanel(); return true;
        }
        function escape() {
            if (builds && builds.escape()) { return true; }
            if (boxMode) { boxReset(!boxStart); return true; }
            return popCD(true) || groupClear() || restoreLayers() || endFocus();
        }
        function restoreLayers() {
            let c = current();
            if (!c || c.state.layers_isolated !== true || restoring) { return false; }
            // Escape cancels the pending jump even if an already-sent edit
            // means restoration itself must wait for a later explicit input.
            cancelStep(); cancel('focus');
            c = current();
            if (!c.connected || c.pending) { info('Wait for the view to reconnect or finish its current edit before restoring layers.'); return true; }
            const t = task('restore-layers');
            t.abort = o.restoreLayers(function (error) {
                if (!valid('restore-layers', t, c)) { return; }
                t.abort = null;
                if (error) { info(error); navigationButtons(); return; }
                isolationNotice = ''; resetCD(); endFocus(); // Keep the selected cursor for click-mode n/p.
                info('Layer visibility restored · error focus cleared.'); navigationButtons(); savePanel();
            });
            navigationButtons(); return true;
        }
        function paintLater(keepHits) {
            if (!keepHits) { markerHits = []; hitStamp = ''; tooltip(''); }
            if (stopped || painting !== null) { return; }
            painting = o.window.requestAnimationFrame(function () { painting = null; paint(lastProjection, lastSize); });
        }
        function marker(r, xy, side, w, h) {
            if (!xy.every(Number.isFinite)) { return; }
            const x = Math.round(xy[0]), y = Math.round(xy[1]), half = Math.floor(side / 2);
            if (x - half >= w || x + half < 0 || y - half >= h || y + half < 0) { return; }
            ctx.fillStyle = groups.contains(r.check, r.local) ? '#f4cd64' : r.status === 1 ? '#70da9a' : '#ff6969'; ctx.fillRect(x - half, y - half, side, side);
            markerHits.push({x: x, y: y, row: r});
        }
        function paint(p, size) {
            markerHits = []; hitStamp = ''; lastProjection = p; lastSize = size;
            const c = current();
            if (!ctx || !p || !size || !c || !el('drc-markers').checked || (!boxMode && !groupRows.length && !rows.length && (!selected || !focusVisible) && !hasCD())) { overlay.hidden = true; return; }
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
            const pageIds = new Set(rows.map(function (r) { return r.check + ':' + r.local; }));
            groupRows.forEach(function (r) {
                if (!rule || r.check !== rule.check || !groups.contains(r.check, r.local) || pageIds.has(r.check + ':' + r.local) ||
                    (filter() !== null && (r.status === 1) !== filter()) || (focusVisible && selected && selected.check === r.check && selected.local === r.local)) { return; }
                const b = bbox(r.bbox_um); marker(r, point(p, b[0] * .5 + b[2] * .5, b[1] * .5 + b[3] * .5), 7, w, h);
            });
            paintSelected(p, w, h);
            if (cdSegments && cdRemaining) { o.rulers.paint(ctx, cdSegments.slice(0, cdRemaining), function (x, y) { return point(p, x, y); }, size); }
            paintBox(p, size.dpr);
        }
        function paintSelected(p, w, h) {
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
        function hitRow(clientX, clientY) {
            const c = current();
            if (!c || !c.connected || c.pending || restoring || overlay.hidden || !el('drc-markers').checked ||
                hitStamp !== contextKey(c) + ':' + c.state.state_rev || !markerHits.length ||
                !Number.isFinite(clientX) || !Number.isFinite(clientY)) { return null; }
            // Use the *painted* overlay's DOM rectangle, including fractional
            // CSS origins/DPR/margin translations. Do not unproject through a
            // newer requested viewport or issue an all-error spatial scan.
            const rect = overlay.getBoundingClientRect();
            if (!(rect.width > 0 && rect.height > 0) || clientX < rect.left || clientX >= rect.right || clientY < rect.top || clientY >= rect.bottom) { return null; }
            let best = null, distance = 36 + 1;
            markerHits.forEach(function (hit) {
                const dx = (hit.x / overlay.width * rect.width + rect.left) - clientX;
                const dy = (hit.y / overlay.height * rect.height + rect.top) - clientY, d = dx * dx + dy * dy;
                if (d <= 36 && d < distance) { best = hit.row; distance = d; }
            });
            return best;
        }
        function click(clientX, clientY, twice, modifiers) {
            if (boxMode) { return boxClick(clientX, clientY, modifiers); }
            const best = hitRow(clientX, clientY); if (!best) { return false; }
            // Like GTK's canvas marker pick, a single click only selects,
            // even in jump mode. A double click explicitly requests focus.
            cancelStep(); rowFocus = false; select(best, !!twice, !!twice); return true;
        }
        function clearSelection() {
            cancel('restore-layers');
            cancelStep(); jumpActive = focusVisible = rowFocus = false;
            resetCD();
            cancel('geometry'); cancel('focus'); selected = null; points = null; pointsReady = false;
            cancel('comparison'); el('drc-comparison').textContent = 'Select an error to compare its parsed bound.';
            jumpScale = null; zoomLock = false; el('drc-selected').textContent = 'Click to inspect · double-click to go to an error.';
            paintLater(); navigationButtons(); renderErrors(); savePanel();
        }
        function endFocus() {
            const hadWork = stepBusy || stepContinuation || (selected && (focusVisible || jumpActive));
            if (!hadWork) { return false; }
            cancelStep(); cancel('focus'); cancel('restore-layers'); cancel('geometry'); points = null; pointsReady = false;
            resetCD();
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
            ruleRows = []; ruleNext = null; renderRules();
            try {
                if (new TextEncoder().encode(search).length > 256) { throw new Error('Rule search is limited to 256 UTF-8 bytes.'); }
                const page = await read('rules', t, c, {kind: 'rules', start: ruleStart, search: search, metric: metric, waived: filter(), limit: 32}); if (!page) { return; }
                if (!Array.isArray(page.rows) || page.rows.length > 32) { throw new Error('DRC rule page limit'); }
                page.rows.forEach(function (r) { cursor(r.check); cursor(r.errors); cursor(r.waived); if (typeof r.name !== 'string') { throw new Error('Invalid rule name'); } });
                ruleRows = page.rows; ruleNext = page.next === null ? null : cursor(page.next); renderRules();
                if (autoSelect !== false && !rule && ruleRows.length) { chooseRule(ruleRows[0]); }
                savePanel();
            } catch (e) { if (strict) { throw e; } failure('rules', t, c, e); }
        }
        function chooseRule(r) {
            if (!current()) { return; }
            boxReset(false); rule = r; query = null; errorStart = '0'; previousErrors.length = 0; rows = []; errorNext = null;
            clearSelection(); renderRules(); el('drc-rule-title').textContent = r.name; el('drc-description').textContent = ''; el('drc-rule-metadata').textContent = '';
            const c = current(), t = task('description');
            read('description', t, c, {kind: 'rule', check: r.check}).then(function (page) {
                if (page) { showDescription(page); }
            }).catch(function (e) { failure('description', t, c, e); });
            loadErrors(); savePanel(); groupsChanged();
        }
        function showDescription(page) {
            el('drc-description').textContent = page.description;
            el('drc-rule-metadata').textContent = hasMetadata() ? metadataText(page.svrf) : 'No SVRF metadata registered.';
        }
        async function loadComparison(r) {
            cancel('comparison');
            if (!hasMetadata()) { el('drc-comparison').textContent = 'No SVRF metadata registered.'; return; }
            const c = current(), t = task('comparison'); el('drc-comparison').textContent = 'Reading constraint comparison…';
            try {
                const v = await read('comparison', t, c, {kind: 'comparison', check: r.check, error: r.local});
                if (v && selected && selected.check === r.check && selected.local === r.local) { el('drc-comparison').textContent = comparisonText(v, r, P); }
            } catch (e) { if (valid('comparison', t, c)) { el('drc-comparison').textContent = 'Comparison unavailable · ' + e.message; } }
        }
        function filter() { return el('drc-waived').value === 'all' ? null : el('drc-waived').value === 'waived'; }
        function inView() { return !query && el('drc-in-view').checked; }
        function selectedOnly() { return !query && el('drc-selected-only').checked; }
        function filtersReady(c) {
            return !!c && (!inView() || c.connected && !c.pending) && (!selectedOnly() || groups.ready());
        }
        function filterKey(c) {
            return JSON.stringify([contextKey(c), rule && rule.check, filter(), inView(), selectedOnly(),
                inView() && c ? c.state.state_rev : null, selectedOnly() ? groups.revision() : null]);
        }
        function resetFilterPage() {
            clearTimeout(filterTimer); filterTimer = null; cancel('errors'); cancelStep();
            boxReset(false); pageReady = false; rows = []; errorNext = null; errorStart = '0'; previousErrors.length = 0;
            renderErrors();
        }
        function followFilters() {
            const c = current();
            if (!c || restoring || !rule || query || (!inView() && !selectedOnly())) { return; }
            if (!filtersReady(c)) {
                if (filterStamp !== 'waiting') { resetFilterPage(); filterStamp = 'waiting'; info('Waiting for the current view/selection before filtering…'); }
                return;
            }
            const stamp = filterKey(c);
            if (filterStamp === stamp) { return; }
            resetFilterPage(); filterStamp = stamp; info('Updating current-rule filters…');
            // Coalesce a burst of accepted pan/zoom or selection changes. Only
            // the first bounded page is read; never auto-drain the whole query.
            filterTimer = setTimeout(function () { filterTimer = null; if (current() && filterKey(current()) === stamp) { loadErrors(); } }, 100);
        }
        function validateFilters(page, body) {
            if (page.selection_rev !== body.selection_rev) { throw new Error('DRC selection revision mismatch'); }
            if (body.selection_rev !== null && (!groups.ready() || groups.revision() !== body.selection_rev)) { throw new Error('DRC selection changed; reload the filtered page.'); }
            if (body.in_view) { P.bbox(page.bbox_um); } else if (page.bbox_um !== null) { throw new Error('Unexpected DRC viewport filter'); }
            cursor(page.scanned); if (P.compare(page.scanned, '262144') > 0) { throw new Error('DRC scan limit'); }
        }
        function matchesFilters(r, body, bounds) {
            if (r.check !== body.check || body.waived !== null && (r.status === 1) !== body.waived ||
                body.selection_rev !== null && !groups.contains(r.check, r.local)) { return false; }
            if (!body.in_view) { return true; }
            const b = bbox(r.bbox_um), q = bbox(bounds);
            return b[0] <= q[2] && b[2] >= q[0] && b[1] <= q[3] && b[3] >= q[1];
        }
        function markErrors() {
            Array.prototype.forEach.call(el('drc-errors').children, function (b, i) {
                const r = rows[i], active = r && selected && selected.check === r.check && selected.local === r.local;
                b.className = 'drc-error' + (r && r.status === 1 ? ' waived' : '') + (r && groups.contains(r.check, r.local) ? ' grouped' : '') + (active ? ' selected' : '');
                if (active && rowFocus) { rowFocus = false; b.focus(); b.scrollIntoView({block: 'nearest'}); }
            });
        }
        function renderErrors() {
            if (selected && doc.activeElement && el('drc-errors').contains(doc.activeElement)) { rowFocus = true; }
            el('drc-errors').textContent = '';
            rows.forEach(function (r) {
                const b = doc.createElement('button');
                b.disabled = restoring || !pageReady;
                b.textContent = '#' + P.next(r.local) + '  ·  global ' + r.global + '  ·  ' + (r.kind === 'p' ? 'poly' : 'edge') + (r.status === 1 ? '  ·  waived' : '');
                b.setAttribute('aria-label', 'Error ' + P.next(r.local) + ', global ' + r.global);
                b.onclick = function (e) {
                    if (e && e.altKey) { return; }
                    if (e && (e.ctrlKey || e.metaKey || e.shiftKey)) { if (!(e.detail > 1) && rule && r.check === rule.check) { groupApply([r.local], e.ctrlKey || e.metaKey ? 'toggle' : 'add'); } return; }
                    cancelStep(); select(r, false);
                };
                b.ondblclick = function (e) { if (e && (e.ctrlKey || e.shiftKey || e.metaKey || e.altKey)) { return; } cancelStep(); select(r, true); };
                b.onkeydown = function (e) {
                    if (e.ctrlKey || e.metaKey || e.altKey || e.isComposing) { return; }
                    if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'n' || e.key === 'p') {
                        e.preventDefault(); step(e.key === 'ArrowUp' || e.key === 'p', false, true);
                    } else if (e.key === 'Escape' && escape()) { e.preventDefault(); }
                    else if ((e.key === 'k' || e.key === 'K') && popCD(e.key === 'K')) { e.preventDefault(); }
                };
                el('drc-errors').appendChild(b);
            });
            if (!rows.length) { el('drc-errors').textContent = !pageReady ? 'Waiting for an error page…' : errorNext === null ? 'No matching errors.' : 'No match in this scan. Continue to the next page.'; }
            el('drc-result-info').textContent = (query ? 'Saved viewport query' : 'Rule errors' + (selectedOnly() ? ' ∩ Selected' : '') + (inView() ? ' ∩ In view' : '')) +
                ' · ' + rows.length + ' on this page' + (!pageReady ? ' · waiting for a page' : errorNext === null ? ' · end' : ' · more available');
            markErrors(); navigationButtons(); paintLater();
        }
        async function loadErrors(strict) {
            const c = current(); if (!c || (!rule && !query)) { return; }
            clearTimeout(filterTimer); filterTimer = null;
            if (!filtersReady(c)) {
                if (strict && selectedOnly() && !groups.ready()) { throw new Error('Selection is not ready; Reload review to retry.'); }
                // A normal reload races WebSocket attach/initial resize. The
                // saved panel is valid; defer its live list, not its restoration.
                pageReady = false; rows = []; filterStamp = 'waiting'; renderErrors();
                info('Waiting for the current view before filtering…');
                followFilters(); return;
            }
            const stamp = filterKey(c); filterStamp = stamp;
            pageReady = false; rows = []; boxReset(false); renderErrors();
            const t = task('errors'), body = query ? (query.bbox ? {kind: 'query', bbox_um: query.bbox, checks: null, cursor: errorStart, waived: filter(), limit: 64} :
                {kind: 'in_view', cursor: errorStart, waived: filter(), limit: 64}) : {kind: 'list', check: rule.check, start: errorStart, waived: filter(), limit: 64,
                    in_view: inView(), selection_rev: selectedOnly() ? groups.revision() : null};
            try {
                const page = await read('errors', t, c, body, body.kind === 'in_view' || body.in_view); if (!page || filterKey(current()) !== stamp) { return; }
                if (body.kind === 'in_view') { P.bbox(page.bbox_um); query.bbox = page.bbox_um; }
                const list = validateRows(page.rows);
                if (body.kind === 'list') {
                    validateFilters(page, body);
                    list.forEach(function (r, i) {
                        if (!matchesFilters(r, body, page.bbox_um) || P.compare(r.local, body.start) < 0 || i && P.compare(list[i - 1].local, r.local) >= 0) { throw new Error('DRC list filter/order mismatch'); }
                    });
                    if (page.next !== null && (P.compare(cursor(page.next), body.start) <= 0 || list.length && P.compare(page.next, list[list.length - 1].local) <= 0)) { throw new Error('DRC list cursor did not progress'); }
                }
                rows = list; errorNext = page.next;
                if (errorNext !== null) { if (query) { cursor(errorNext.check); cursor(errorNext.error); } else { cursor(errorNext); } }
                if (JSON.stringify(errorNext) === JSON.stringify(errorStart)) { throw new Error('DRC cursor did not progress'); }
                pageReady = true; renderErrors(); info('Read-only · review files are never changed.'); savePanel();
            } catch (e) {
                const now = current();
                if (body.in_view && valid('errors', t, c) && now && (!now.connected || now.pending || now.state.state_rev !== c.state.state_rev)) {
                    filterStamp = 'waiting'; followFilters(); return;
                }
                if (strict) { throw e; } failure('errors', t, c, e);
            }
            finally { if (valid('errors', t, c)) { navigationButtons(); } }
        }
        async function geometry(r) {
            const c = current(), t = task('geometry'); let start = '0', total = null, units = null;
            try {
                for (let pageNo = 0; pageNo < 128; ++pageNo) {
                    const page = await read('geometry', t, c, {kind: 'geometry', check: r.check, error: r.local, start: start, limit: 2048}); if (!page) { return; }
                    cursor(page.total); cursor(page.start);
                    const n = Number(page.total), v = vertices(page, registration.metadata.format, P);
                    if (page.check !== r.check || page.local !== r.local || page.global !== r.global || page.kind !== r.kind || page.start !== start || n < 1 || n > 262144) { throw new Error('Invalid DRC geometry page'); }
                    const stamp = v.mode + ':' + v.precision;
                    if (total === null) { total = n; units = stamp; points = new Float64Array(n * 2); } else if (n !== total || units !== stamp) { throw new Error('DRC geometry changed'); }
                    const offset = Number(start), end = offset + v.points.length / 2;
                    if (end > total) { throw new Error('DRC geometry bounds'); }
                    points.set(v.points, offset * 2);
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
                const page = await read('focus', t, c, {kind: 'focus', check: r.check, error: r.local, fit: !zoomLock, isolate: true}, true); if (!page) { return; }
                if (!current().connected || current().state.connection_epoch !== s.connection_epoch || current().pending) { info('View input or connection changed; select the error again to move.'); return; }
                const isolation = page.layer_isolation, statuses = ['ready', 'no_metadata', 'no_rule', 'no_source_layers', 'no_match', 'unsupported_deck'];
                if (page.check !== r.check || page.local !== r.local || !/^[0-9a-f]{64}$/.test(page.prepared_token || '') ||
                    !isolation || !statuses.includes(isolation.status)) { throw new Error('Invalid prepared move'); }
                cursor(isolation.matched);
                if ((isolation.status === 'ready') !== (isolation.matched !== '0')) { throw new Error('Invalid isolated layer count'); }
                const nextScale = Number(P.decimal(page.navigation.width_um)) / s.pixels[0];
                if (!(nextScale > 0) || !Number.isFinite(nextScale)) { throw new Error('Invalid prepared scale'); }
                // The client never sends metadata layer pairs or recomputes
                // geometry. Server approval commits layers + navigation once.
                t.abort = o.navigate(page.navigation, page.prepared_token, function (error) {
                    if (!valid('focus', t, c)) { return; }
                    t.abort = null;
                    if (error) { info(error); return; }
                    if (inView()) {
                        el('drc-in-view').checked = false; resetFilterPage(); filterStamp = '';
                        errorStart = r.local; loadErrors();
                    }
                    jumpScale = nextScale; jumpCD(r);
                    isolationNotice = isolation.status === 'ready' ? 'Last jump: ' + isolation.matched + ' layer pairs isolated.' :
                        isolation.status === 'unsupported_deck' ? 'Layers unchanged · physical GDS isolation is unavailable in jobdeck views.' :
                        'Layers unchanged · this rule has no matching source-layer metadata.';
                    info('Read-only · review files are never changed.');
                    navigationButtons(); savePanel();
                });
            } catch (e) { failure('focus', t, c, e); }
        }
        function syncRule(check) {
            if (rule && rule.check === check) { return; }
            resetCD();
            rule = {check: check, name: 'Rule ' + P.next(check)};
            boxReset(false); groupsChanged();
            renderRules(); el('drc-rule-title').textContent = rule.name; el('drc-description').textContent = ''; el('drc-rule-metadata').textContent = '';
            const c = current(), t = task('description');
            read('description', t, c, {kind: 'rule', check: check}).then(function (v) {
                if (!v || !rule || rule.check !== check) { return; }
                cursor(v.errors); cursor(v.waived); rule = {check: check, name: v.name, errors: v.errors, waived: v.waived};
                el('drc-rule-title').textContent = v.name; showDescription(v); renderRules();
            }).catch(function (e) { failure('description', t, c, e); });
        }
        function select(r, frame, allowFocus) {
            if (!current()) { return; }
            cancel('focus'); cancel('restore-layers'); isolationNotice = ''; syncRule(r.check);
            const same = selected && selected.check === r.check && selected.local === r.local;
            if (!selected) { jumpScale = null; zoomLock = false; }
            selected = r; focusVisible = true; if (frame) { jumpActive = true; }
            if (!same) { loadComparison(r); }
            if (!same || !pointsReady) { points = null; pointsReady = false; el('drc-selected').textContent = 'Global ' + r.global + ' · bounding-box preview'; geometry(r); }
            // Keep the button node alive between click and double-click.
            markErrors(); navigationButtons(); paintLater();
            if (jumpActive && allowFocus !== false) { focus(r, frame); }
            savePanel();
        }
        async function step(backwards, resume, focusRow) {
            const c = current(); if (!c || restoring || !rule || stepBusy || !filtersReady(c) || (query && !query.bbox)) { return; }
            const stamp = filterKey(c);
            let body;
            if (resume) { if (!stepContinuation) { return; } body = stepContinuation; }
            else {
                stepContinuation = null; el('drc-step-continue').hidden = true;
                let after = selected && selected.check === rule.check && (filter() === null || (selected.status === 1) === filter()) ? selected.local : null;
                if (after !== null && query) {
                    const a = bbox(selected.bbox_um), b = bbox(query.bbox);
                    if (a[0] > b[2] || a[2] < b[0] || a[1] > b[3] || a[3] < b[1]) { after = null; }
                }
                body = query ? {kind: 'step', check: rule.check, backwards: backwards, after: after, cursor: null, waived: filter(), bbox_um: query.bbox} :
                    {kind: 'filtered_step', check: rule.check, backwards: backwards, after: after, cursor: null, waived: filter(),
                        in_view: inView(), selection_rev: selectedOnly() ? groups.revision() : null};
            }
            cancel('focus'); const t = task('step'); stepBusy = true; rowFocus = false;
            navigationButtons(); info('Searching the current rule…');
            try {
                const page = await read('step', t, c, body, body.in_view); if (!page || filterKey(current()) !== stamp) { return; }
                if (body.kind === 'filtered_step') { validateFilters(page, body); }
                cursor(page.scanned);
                if (P.compare(page.scanned, '262144') > 0 || page.hit === undefined || page.next === undefined || (page.hit !== null && page.next !== null)) { throw new Error('Invalid DRC step response'); }
                if (page.next !== null) {
                    if (body.selection_rev) { throw new Error('Unexpected selected-step continuation'); }
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
                if (body.kind === 'filtered_step' && !matchesFilters(r, body, page.bbox_um)) { throw new Error('DRC step filter mismatch'); }
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
            return {search: el('drc-search').value.trim(), metric: metric, rule_start: ruleStart, check: rule ? rule.check : null,
                error_start: query ? '0' : errorStart, query: query ? {bbox_um: query.bbox, state_rev: query.rev, cursor: errorStart} : null,
                waived: filter(), selected: selected ? {check: selected.check, error: selected.local} : null,
                in_view: inView(), selected_only: selectedOnly(),
                markers: el('drc-markers').checked, shown: shown, jump_scale: jumpScale === null ? null : String(jumpScale), zoom_lock: zoomLock,
                jump_active: jumpActive, focus_visible: focusVisible,
                cd: cdTarget ? {target: cdTarget, remaining: cdRemaining} : null};
        }
        function savePanel() {
            if (restoring || !current()) { return; }
            const data = panelData(); if (data) { persistence.change(data); }
        }
        async function restorePanel(data) {
            const c = current(); if (!c) { return; }
            const t = task('restore'); restoring = true;
            cancel('types'); cancel('rules'); cancel('errors'); cancel('description'); clearSelection(); boxReset(true);
            metric = null; typeStart = '0'; typeNext = null; typeBusy = typeReady = false; previousTypes.length = 0; renderTypes([]);
            rule = null; rows = []; query = null; previousRules.length = previousErrors.length = 0;
            filterStamp = ''; clearTimeout(filterTimer); filterTimer = null;
            el('drc-in-view').checked = el('drc-selected-only').checked = false;
            ruleStart = errorStart = '0'; ruleNext = errorNext = null;
            navigationButtons();
            try {
                if (!data) {
                    // A new registry identity has no saved panel. Do not seed
                    // it with the retired review's search/status/zoom filters.
                    el('drc-search').value = ''; el('drc-waived').value = 'all'; el('drc-markers').checked = true;
                    jumpScale = null; zoomLock = false;
                    await loadTypes(true); if (valid('restore', t, c)) { await loadRules(undefined, true); } return;
                }
                if (typeof data.search !== 'string' || new TextEncoder().encode(data.search).length > 256 ||
                    (data.waived !== null && typeof data.waived !== 'boolean') ||
                    !['markers','shown','zoom_lock','jump_active','focus_visible'].every(function (k) { return typeof data[k] === 'boolean'; }) ||
                    (data.jump_active && !data.focus_visible) || ((data.jump_active || data.focus_visible) && !data.selected)) { throw new Error('Invalid saved review state'); }
                ruleStart = cursor(data.rule_start); errorStart = cursor(data.error_start);
                metric = data.metric === undefined || data.metric === null ? null : metricName(data.metric);
                if (metric !== null && !hasMetadata()) { throw new Error('Saved type filter needs the registered SVRF metadata.'); }
                await loadTypes(true); if (!valid('restore', t, c)) { return; }
                ['in_view', 'selected_only'].forEach(function (k) { if (data[k] !== undefined && typeof data[k] !== 'boolean') { throw new Error('Invalid saved filter'); } });
                if (data.query && (data.in_view || data.selected_only)) { throw new Error('Saved snapshot and live filters conflict'); }
                el('drc-in-view').checked = data.in_view === true; el('drc-selected-only').checked = data.selected_only === true;
                el('drc-search').value = data.search; el('drc-waived').value = data.waived === null ? 'all' : data.waived ? 'waived' : 'unwaived';
                el('drc-markers').checked = data.markers;
                if (shown !== data.shown) { shown = data.shown; el('drc-panel').hidden = !shown; el('drc-toggle').setAttribute('aria-expanded', String(shown)); o.resize(); }
                jumpScale = data.jump_scale === null ? null : Number(P.decimal(data.jump_scale)); zoomLock = data.zoom_lock;
                jumpActive = data.jump_active; focusVisible = data.focus_visible;
                if (data.cd !== null) {
                    const cd = data.cd;
                    if (!cd || !cd.target || !jumpActive || !Number.isInteger(cd.remaining) || cd.remaining < 0 || cd.remaining > 3) { throw new Error('Invalid saved CD state'); }
                    cdTarget = {check: cursor(cd.target.check), error: cursor(cd.target.error)}; cdRemaining = cd.remaining;
                }
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
                    el('drc-rule-title').textContent = page.name; showDescription(page);
                } else { el('drc-rule-title').textContent = 'Choose a rule'; el('drc-description').textContent = ''; el('drc-rule-metadata').textContent = ''; }
                await loadErrors(true); if (!valid('restore', t, c)) { return; }
                if (data.selected) {
                    const ref = data.selected; cursor(ref.check); cursor(ref.error);
                    const page = await read('restore', t, c, {kind: 'geometry', check: ref.check, error: ref.error, start: '0', limit: 1}); if (!page) { return; }
                    selected = validateRows([page])[0]; points = null; pointsReady = false;
                    loadComparison(selected);
                    el('drc-selected').textContent = 'Global ' + selected.global + (focusVisible ? ' · bounding-box preview' : ' · focus cleared; n/p continues without moving the view.');
                    if (focusVisible) { geometry(selected); }
                }
                if (cdTarget && cdRemaining) { loadCD(); } showCD();
            } finally {
                if (valid('restore', t, c)) { restoring = false; renderRules(); renderErrors(); contextChanged(); }
            }
        }
        function restoreState() {
            const c = current(); if (!c) { return; }
            // Cancel immediately, not after the panel GET completes. A slow
            // previous focus/step must not move the view during restoration.
            cancelStep(); ['focus', 'restore-layers', 'geometry', 'cd', 'types', 'comparison', 'rules', 'errors', 'description', 'restore', 'group-metadata'].forEach(cancel);
            boxReset(true); groupRows = []; groupStamp = '';
            clearTimeout(filterTimer); filterTimer = null; filterStamp = '';
            const key = contextKey(c), turn = ++restoreTurn; restoring = true; navigationButtons(); renderRules(); renderErrors();
            const path = '/api/v1/drc/' + registration.id + '/views/' + c.id;
            // Selected=true must not restore before its server group snapshot.
            // Guard this await too: a superseded reload cannot start a new GET.
            return groups.attach({path: path + '/selection', revision: registration.revision, view: c.id}).then(function () {
                if (turn !== restoreTurn || contextKey(current()) !== key) { return; }
                return persistence.attach({path: path + '/panel', revision: registration.revision, view: c.id});
            }).then(function () {
                if (turn === restoreTurn && contextKey(current()) === key) { restoring = false; renderRules(); renderErrors(); contextChanged(); groupsChanged(); }
            });
        }
        function contextChanged() {
            if (builds) { builds.contextChanged(); }
            markerHits = []; hitStamp = ''; tooltip('');
            const c = current(), key = contextKey(c);
            if (c && !c.connected) { cancel('focus'); cancel('restore-layers'); }
            if (key !== bound) {
                ++restoreTurn; clearTimeout(filterTimer); filterTimer = null; filterStamp = '';
                persistence.close(); groups.close(); cancelAll(); boxReset(true); groupRows = []; groupStamp = ''; pageReady = false;
                bound = key; restoring = false; isolationNotice = ''; rule = null; ruleRows = []; rows = []; ruleStart = errorStart = '0'; query = null;
                metric = null; typeStart = '0'; typeNext = null; typeReady = typeBusy = false; previousTypes.length = 0; renderTypes([]);
                previousRules.length = previousErrors.length = 0; ruleNext = errorNext = null; clearSelection(); renderRules();
                el('drc-rule-title').textContent = 'Choose a rule'; el('drc-description').textContent = ''; el('drc-rule-metadata').textContent = ''; el('drc-type-info').textContent = '';
                if (c) { restoreState(); }
            }
            groupsChanged();
            if (registration && !c && registration.phase === 'ready') { info('Open the source associated with this DRC database.'); }
            if (query && c && query.rev !== c.state.state_rev) { el('drc-result-info').textContent = 'Saved earlier-viewport query · enable In view for the live current-rule filter.'; }
        }
        function applyCatalog(v) {
            const before = registration, present = !!(v.drc || v.build);
            registration = v.drc; el('drc-toggle').hidden = !present; el('drc-panel').hidden = !present || !shown;
            if (registration) {
                el('drc-title').textContent = registration.title;
                el('drc-summary').textContent = registration.metadata ? registration.metadata.checks + ' rules · ' + registration.metadata.errors + ' errors' : registration.phase;
                if (registration.metadata) {
                    const m = registration.metadata;
                    if (m.format) { el('drc-summary').textContent += ' · ' + (m.format === 'ascii' ? 'ASCII' : 'ICE'); }
                    if (m.truncated_records !== undefined && cursor(m.truncated_records) !== '0') { el('drc-summary').textContent += ' · ' + m.truncated_records + ' truncated records'; }
                }
                if (!before || before.id !== registration.id || before.phase !== registration.phase) {
                    info(registration.error || (registration.phase === 'opening' ? 'Opening DRC metadata…' : 'Read-only review'));
                }
            }
            // The catalog is polled while idle too. Rebinding/restarting an
            // unfinished geometry read every poll would starve large outlines.
            if (!before || !registration || before.id !== registration.id || before.phase !== registration.phase || contextKey(current()) !== bound) {
                contextChanged();
            }
        }
        async function refresh() {
            if (stopped) { return; }
            if (builds) { return builds.refresh(); }
            const t = task('catalog');
            try {
                const v = await o.http('GET', '/api/v1/drc', undefined, false, t); if (t.cancelled || stopped) { return; }
                applyCatalog(v);
                if (registration && registration.phase === 'opening') { timer = setTimeout(refresh, 500); }
            } catch (e) { if (!t.cancelled) { info(e.message); } }
        }
        el('drc-search-form').onsubmit = function (e) { e.preventDefault(); ruleStart = '0'; previousRules.length = 0; loadRules(); };
        el('drc-rule-prev').onclick = function () { if (previousRules.length) { ruleStart = previousRules.pop(); loadRules(); } };
        el('drc-rule-next').onclick = function () { if (ruleNext !== null) { remember(previousRules, ruleStart); ruleStart = ruleNext; loadRules(); } };
        el('drc-type').onchange = function () {
            if (!current() || restoring || !typeReady || typeBusy) { return; }
            metric = el('drc-type').value || null; ruleStart = '0'; previousRules.length = 0; loadRules(false); savePanel();
        };
        el('drc-type-prev').onclick = function () { if (!typeBusy && previousTypes.length) { typeStart = previousTypes.pop(); loadTypes(); } };
        el('drc-type-next').onclick = function () { if (!typeBusy && typeNext !== null) { remember(previousTypes, typeStart); typeStart = typeNext; loadTypes(); } };
        el('drc-error-prev').onclick = function () { cancelStep(); if (previousErrors.length) { errorStart = previousErrors.pop(); loadErrors(); } };
        el('drc-error-next').onclick = function () { cancelStep(); if (errorNext !== null) { remember(previousErrors, errorStart); errorStart = errorNext; loadErrors(); } };
        el('drc-first').onclick = function () { cancelStep(); previousErrors.length = 0; errorStart = query ? {check: '0', error: '0'} : '0'; loadErrors(); };
        el('drc-waived').onchange = function () { boxReset(false); if (groups.total()) { groups.change({kind: 'clear_all'}); } clearSelection();
            ruleStart = '0'; previousRules.length = 0; loadRules(false); el('drc-first').onclick(); };
        el('drc-in-view').onchange = el('drc-selected-only').onchange = function () {
            if (!current() || restoring) { return; }
            query = null; clearSelection(); resetFilterPage(); filterStamp = ''; loadErrors(); savePanel();
        };
        el('drc-frame').onclick = function () { if (selected) { cancelStep(); select(selected, true); } };
        el('drc-restore-layers').onclick = restoreLayers;
        el('drc-step-prev').onclick = function () { step(true, false, true); };
        el('drc-step-next').onclick = function () { step(false, false, true); };
        el('drc-step-continue').onclick = function () { step(false, true, true); };
        el('drc-clear').onclick = clearSelection;
        el('drc-cd-pop').onclick = function () { popCD(false); };
        el('drc-cd-clear').onclick = function () { popCD(true); };
        el('drc-markers').onchange = function () { if (!el('drc-markers').checked) { boxReset(true); } paintLater(); navigationButtons(); savePanel(); };
        el('drc-box').onclick = toggleBox;
        el('drc-group-clear').onclick = groupClear;
        el('drc-toggle').onclick = function () { shown = !shown; el('drc-panel').hidden = !shown; el('drc-toggle').setAttribute('aria-expanded', String(shown)); o.resize(); savePanel(); };
        el('drc-reload').onclick = restoreState;
        return {init: refresh, refresh: refresh, contextChanged: contextChanged, paint: paint, click: click, clear: clearSelection,
            boxActive: function () { return boxMode; }, move: move,
            key: function (key) {
                if (key === 'Escape') { return escape(); }
                if (key === 'k' || key === 'K') { return popCD(key === 'K'); }
                if (key === 'e') { return toggleBox(); }
                if ((key === 'n' || key === 'p') && rule && current()) { step(key === 'p', false, false); return true; }
                return false;
            },
            stop: function () { stopped = true; if (builds) { builds.stop(); } ++restoreTurn; clearTimeout(filterTimer); filterTimer = null; persistence.close(); groups.close(); bound = ''; boxReset(true); cancelAll(); clearTimeout(timer); if (painting !== null) { o.window.cancelAnimationFrame(painting); painting = null; } overlay.hidden = true; },
            resume: function () { stopped = false; return builds ? builds.resume() : refresh(); }};
    }
    const api = {bind: bind, projection: projection, point: point, shifted: shifted, vertices: vertices, metadataText: metadataText, comparisonText: comparisonText};
    if (typeof module === 'object' && module.exports) { module.exports = api; } else { root.FloeDRC = api; }
}(typeof window === 'object' ? window : this));
