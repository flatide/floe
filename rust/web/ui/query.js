/* ES2017 display-pinned owner queries. No geometry scans or world-coordinate
 * requests in the browser; only current viewport fractions cross the wire. */
(function (root) {
    'use strict';
    const FIELDS = ['dataset_revision', 'worker_epoch', 'frame_id', 'state_rev', 'render_rev', 'render_key'];
    const messages = {
        stale_frame: 'The displayed view changed. Select again.',
        frame_not_displayed: 'The frame is not query-ready yet. Select again.',
        scene_unavailable: 'No query geometry is published for this view.',
        scene_mismatch: 'The renderer scene changed. Select again.',
        scene_incomplete: 'Geometry is incomplete; no query result is available.',
        scene_summary: 'Visible layers include occupancy summaries. Zoom in or select exact layers.',
        superseded: 'A newer query replaced this request.',
        query_failed: 'The native query failed; no empty result was assumed.',
        unsupported: 'Geometry queries are not supported in this view.',
        invalid_request: 'The query position, radius or layers are unsupported.',
        incomplete: 'Geometry is incomplete; no query result is available.'
    };
    function keys(v, names) {
        if (!v || typeof v !== 'object' || Array.isArray(v) || Object.keys(v).length !== names.length ||
            !names.every(function (k) { return Object.prototype.hasOwnProperty.call(v, k); })) { throw new Error('Invalid query response'); }
    }
    function i64(s, P) {
        if (typeof s !== 'string' || !/^(0|-?[1-9][0-9]*)$/.test(s)) { throw new Error('Invalid geometry coordinate'); }
        const negative = s[0] === '-', n = negative ? s.slice(1) : s;
        if (P.compare(n, negative ? '9223372036854775808' : '9223372036854775807') > 0) { throw new Error('Geometry coordinate limit'); }
        return s;
    }
    function cmp(a, b, P) {
        const na = a[0] === '-', nb = b[0] === '-';
        return na !== nb ? (na ? -1 : 1) : P.compare(na ? a.slice(1) : a, nb ? b.slice(1) : b) * (na ? -1 : 1);
    }
    function scene(v, P) {
        keys(v, ['generation', 'round', 'complete', 'summary_layers']);
        if (typeof v.complete !== 'boolean' || (v.generation === null) !== (v.round === null)) { throw new Error('Invalid query scene'); }
        P.counter(v.summary_layers, true);
        if (v.generation !== null) { P.counter(v.generation); P.counter(v.round); }
        if (v.complete && v.generation === null) { throw new Error('Missing query scene'); }
        return v;
    }
    function anchor(c, P) {
        const a = {};
        FIELDS.forEach(function (k) { a[k] = P.counter(k === 'frame_id' ? c.frame.frame_id : c.state[k]); });
        return a;
    }
    function same(a, b) { return FIELDS.every(function (k) { return a[k] === b[k]; }); }
    function scope(c, P) {
        if (!c || !c.connected || c.pending || c.hidden || !c.state.capabilities.query || !c.frame || !c.acked ||
            c.id !== c.state.view_id || !Number.isFinite(c.size.dpr) || c.size.dpr <= 0 ||
            ![c.size.left, c.size.top, c.rect.left, c.rect.top].every(Number.isFinite) ||
            ['closed', 'failed', 'opening'].includes(c.state.status) || !c.frame.query || !P.matches(c.frame, c.state) ||
            c.size.pixels[0] !== c.state.pixels[0] || c.size.pixels[1] !== c.state.pixels[1]) { return null; }
        const s = scene(c.frame.query_scene, P);
        if (!s.complete || s.generation === null) { return null; }
        const p = c.origin;
        if (!p || p[0] < 0 || p[1] < 0 || p[0] + c.size.pixels[0] > c.frame.width || p[1] + c.size.pixels[1] > c.frame.height) { return null; }
        const a = anchor(c, P);
        return {anchor: a, key: JSON.stringify([c.id, c.state.connection_epoch, a, c.size.pixels,
            c.size.dpr, c.size.left, c.size.top, c.rect.left, c.rect.top, c.origin])};
    }
    function position(c, x, y) {
        const s = c.size;
        const p = [((x - c.rect.left) - s.left) * s.dpr / s.pixels[0],
            ((y - c.rect.top) - s.top) * s.dpr / s.pixels[1]];
        return p.every(function (n) { return Number.isFinite(n) && n >= 0 && n <= 1; }) ? p : null;
    }
    function hit(v, kind, P) {
        if (v === null) { return null; }
        if (kind === 'snap') {
            keys(v, ['kind', 'point_dbu', 'snap']);
            if (v.kind !== kind || !['vertex', 'edge'].includes(v.snap) || !Array.isArray(v.point_dbu) || v.point_dbu.length !== 2) { throw new Error('Invalid snap result'); }
            v.point_dbu.forEach(function (s) { i64(s, P); }); return v;
        }
        keys(v, ['kind', 'count', 'index', 'pair', 'layer_name', 'cell_name', 'area_dbu2', 'bbox_dbu', 'points_dbu', 'points_truncated']);
        if (v.kind !== kind || !P.pair(v.pair) || typeof v.points_truncated !== 'boolean') { throw new Error('Invalid picked shape'); }
        P.counter(v.count); P.counter(v.index, true);
        if (P.compare(v.count, '64') > 0 || P.compare(v.index, v.count) >= 0) { throw new Error('Invalid pick cycle'); }
        ['layer_name', 'cell_name'].forEach(function (k) {
            if (typeof v[k] !== 'string' || new TextEncoder().encode(v[k]).length > 65536) { throw new Error('Shape name limit'); }
        });
        if (Number(P.decimal(v.area_dbu2)) < 0 || !Array.isArray(v.bbox_dbu) || v.bbox_dbu.length !== 4) { throw new Error('Invalid shape area or bounds'); }
        v.bbox_dbu.forEach(function (s) { i64(s, P); });
        if (cmp(v.bbox_dbu[0], v.bbox_dbu[2], P) > 0 || cmp(v.bbox_dbu[1], v.bbox_dbu[3], P) > 0 ||
            !Array.isArray(v.points_dbu) || v.points_dbu.length > 512) { throw new Error('Invalid shape outline'); }
        v.points_dbu.forEach(function (p) {
            if (!Array.isArray(p) || p.length !== 2) { throw new Error('Invalid shape point'); }
            p.forEach(function (s) { i64(s, P); });
            if (cmp(p[0], v.bbox_dbu[0], P) < 0 || cmp(p[0], v.bbox_dbu[2], P) > 0 ||
                cmp(p[1], v.bbox_dbu[1], P) < 0 || cmp(p[1], v.bbox_dbu[3], P) > 0) { throw new Error('Point outside picked bounds'); }
        });
        return v;
    }
    function result(m, t, P) {
        if (t.id === null || m.view_id !== t.view || m.connection_epoch !== t.epoch || m.query_id !== t.id || !m.anchor || !same(m.anchor, t.anchor)) { throw new Error('Query identity mismatch'); }
        keys(m.anchor, FIELDS); FIELDS.forEach(function (k) { P.counter(m.anchor[k]); });
        const native = m.scene !== undefined;
        keys(m, ['type', 'seq', 'view_id', 'connection_epoch', 'query_id', 'anchor', 'status', 'hit'].concat(native ? ['scene', 'requested_summary_layers'] : []));
        if (m.status !== 'ok' && !Object.prototype.hasOwnProperty.call(messages, m.status)) { throw new Error('Invalid query status'); }
        if (native) {
            const s = scene(m.scene, P); P.counter(m.requested_summary_layers, true);
            if (P.compare(m.requested_summary_layers, s.summary_layers) > 0 ||
                (m.status === 'ok' && (!s.complete || m.requested_summary_layers !== '0'))) { throw new Error('Incomplete query result'); }
        } else if (!['stale_frame', 'superseded'].includes(m.status)) { throw new Error('Missing query metadata'); }
        if (m.status !== 'ok' && m.hit !== null) { throw new Error('Geometry in refused query'); }
        return {status: m.status, hit: hit(m.hit, t.kind, P), message: m.status === 'ok' ? '' : messages[m.status]};
    }
    function bind(o) {
        const P = o.protocol, slots = {pick: null, snap: null}, written = {pick: null, snap: null};
        let timer = null, last = -Infinity, stopped = false;
        function current() { try { return scope(o.context(), P); } catch (e) { return null; } }
        function retire(kind, wire) {
            const t = slots[kind]; slots[kind] = null;
            if (t && t.timeout !== null) { o.clearTimeout(t.timeout); }
            const w = written[kind];
            if (wire) { written[kind] = null; }
            if (wire && w) {
                const c = o.context();
                if (c && c.connected && c.id === w.view && c.state.connection_epoch === w.epoch) {
                    try { o.send({type: 'view.query.cancel', view_id: w.view, connection_epoch: w.epoch, kind: kind}); } catch (e) { /* local retirement still wins */ }
                }
            }
        }
        function settled(t) { written[t.kind] = null; retire(t.kind, false); }
        function cancel(kind) { retire(kind, true); if (!slots.pick && !slots.snap && timer !== null) { o.clearTimeout(timer); timer = null; } }
        function changed() {
            const c = current();
            ['pick', 'snap'].forEach(function (k) { if (slots[k] && (!c || slots[k].key !== c.key)) { cancel(k); } });
            return c;
        }
        function pump() {
            timer = null;
            if (stopped) { return; }
            const c = changed(); if (!c) { return; }
            const t = [slots.pick, slots.snap].find(function (v) { return v && !v.seq; });
            if (!t) { return; }
            const wait = 80 - (o.now() - last);
            if (wait > 0) { timer = o.setTimeout(pump, wait); return; }
            try {
                t.seq = o.send({type: 'view.query', view_id: t.view, connection_epoch: t.epoch,
                    body: {anchor: t.anchor, operation: t.kind === 'snap' ? {kind: 'snap'} : {kind: 'pick', nth: t.nth},
                        position: t.position, radius_px: t.radius, layers: {mode: 'all'}}});
                written[t.kind] = {view: t.view, epoch: t.epoch};
                last = o.now();
                t.timeout = o.setTimeout(function () {
                    if (slots[t.kind] !== t) { return; }
                    cancel(t.kind); t.done({status: 'timeout', hit: null, message: 'Query timed out. Select again; it was not replayed.'});
                }, 8000);
            } catch (e) { cancel(t.kind); t.done({status: 'error', hit: null, message: 'Query could not be sent. Check the connection.'}); }
            if ([slots.pick, slots.snap].some(function (v) { return v && !v.seq; })) { timer = o.setTimeout(pump, 80); }
        }
        function request(kind, p, radius, nth, done) {
            const c = o.context(), s = changed();
            if (stopped || !s || !['pick', 'snap'].includes(kind) || !Array.isArray(p) || p.length !== 2 || !p.every(function (n) { return Number.isFinite(n) && n >= 0 && n <= 1; }) ||
                !Number.isFinite(radius) || radius < 0 || radius > 64) { return false; }
            if (kind === 'pick') { i64(nth, P); }
            retire(kind, false); // Replacing input never queues all intermediate hover points.
            slots[kind] = {kind: kind, view: c.id, epoch: c.state.connection_epoch, anchor: s.anchor, key: s.key,
                position: p.slice(), radius: radius, nth: nth, done: done, seq: null, id: null, timeout: null};
            if (timer === null) { pump(); } return true;
        }
        function receive(m) {
            if (!m || !['query.accepted', 'query.result', 'query.cancelled', 'error'].includes(m.type)) { return false; }
            const t = [slots.pick, slots.snap].find(function (v) { return v && v.seq === m.seq; });
            if (!t) { return m.type !== 'error'; }
            const c = changed(); if (!c || slots[t.kind] !== t) { return true; }
            try {
                if (m.type === 'query.accepted') {
                    keys(m, ['type', 'seq', 'view_id', 'connection_epoch', 'query_id']);
                    if (t.id !== null || m.view_id !== t.view || m.connection_epoch !== t.epoch) { throw new Error('Invalid query acknowledgement'); }
                    t.id = P.counter(m.query_id); return true;
                }
                if (m.type === 'query.result') {
                    const v = result(m, t, P); settled(t); t.done(v); return true;
                }
                // A refused replacement may leave the previous native ticket
                // alive on this connection; explicitly retire that kind too.
                if (m.type === 'error') {
                    keys(m, ['type', 'seq', 'code']);
                    const message = typeof m.code === 'string' && Object.prototype.hasOwnProperty.call(messages, m.code) ? messages[m.code] : 'Query was refused.';
                    cancel(t.kind); t.done({status: 'error', hit: null, message: message}); return true;
                }
                throw new Error('Unexpected query cancellation');
            } catch (e) { cancel(t.kind); t.done({status: 'invalid', hit: null, message: 'Invalid query response; no geometry was accepted.'}); return true; }
        }
        return {request: request, receive: receive, changed: changed, cancel: cancel,
            stop: function () { stopped = true; cancel('pick'); cancel('snap'); }, resume: function () { stopped = false; }};
    }
    const api = {bind: bind, scope: scope, position: position, scene: scene, hit: hit, i64: i64};
    if (typeof module === 'object' && module.exports) { module.exports = api; } else { root.FloeQuery = api; }
}(typeof window === 'object' ? window : this));
