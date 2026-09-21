/* ES2017. Opt-in, numeric-only callback timings, NOT input-to-photon. */
(function (root) {
    'use strict';
    const LIMIT = 256, PENDING = 128;
    function create(clock) {
        let enabled = false, origin = 0, rows = [], head = 0, overwritten = 0, untracked = 0;
        const pending = new Set(), previewAt = new Map();
        function now() {
            if (!enabled || typeof clock !== 'function') { return null; }
            const n = clock();
            return Number.isFinite(n) && n >= origin ? n : null;
        }
        function duration(a, b) {
            return a !== null && b !== null && b >= a ? Math.round((b - a) * 1000) / 1000 : null;
        }
        function append(row) {
            if (rows.length < LIMIT) { rows.push(row); }
            else { rows[head] = row; head = (head + 1) % LIMIT; ++overwritten; }
        }
        function clear() { rows = []; head = overwritten = untracked = 0; pending.clear(); previewAt.clear(); }
        function track(kind, data) {
            const at = now();
            if (at === null) { return null; }
            if (pending.size >= PENDING) { ++untracked; return null; }
            const ticket = {kind: kind, start: at, sent: null, ack: null, decoded: null, data: data};
            pending.add(ticket); return ticket;
        }
        function stamp(t, field) { if (pending.has(t) && t[field] === null) { t[field] = now(); } }
        function end(t, outcome) {
            if (!pending.delete(t)) { return; }
            const at = now();
            if (at === null) { ++untracked; return; }
            const row = Object.assign({kind: t.kind, outcome: outcome, at_ms: duration(origin, at), elapsed_ms: duration(t.start, at)}, t.data);
            if (t.kind === 'edit') {
                Object.assign(row, {queue_ms: duration(t.start, t.sent), send_to_ack_ms: duration(t.sent, t.ack),
                    ack_to_snapshot_ms: outcome === 'accepted' ? duration(t.ack, at) : null,
                    queue_to_snapshot_ms: outcome === 'accepted' ? duration(t.start, at) : null});
            } else {
                Object.assign(row, {packet_ms: duration(t.received, t.decode_start), decode_ms: duration(t.decode_start, t.decoded),
                    submit_ms: outcome === 'submitted' ? duration(t.decoded, at) : null,
                    receive_to_submit_ms: outcome === 'submitted' ? duration(t.received, at) : null});
            }
            append(row);
        }
        function interrupt() { Array.from(pending).forEach(function (t) { end(t, 'interrupted'); }); previewAt.clear(); }
        function enable(value) {
            if (!value) { interrupt(); enabled = false; return false; }
            if (enabled) { return true; }
            const at = typeof clock === 'function' ? clock() : NaN;
            if (!Number.isFinite(at) || at < 0) { return false; }
            clear(); origin = at; enabled = true; return true;
        }
        function beginEdit(action) {
            if (!enabled) { return null; }
            return track('edit', {action: ['pan', 'zoom', 'goto', 'fit', 'band'].includes(action) ? action : 'display'});
        }
        function beginFrame(h, received) {
            if (!enabled) { return null; }
            // Never retain the header: it contains view IDs/world coordinates.
            const t = track('frame', {purpose: h.purpose === 'margin' ? 'margin' : 'foreground',
                format: h.format === 'raw' ? 'raw' : 'png', final: h.final === true});
            if (t) {
                t.decode_start = t.start;
                t.received = Number.isFinite(received) && received >= origin && received <= t.start ? received : null;
                if (t.received !== null) { t.start = t.received; }
            }
            return t;
        }
        function preview(start, kind, trigger) {
            const at = now();
            if (start === null || !Number.isFinite(start) || at === null || start < origin || start > at) { return; }
            kind = kind === 'band' ? 'band' : 'pan';
            append({kind: 'preview', action: kind, trigger: trigger === 'raf' ? 'raf' : 'release',
                at_ms: duration(origin, at), event_to_submit_ms: duration(start, at),
                submit_interval_ms: duration(previewAt.has(kind) ? previewAt.get(kind) : null, at)});
            previewAt.set(kind, at);
        }
        function snapshot() {
            const events = rows.slice(head).concat(rows.slice(0, head)).map(function (r) { return Object.assign({}, r); });
            return {format: 'floe.browser-timing', version: 1, boundary: 'JavaScript callbacks, not paint/photon or GTK parity',
                enabled: enabled, limit: LIMIT, overwritten: overwritten, untracked: untracked, pending: pending.size, events: events};
        }
        return Object.freeze({enable: enable, clear: clear, now: now, beginEdit: beginEdit, beginFrame: beginFrame,
            sent: function (t) { stamp(t, 'sent'); }, ack: function (t) { stamp(t, 'ack'); },
            decoded: function (t) { stamp(t, 'decoded'); },
            endEdit: function (t, ok) { end(t, ok ? 'accepted' : 'interrupted'); },
            endFrame: function (t, displayed) { end(t, displayed ? 'submitted' : 'discarded'); },
            interrupt: interrupt, preview: preview, endGesture: function () { previewAt.clear(); }, snapshot: snapshot});
    }
    function bind(o) {
        const toggle = o.el('timing-enabled'), report = o.el('timing-report');
        const clock = o.window.performance && o.window.performance.now;
        const core = create(typeof clock === 'function' ? function () { return clock.call(o.window.performance); } : null);
        function refresh() { report.textContent = JSON.stringify(core.snapshot(), null, 2); }
        toggle.checked = false; toggle.disabled = typeof clock !== 'function';
        toggle.addEventListener('change', function () { toggle.checked = core.enable(toggle.checked); refresh(); });
        o.el('timing-refresh').onclick = refresh;
        o.el('timing-clear').onclick = function () { core.clear(); refresh(); };
        return Object.freeze(Object.assign({}, core, {stop: function () { core.enable(false); toggle.checked = false; refresh(); },
            clear: function () { core.clear(); refresh(); }}));
    }
    const api = {create: create, bind: bind};
    if (typeof module !== 'undefined' && module.exports) { module.exports = api; }
    else { root.FloeDisplayTiming = api; }
}(typeof window === 'undefined' ? this : window));
