/* In-session selection commands. Never retry a toggle after an ambiguous reply. */
(function (root) {
    'use strict';
    function decode(value, scope, P) {
        if (!value || value.revision !== scope.revision || value.view_id !== scope.view) { throw new Error('Selection context changed'); }
        const s = value.state, rules = new Map();
        if (!s || s.limit !== 5000 || !Array.isArray(s.rules) || s.rules.length > 5000) { throw new Error('Invalid selection state'); }
        P.counter(s.selection_rev); P.counter(s.total, true);
        let total = 0, previous = null;
        s.rules.forEach(function (r) {
            P.counter(r.check, true);
            if (previous !== null && P.compare(r.check, previous) <= 0 || !Array.isArray(r.errors) || !r.errors.length || r.errors.length > 5000 - total) { throw new Error('Invalid selection rules'); }
            let last = null;
            r.errors.forEach(function (id) {
                P.counter(id, true);
                if (last !== null && P.compare(id, last) <= 0) { throw new Error('Invalid selection members'); } last = id;
            });
            total += r.errors.length; rules.set(r.check, new Set(r.errors)); previous = r.check;
        });
        if (String(total) !== s.total) { throw new Error('Selection count mismatch'); }
        return {revision: s.selection_rev, total: total, rules: rules};
    }
    function bind(o) {
        let scope = null, state = null, request = null, ready = false, busy = false;
        function changed() { o.changed(); }
        function active(t, s) { return request === t && !t.cancelled && scope === s; }
        function close() {
            if (request) { request.cancelled = true; if (request.abort) { request.abort(); } }
            request = null; scope = null; state = null; ready = busy = false;
        }
        async function load(t, s) {
            const v = await o.http('GET', s.path, undefined, false, t);
            if (!active(t, s)) { return false; }
            state = decode(v, s, o.protocol); ready = true; return true;
        }
        async function attach(s) {
            close(); scope = s; const t = {cancelled: false, abort: null}; request = t;
            busy = true; changed(); o.status('Reading selection groups…');
            try { if (await load(t, s)) { o.status('Selections live in this open view; files are unchanged.'); } }
            catch (e) { if (active(t, s)) { o.status('Selection unavailable · ' + e.message + ' · Reload review to retry.'); } }
            finally { if (active(t, s)) { request = null; busy = false; changed(); } }
        }
        async function change(body, viewRevision) {
            if (!scope || !ready || busy) { o.status('Wait for selection synchronization, or Reload review.'); return false; }
            const s = scope, base = state.revision, t = {cancelled: false, abort: null}; request = t;
            busy = true; changed(); o.status('Updating selection…');
            const payload = {revision: s.revision, base_selection_rev: base, body: body};
            if (viewRevision !== undefined) { payload.state_rev = viewRevision; }
            try {
                const v = await o.http('POST', s.path, payload, false, t);
                if (!active(t, s)) { return false; }
                const next = decode(v, s, o.protocol);
                if (next.revision !== o.protocol.next(base)) { throw new Error('Unexpected selection revision'); }
                state = next; o.status('Selection updated · files unchanged.'); return true;
            } catch (e) {
                if (!active(t, s)) { return false; }
                ready = false;
                // The server may have committed before the response was lost.
                // Reconcile once with GET; never replay replace/add/toggle.
                try {
                    if (await load(t, s)) { o.status('Selection reply failed · ' + e.message + ' · server state reloaded; command not retried.'); }
                } catch (readError) {
                    if (active(t, s)) { o.status('Selection uncertain · Reload review before editing: ' + readError.message); }
                }
                return false;
            } finally { if (active(t, s)) { request = null; busy = false; changed(); } }
        }
        return {attach: attach, change: change, close: close,
            ready: function () { return ready && !busy; },
            contains: function (ci, id) { return !!state && state.rules.has(ci) && state.rules.get(ci).has(id); },
            ids: function (ci) { return state && state.rules.has(ci) ? Array.from(state.rules.get(ci)) : []; },
            total: function () { return state ? state.total : 0; }};
    }
    const api = {bind: bind, decode: decode};
    if (typeof module === 'object' && module.exports) { module.exports = api; } else { root.FloeDRCGroups = api; }
}(typeof window === 'object' ? window : this));
