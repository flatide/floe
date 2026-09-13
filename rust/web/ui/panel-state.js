/* ES2017. One active save + one latest pending state, scoped to an open view.
 * Network ambiguity is reconciled by an explicit reload, never blind replay. */
(function (root) {
    'use strict';
    function encode(body) {
        // serde_json maps and JS objects need not share insertion order.
        return JSON.stringify(body, function (_, v) {
            if (!v || typeof v !== 'object' || Array.isArray(v)) { return v; }
            const out = {}; Object.keys(v).sort().forEach(function (k) { out[k] = v[k]; }); return out;
        });
    }
    function bind(o) {
        let scope = null, serial = 0, revision = '1', last = '', ready = false;
        let active = null, pending = null, timer = null;
        function cancel() {
            if (active) { active.cancelled = true; if (active.abort) { active.abort(); } active = null; }
            o.clearTimeout(timer); timer = null; pending = null;
        }
        function close() { ++serial; ready = false; scope = null; cancel(); }
        function valid(n, token) { return n === serial && scope && !token.cancelled; }
        function checked(v) {
            if (!v || v.revision !== scope.revision || v.view_id !== scope.view || !v.state ||
                !Object.prototype.hasOwnProperty.call(v.state, 'body') ||
                (v.state.body !== null && (typeof v.state.body !== 'object' || Array.isArray(v.state.body)))) { throw new Error('DRC panel context changed'); }
            o.protocol.counter(v.state.panel_rev); return v.state;
        }
        function status(s) { o.status(s); }
        async function attach(next) {
            close(); scope = next; const n = serial, token = {cancelled: false}; active = token;
            status('Restoring review state…');
            try {
                const response = await o.http('GET', scope.path, undefined, false, token);
                if (!valid(n, token)) { return; }
                const state = checked(response); revision = state.panel_rev; last = encode(state.body);
                await o.apply(state.body);
                if (!valid(n, token)) { return; }
                ready = true; status('Review state synchronized · server session only');
            } catch (e) {
                if (valid(n, token)) { ready = false; status('Review state unavailable: ' + e.message + ' · use Reload review.'); }
            } finally { if (valid(n, token)) { active = null; } }
        }
        function schedule() {
            o.clearTimeout(timer); timer = o.setTimeout(function () { timer = null; flush(); }, 120);
        }
        function change(body) {
            if (!scope || !ready) { return; }
            // Snapshot the state, not mutable UI arrays/cursor objects.
            const text = encode(body);
            if (new TextEncoder().encode(text).length > 4096) { status('Review state exceeds its limit; not saved.'); return; }
            pending = text; status('Saving review state…'); schedule();
        }
        async function flush() {
            if (!scope || !ready || active || pending === null) { return; }
            const text = pending; pending = null;
            if (text === last) { status('Review state synchronized · server session only'); return; }
            const n = serial, token = {cancelled: false}; active = token;
            try {
                const response = await o.http('POST', scope.path,
                    {revision: scope.revision, base_panel_rev: revision, body: JSON.parse(text)}, false, token);
                if (!valid(n, token)) { return; }
                const state = checked(response);
                if (o.protocol.compare(state.panel_rev, revision) < 0 || encode(state.body) !== text) { throw new Error('DRC panel state differs from the saved request'); }
                revision = state.panel_rev; last = encode(state.body);
                if (pending === null || pending === last) { pending = null; status('Review state synchronized · server session only'); }
            } catch (e) {
                if (valid(n, token)) {
                    ready = false; pending = null;
                    status('Review state save unconfirmed: ' + e.message + ' · use Reload review. Local changes were not replayed.');
                }
            } finally {
                if (valid(n, token)) { active = null; if (ready && pending !== null) { schedule(); } }
            }
        }
        return {attach: attach, change: change, close: close, ready: function () { return ready; }};
    }
    const api = {bind: bind};
    if (typeof module === 'object' && module.exports) { module.exports = api; } else { root.FloePanelState = api; }
}(typeof window === 'object' ? window : this));
