/* ES2017 owner-only pack build. The server owns the job, identity replacement
 * and cancellation/commit race. Disconnecting this controller never cancels it. */
(function (root) {
    'use strict';
    const phases = {
        queued: 'Queued', closing_review: 'Closing the previous review',
        preparing: 'Preparing pack', running: 'Building pack', validating: 'Validating pack',
        cancelling: 'Cancelling build', opening_review: 'Registering the new review',
        succeeded: 'Pack ready', failed: 'Build failed', cancelled: 'Build cancelled'
    };
    const errors = {
        busy: 'Not enough free worker slots, or a reader/index job holds the file. No automatic retry.',
        index_unavailable: 'Existing pack needs replacement approval. Check “Replace existing pack” to rebuild.',
        invalid_request: 'The input, output or options cannot be used to build this pack.',
        unsupported: 'This source cannot be encoded in the current pack format.',
        worker_failed: 'The native indexer failed. The previous pack was preserved.',
        io_error: 'A source, staging or pack file could not be accessed.',
        drc_build_unavailable: 'Pack building is unavailable for this registration.',
        drc_context_changed: 'The registered review or layout changed. Review the new state before approving.',
        operation_sequence: 'Another request used this sequence. No new build was submitted.',
        operation_conflict: 'Another request used this sequence. Check the current review before a new approval.',
        operation_expired: 'This request is outside the retained history. Check the current review before a new approval.'
    };
    function terminal(op) { return !!op && ['succeeded', 'failed', 'cancelled'].includes(op.phase); }
    function validateOperation(op, P) {
        if (!op || op.kind !== 'drc_build' || !Object.prototype.hasOwnProperty.call(phases, op.phase)) { throw new Error('Invalid pack build state'); }
        P.counter(op.seq);
        if (op.elapsed_ms !== undefined) { P.counter(op.elapsed_ms, true); }
        ['noninteger', 'cleanup_warning'].forEach(function (k) {
            if (op[k] !== undefined && typeof op[k] !== 'boolean') { throw new Error('Invalid pack build flag'); }
        });
        if (op.error !== undefined && op.error !== null && (typeof op.error !== 'string' || op.error.length > 128)) { throw new Error('Invalid pack build error'); }
        if (op.native) {
            ['checks', 'total_checks', 'errors', 'output_bytes', 'dropped_lines'].forEach(function (k) {
                if (op.native[k] !== null) { P.counter(op.native[k], true); }
            });
        }
        if (op.migration !== undefined && op.migration !== null &&
            (typeof op.migration !== 'object' || Object.keys(op.migration).join(',') !== 'directory_synced' || typeof op.migration.directory_synced !== 'boolean')) {
            throw new Error('Invalid pack migration receipt');
        }
        if (op.outcome) {
            ['checks', 'errors', 'bytes'].forEach(function (k) { P.counter(op.outcome[k], true); });
            if (typeof op.outcome.reused !== 'boolean' || typeof op.outcome.directory_synced !== 'boolean') { throw new Error('Invalid pack build outcome'); }
        }
        return op;
    }
    function migrationText(m) {
        if (!m) { return ''; }
        return '\nLegacy pack renamed; later failure or cancellation does not undo that name change.' +
            (!m.directory_synced ? '\nRename committed, but directory sync failed.' : '');
    }
    function validate(v, P) {
        if (!v || v.drc === undefined) { throw new Error('Invalid DRC catalog'); }
        const b = v.build;
        if (!b) { return v; }
        if (typeof b.available !== 'boolean' || typeof b.source_id !== 'string' ||
            b.jobs_min !== 1 || b.jobs_max !== 16 || b.jobs_default !== 4) { throw new Error('Invalid pack build capability'); }
        const a = b.operations;
        if (!a || !Array.isArray(a.history) || a.history.length > 32) { throw new Error('Invalid pack build history'); }
        P.counter(a.last_seq, true); if (a.active !== null) { P.counter(a.active); }
        let prev = '0';
        a.history.forEach(function (op) {
            validateOperation(op, P);
            if (P.compare(op.seq, prev) <= 0 || P.compare(op.seq, a.last_seq) > 0 ||
                (!terminal(op) && op.seq !== a.active)) { throw new Error('Invalid pack build ordering'); }
            prev = op.seq;
        });
        if ((a.last_seq !== '0' && prev !== a.last_seq) || (a.active !== null &&
            (a.active !== prev || terminal(a.history[a.history.length - 1])))) { throw new Error('Invalid active pack build'); }
        return v;
    }
    function status(op) {
        if (!op) { return 'No pack build requested.'; }
        let text = op.outcome && op.phase === 'succeeded' && op.outcome.reused ? 'Current pack reused' : phases[op.phase];
        if (op.elapsed_ms !== undefined) { text += ' · ' + (Number(op.elapsed_ms) / 1000).toFixed(1) + ' s'; }
        if (op.outcome) { text += ' · ' + op.outcome.checks + ' rules · ' + op.outcome.errors + ' errors · ' + op.outcome.bytes + ' bytes'; }
        else if (op.native) {
            const n = op.native;
            if (n.checks !== null) { text += ' · ' + n.checks + (n.total_checks === null ? '' : '/' + n.total_checks) + ' checks reported'; }
            if (n.errors !== null) { text += ' · ' + n.errors + ' errors reported'; }
        }
        if (op.error) { text += '\n' + (errors[op.error] || op.error); }
        if (op.noninteger) { text += '\nFractional coordinates cannot be encoded. Continue with the ASCII review.'; }
        if (op.cleanup_warning) { text += '\nTemporary-file cleanup needs attention in local service diagnostics.'; }
        if (op.outcome && !op.outcome.directory_synced) { text += '\nPack published, but directory sync failed; durability is not confirmed.'; }
        text += migrationText(op.migration);
        return text;
    }
    function bind(o) {
        const P = o.protocol, el = function (id) { return o.document.getElementById(id); };
        let catalog = null, stopped = false, timer = null, getTask = null, writeTask = null, cancelTask = null;
        let confirmation = '', waiting = false, pending = null, uncertain = false, stale = true, note = '', pollError = '';
        function abort(t) { if (t) { t.cancelled = true; if (t.abort) { t.abort(); } } }
        function contextKey() {
            const c = o.context(), d = catalog && catalog.drc, b = catalog && catalog.build;
            return !stopped && !stale && c && c.connected && !c.pending && d && b && b.available &&
                b.source_id === c.source && d.source_id === c.source && !['failed', 'closed'].includes(c.state.status) ?
                [c.id, c.source, c.state.connection_epoch, d.id, d.revision].join(':') : '';
        }
        function operations() { return catalog && catalog.build && catalog.build.operations; }
        function active() { const a = operations(); return a && a.active; }
        function last() { const a = operations(); return a && a.history[a.history.length - 1]; }
        function suspend() { return stopped || stale || !!pending || uncertain || !!active(); }
        function closeConfirmation(focus) {
            if (!confirmation) { return false; }
            confirmation = ''; el('drc-build-form').hidden = true;
            el('drc-build-open').setAttribute('aria-expanded', 'false');
            if (focus) { el('drc-build-open').focus(); } return true;
        }
        function render() {
            if (confirmation && confirmation !== contextKey()) { closeConfirmation(false); note = 'Review or connection changed. Open the approval again.'; }
            const b = catalog && catalog.build, op = last();
            el('drc-build').hidden = !b;
            el('drc-build-open').hidden = !b || !b.available;
            el('drc-build-open').disabled = !contextKey() || waiting || uncertain || !!active();
            el('drc-build-submit').disabled = !confirmation || waiting || uncertain || !!active();
            el('drc-build-cancel').hidden = !active();
            el('drc-build-cancel').disabled = stopped || stale || !!cancelTask || op && ['cancelling', 'opening_review'].includes(op.phase);
            el('drc-build-resolve').hidden = !uncertain;
            el('drc-build-resolve').disabled = stopped || waiting;
            el('drc-build-status').textContent = status(op);
            el('drc-build-note').textContent = pollError || note || (b && !b.available ? 'This registration is read-only; no pack build is available.' :
                !contextKey() && !active() ? 'Open the associated layout to enable pack building.' : '');
            const d = catalog && catalog.drc;
            el('drc-build-review').textContent = active() ? 'DRC reads paused · layout stays open.' :
                op ? (d ? (d.phase === 'ready' ? 'Review ready · selections were reset when the build began.' :
                    d.phase === 'opening' ? 'Opening the review metadata…' : 'Review unavailable · ' + (d.error || d.phase)) : 'Review registration unavailable.') : '';
        }
        function notify() {
            render();
            // A pending write must not adopt an older GET that was already in
            // flight. Only an authoritative post-receipt catalog releases it.
            if (suspend()) {
                if (catalog) { o.changed({drc: null, build: catalog.build}); }
                o.retire();
            } else if (catalog) { o.changed(catalog); }
        }
        function schedule() {
            o.clearTimeout(timer); timer = null;
            if (!stopped && (!catalog || catalog.build || catalog.drc && catalog.drc.phase === 'opening')) {
                timer = o.setTimeout(refresh, active() || waiting || catalog && catalog.drc && catalog.drc.phase === 'opening' ? 500 : 2500);
            }
        }
        async function refresh() {
            if (stopped) { return; }
            o.clearTimeout(timer); timer = null; abort(getTask);
            const t = {cancelled: false, abort: null}; getTask = t;
            try {
                const v = validate(await o.http('GET', '/api/v1/drc', undefined, false, t), P);
                if (t.cancelled || stopped || getTask !== t) { return; }
                catalog = v; stale = false; pollError = ''; notify();
            } catch (e) {
                if (!t.cancelled && !stopped && getTask === t) {
                    stale = true; pollError = 'Review state could not be checked. ' + e.message;
                    if (e.status === 401) { stopped = true; closeConfirmation(false); }
                    notify();
                }
            } finally { if (getTask === t) { getTask = null; schedule(); } }
        }
        async function send(request) {
            waiting = true; uncertain = false; pending = request; note = ''; closeConfirmation(false);
            abort(getTask); getTask = null; notify();
            const t = {cancelled: false, abort: null}; writeTask = t;
            try {
                const op = validateOperation(await o.http('POST', '/api/v1/drc/builds', request, false, t), P);
                if (op.seq !== request.seq) { throw new Error('Pack build receipt mismatch'); }
                if (t.cancelled || stopped || writeTask !== t) { return; }
                pending = null;
            } catch (e) {
                if (t.cancelled || stopped || writeTask !== t) { return; }
                // A parsed non-2xx HTTP response is a definitive rejection.
                // Transport errors and malformed receipts are not: the server
                // may already have accepted the exact approved request.
                uncertain = !(e.status >= 400 && e.status < 500);
                if (!uncertain) { pending = null; }
                note = uncertain ? 'Request outcome unknown. “Resolve request” sends the SAME approved request, never a new job.' : (errors[e.code] || e.message);
            } finally {
                if (writeTask === t) {
                    writeTask = null; waiting = false;
                    if (!stopped) { await refresh(); }
                }
            }
        }
        async function approve() {
            if (!confirmation || confirmation !== contextKey() || waiting || uncertain || active()) { return; }
            const key = confirmation, c = o.context(), jobsText = el('drc-build-jobs').value;
            if (!/^(?:[1-9]|1[0-6])$/.test(jobsText)) { note = 'Choose 1–16 jobs.'; render(); return; }
            const jobs = Number(jobsText), force = el('drc-build-force').checked;
            waiting = true; render();
            // Refresh the high-water mark before allocating, then verify the
            // confirmation still belongs to this view, epoch and DRC revision.
            await refresh(); waiting = false;
            if (stopped || key !== confirmation || key !== contextKey() || active()) { render(); return; }
            try {
                await send({seq: P.next(operations().last_seq), drc_id: catalog.drc.id,
                    revision: catalog.drc.revision, view_id: c.id, approve: true, force: force, jobs: jobs});
            } catch (e) { note = e.message; render(); }
        }
        async function cancel() {
            const seq = active(); if (!seq || stopped || stale || cancelTask) { return; }
            const t = {cancelled: false, abort: null}; cancelTask = t; render();
            try {
                validateOperation(await o.http('POST', '/api/v1/drc/builds/' + seq + '/cancel', {}, false, t), P);
                if (!t.cancelled) { note = 'Cancellation requested; a pack already published stays successful.'; }
            } catch (e) { if (!t.cancelled) { note = 'Cancellation was not confirmed. Check the job state. ' + e.message; } }
            finally { if (cancelTask === t) { cancelTask = null; if (!stopped) { await refresh(); } } }
        }
        el('drc-build-open').onclick = function () {
            if (!contextKey() || waiting || uncertain || active()) { return; }
            confirmation = contextKey(); note = '';
            el('drc-build-force').checked = false; el('drc-build-jobs').value = String(catalog.build.jobs_default);
            el('drc-build-source').textContent = catalog.drc.title;
            el('drc-build-form').hidden = false; el('drc-build-open').setAttribute('aria-expanded', 'true');
            render(); el('drc-build-jobs').focus();
        };
        el('drc-build-form').onsubmit = function (e) { e.preventDefault(); return approve(); };
        el('drc-build-dismiss').onclick = function () { closeConfirmation(true); render(); };
        el('drc-build-form').onkeydown = function (e) {
            if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); closeConfirmation(true); render(); }
        };
        el('drc-build-cancel').onclick = cancel;
        el('drc-build-resolve').onclick = function () { if (uncertain && pending && !waiting && !stopped) { return send(pending); } };
        render();
        return {refresh: refresh, contextChanged: render, suspended: suspend,
            escape: function () { const closed = closeConfirmation(true); render(); return closed; },
            stop: function () {
                stopped = true; stale = true; closeConfirmation(false); o.clearTimeout(timer); timer = null;
                if (writeTask) { uncertain = true; note = 'Request outcome unknown. Resolve the same request after returning.'; }
                waiting = false;
                [getTask, writeTask, cancelTask].forEach(abort); getTask = writeTask = cancelTask = null;
            },
            resume: function () { stopped = false; return refresh(); }};
    }
    const api = {bind: bind, validate: validate, validateOperation: validateOperation, status: status};
    if (typeof module === 'object' && module.exports) { module.exports = api; } else { root.FloeDRCBuild = api; }
}(typeof window === 'object' ? window : this));
