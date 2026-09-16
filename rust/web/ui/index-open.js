/* ES2017. Explicit index approval, original server selection, read-only recovery. */
(function (root) {
    'use strict';
    function id(s) { return typeof s === 'string' && /^[0-9a-f]{64}$/.test(s); }
    function preview(v, P) {
        if (!v || !id(v.source_id) || typeof v.title !== 'string' || v.title.length > 1024 ||
            !['level','chip','layer'].includes(v.mode) || !['window','explicit'].includes(v.display_policy) ||
            !Number.isInteger(v.jobs_available) || v.jobs_available < 0 || v.jobs_available > 16 || !v.levels || typeof v.occupancy_default !== 'boolean') {
            throw Error('Invalid index preview');
        }
        P.counter(v.open_seq);
        const l = v.levels;
        if (l.mode !== 'all' && !(l.mode === 'only' && Array.isArray(l.ids) && l.ids.length > 0 && l.ids.length <= 4096 &&
            new Set(l.ids).size === l.ids.length && l.ids.every(function (s) { return typeof s === 'string' && /^-?(0|[1-9][0-9]{0,18})$/.test(s) && s !== '-0'; }))) {
            throw Error('Invalid original level selection');
        }
        if (v.reselect !== undefined) {
            const a = v.reselect;
            if (!a || Object.keys(a).sort().join(',') !== 'pixels,target' || !a.target || a.target.kind !== 'replace' || !Array.isArray(a.pixels) || a.pixels.length !== 2) { throw Error('Invalid reselection anchor'); }
            target(a.target,P); P.pixels(a.pixels[0],a.pixels[1]);
        }
        return v;
    }
    function target(v, P) {
        if (!v || !['empty','replace'].includes(v.kind)) { throw Error('Invalid view target'); }
        if (v.kind === 'replace') { if (!id(v.view_id) || Object.keys(v).sort().join(',') !== 'kind,state_rev,view_id') { throw Error('Invalid target view ID'); } P.counter(v.state_rev); }
        else if (Object.keys(v).length !== 1) { throw Error('Invalid empty target'); }
        return v;
    }
    function journal(v, session, P) {
        if (!v || v.schema !== 1 || v.session !== session) { throw Error('Saved approval belongs to another session'); }
        preview(v.preview, P);
        const r = v.request;
        if (!r || r.kind !== 'index_open' || r.approved !== true || !id(r.request_id) || r.open_seq !== v.preview.open_seq ||
            Object.keys(r).sort().join(',') !== 'approved,kind,open_seq,options,pixels,request_id,seq,target') { throw Error('Invalid saved approval'); }
        P.counter(r.seq); P.counter(r.open_seq); target(r.target, P);
        if (!Array.isArray(r.pixels) || r.pixels.length !== 2) { throw Error('Invalid saved pixels'); }
        P.pixels(r.pixels[0], r.pixels[1]);
        if (v.preview.reselect && (!sameTarget(r.target,v.preview.reselect.target) || !samePixels(r.pixels,v.preview.reselect.pixels))) { throw Error('Saved approval changed the reselection anchor'); }
        const o = r.options;
        if (!o || Object.keys(o).sort().join(',') !== 'force,jobs,lod,occupancy' || !Number.isInteger(o.jobs) || o.jobs < 1 || o.jobs > 16 ||
            ['force','lod','occupancy'].some(function (k) { return typeof o[k] !== 'boolean'; })) { throw Error('Invalid saved index options'); }
        return v;
    }
    function sameTarget(a,b) { return a.kind === b.kind && a.view_id === b.view_id && a.state_rev === b.state_rev; }
    function samePixels(a,b) { return Array.isArray(a) && a.length === 2 && a[0] === b[0] && a[1] === b[1]; }
    function resultText(v, message) {
        const i = v.index || {}, phase = i.native && i.native.phase || i.phase || v.phase;
        if (v.stage === 'open') {
            return 'Index succeeded. ' + (v.phase === 'succeeded' ? 'View attached; see canvas for rendering status.' :
                v.phase === 'opening' ? 'Opening the indexed layout…' : 'View was not opened: ' + message(v.error || v.phase) + ' Completed cache files remain.');
        }
        if (['failed','cancelled','incomplete'].includes(v.phase)) {
            return 'Index ' + v.phase + '. View not opened. ' + message(v.error || v.phase) + ' Any completed cache files remain.' +
                (Number.isInteger(i.skipped) ? ' Skipped sources: ' + i.skipped + '.' : '');
        }
        return 'Index · ' + phase + (Number.isInteger(i.total) ? ' · ' + i.completed + '/' + i.total + ' sources' : '');
    }
    function bind(o) {
        const el = o.el, doc = o.document, P = o.protocol, panel = el('index-open-dialog'), button = el('index-open');
        let enabled = false, paused = true, opened = false, candidate = null, review = null, pending = null;
        let busy = false, invalid = false, task = null, timer = null, prior = null, hidden = [], error = '', status = '';
        function blocked() { return opened || !!pending || invalid; }
        function paint() {
            button.hidden = !enabled || !(candidate || pending || invalid);
            button.disabled = !enabled || !(pending || invalid) && (!candidate || !o.ready());
            button.textContent = pending || invalid ? 'Resolve index / open request…' : 'Index and open…';
            el('index-open-status').textContent = error || status || 'Review the original selection. No indexing has started.';
            panel.setAttribute('aria-busy', busy ? 'true' : 'false');
            if (pending) {
                const options = pending.request.options;
                el('index-open-jobs').value = String(options.jobs);
                ['lod','occupancy','force'].forEach(function (k) { el('index-open-'+k).checked = options[k]; });
            }
            ['index-open-jobs','index-open-lod','index-open-occupancy','index-open-force'].forEach(function (k) { el(k).disabled = busy || !!pending || invalid || !review; });
            el('index-open-approve').hidden = !!pending || invalid;
            el('index-open-approve').disabled = busy || !review || !review.preview.jobs_available || !o.ready();
            el('index-open-check').hidden = !pending; el('index-open-check').disabled = busy || invalid;
            el('index-open-cancel').hidden = !pending; el('index-open-cancel').disabled = busy || invalid;
        }
        function details(v, viewTarget) {
            el('index-open-preview').textContent = v.title + '\nSource ID: ' + v.source_id + '\nMode: ' + v.mode +
                '\nLevels: ' + (v.levels.mode === 'all' ? 'all' : v.levels.ids.join(', ')) +
                '\nDisplay: ' + (v.display_policy === 'window' ? 'inherit the current window preferences' : 'preserve the original open options') +
                (v.reselect ? '\nCamera: fixed at the original level selection. Navigation/resize invalidates this request.' : '') +
                '\nAfter indexing: ' + (viewTarget.kind === 'empty' ? 'open in the empty workspace' : 'replace the current view only if its revision is unchanged') +
                '\nIndex CPU slots available now: ' + v.jobs_available + ' (advice only; not reserved).';
        }
        function save(v) { o.savePending(v === null ? null : JSON.stringify(v)); pending = v; }
        function show() {
            if (opened) { return; } opened = true; prior = doc.activeElement; panel.hidden = false;
            ['app-header','app-workspace'].forEach(function (k) { const n = el(k); hidden.push([n,n.getAttribute('aria-hidden')]); n.setAttribute('aria-hidden','true'); });
            el('index-open-close').focus(); o.changed(); paint();
        }
        function close(restore) {
            if (!opened) { return; } opened = false; panel.hidden = true;
            if (!pending && task) { task.cancelled = true; if (task.abort) { task.abort(); } task = null; busy = false; review = null; }
            hidden.forEach(function (p) { if (p[1] === null) { p[0].removeAttribute('aria-hidden'); } else { p[0].setAttribute('aria-hidden',p[1]); } }); hidden = [];
            if (restore) { (prior && doc.contains(prior) ? prior : button).focus(); } prior = null; o.changed();
        }
        function active(t) { return task === t && !t.cancelled && !paused; }
        function begin() { const t = {cancelled:false}; task = t; busy = true; error = ''; paint(); return t; }
        function later() {
            o.clearTimeout(timer); timer = null;
            if (!paused && pending && !busy && !error && !invalid) { timer = o.setTimeout(function () { return check(false); }, 500); }
        }
        function end(t) { if (task === t) { task = null; busy = false; paint(); later(); o.changed(); } }
        function match(v) {
            const r = pending.request;
            if (!v || v.seq !== r.seq || v.kind !== 'index_open' || v.request_id !== r.request_id || v.open_seq !== r.open_seq) {
                throw Error('A different request owns this sequence. No result was adopted or cancelled. End this session to discard the recovery record safely.');
            }
            if (!['index','open'].includes(v.stage) || !['queued','preparing','running','cancelling','opening','succeeded','failed','incomplete','cancelled'].includes(v.phase)) {
                throw Error('Invalid index / open receipt');
            }
            return v;
        }
        async function receive(v, t) {
            match(v); status = resultText(v, o.message); error = '';
            if (!['succeeded','failed','incomplete','cancelled'].includes(v.phase)) { return; }
            if (v.phase === 'succeeded' && (v.stage !== 'open' || !id(v.view_id) || !v.index || v.index.phase !== 'succeeded')) { throw Error('Invalid successful open receipt'); }
            await o.completed(v);
            if (!active(t)) { return; }
            const original = pending.preview;
            save(null); review = null;
            candidate = v.phase === 'succeeded' ? null : original;
            o.changed();
        }
        async function lookup(t) {
            try { return match(await o.http('GET','/api/v1/operations/' + pending.request.seq,undefined,false,t)); }
            catch (e) {
                if (!active(t) || e.code !== 'operation_expired') { throw e; }
                const all = await o.http('GET','/api/v1/operations',undefined,false,t);
                if (!active(t)) { return null; }
                P.counter(all.last_seq,true);
                if (P.next(all.last_seq) !== pending.request.seq) { throw Error('Approval receipt expired or the sequence changed. No new request was sent. End this session to discard its recovery record safely.'); }
                return null;
            }
        }
        async function check(retry) {
            if (paused || busy || !pending || invalid) { return; }
            const t = begin();
            try {
                let v = await lookup(t); if (!active(t)) { return; }
                if (!v && retry) { v = await o.http('POST','/api/v1/operations',pending.request,false,t); }
                if (!active(t)) { return; }
                if (!v) { throw Error('Approval outcome is unknown. Check / retry sends only the same saved request; it never creates another job.'); }
                await receive(v,t);
            } catch (e) { if (active(t)) { error = e.message || String(e); } }
            finally { end(t); }
        }
        async function open() {
            if (!enabled || paused || opened || !(pending || invalid) && (!candidate || !o.ready())) { return; }
            show();
            if (pending) { details(pending.preview,pending.request.target); check(false); return; }
            if (invalid) { return; }
            review = null; status = ''; const item = candidate, t = begin();
            el('index-open-preview').textContent = 'Reading original open selection…';
            try {
                const v = preview(await o.http('GET','/api/v1/operations/' + item.open_seq + '/index-open',undefined,false,t),P);
                if (!active(t) || !opened) { return; }
                if (v.open_seq !== item.open_seq || v.source_id !== item.source_id) { throw Error('The original open selection changed'); }
                const current = await o.http('GET','/api/v1/view',undefined,true,t);
                if (!active(t) || !opened) { return; }
                if (current && !['idle','rendering'].includes(current.view.status)) { throw Error('Wait for the current view to finish opening or closing.'); }
                const to = current ? {kind:'replace',view_id:current.view.view_id,state_rev:current.view.state_rev} : {kind:'empty'};
                if (v.reselect && (!sameTarget(to,v.reselect.target) || !samePixels(current && current.view.pixels,v.reselect.pixels) || !samePixels(o.pixels(),v.reselect.pixels))) {
                    throw Error('The view changed after selecting levels. Close this dialog and apply the levels again. No indexing has started.');
                }
                target(to,P); review = {preview:v,target:to}; details(v,to);
                el('index-open-jobs').value = String(Math.max(1,Math.min(12,v.jobs_available)));
                ['lod','force'].forEach(function (k) { el('index-open-'+k).checked = false; });
                el('index-open-occupancy').checked = v.occupancy_default;
                if (!v.jobs_available) { status = 'No index slots are available. Close this dialog and review again after other work finishes.'; }
            } catch (e) { if (active(t)) { error = e.message || String(e); } }
            finally { end(t); }
        }
        async function approve() {
            if (paused || busy || pending || invalid || !opened || !review || !review.preview.jobs_available || !o.ready()) { return; }
            const reviewed = review, t = begin();
            try {
                if (reviewed.preview.reselect && !samePixels(o.pixels(),reviewed.preview.reselect.pixels)) { throw Error('The viewport resized. Close this dialog and apply the levels again.'); }
                const jobs = Number(el('index-open-jobs').value);
                if (!Number.isInteger(jobs) || jobs < 1 || jobs > 16) { throw Error('Index jobs must be 1–16.'); }
                const options = {jobs:jobs,force:el('index-open-force').checked,lod:el('index-open-lod').checked,occupancy:el('index-open-occupancy').checked};
                const all = await o.http('GET','/api/v1/operations',undefined,false,t);
                if (!active(t) || !opened) { return; }
                if (all.active !== null) { throw Error('Another owner operation is running. Review and approve again after it finishes.'); }
                const r = {kind:'index_open',seq:P.next(all.last_seq),request_id:o.randomId(),open_seq:reviewed.preview.open_seq,
                    approved:true,target:reviewed.target,pixels:reviewed.preview.reselect ? reviewed.preview.reselect.pixels : o.pixels(),options:options};
                const record = journal({schema:1,session:o.session(),preview:reviewed.preview,request:r},o.session(),P);
                save(record); // MUST succeed before any mutation is submitted.
                o.changed(); paint();
                const v = await o.http('POST','/api/v1/operations',r,false,t);
                if (active(t)) { await receive(v,t); }
            } catch (e) { if (active(t)) { error = e.message || String(e); } }
            finally { end(t); }
        }
        async function cancel() {
            if (paused || busy || !pending || invalid) { return; }
            const t = begin();
            try {
                const v = await lookup(t); if (!active(t)) { return; }
                // Never cancel a colliding sequence or claim that a possibly
                // in-flight, not-yet-admitted POST cannot arrive after this read.
                if (!v) { throw Error('Admission is unconfirmed; cancellation cannot fence a late request. Check / retry the identical approval, or close this dialog and End session.'); }
                if (['succeeded','failed','incomplete','cancelled'].includes(v.phase)) { await receive(v,t); }
                else { const next = await o.http('POST','/api/v1/operations/' + pending.request.seq + '/cancel',{},false,t); if (active(t)) { await receive(next,t); } }
            } catch (e) { if (active(t)) { error = e.message || String(e); } }
            finally { end(t); }
        }
        function observe(all) {
            if (!enabled || pending || invalid) { return; }
            const history = all.history || [], last = history.slice().reverse().find(function (v) { return ['open','reselect_levels','index_open'].includes(v.kind); });
            if (last && ['open','reselect_levels'].includes(last.kind)) { candidate = last.phase === 'failed' ? last.index_open || null : null; }
            else if (last && last.kind === 'index_open') {
                const original = history.find(function (v) { return v.seq === last.open_seq && ['open','reselect_levels'].includes(v.kind); });
                candidate = ['failed','incomplete','cancelled'].includes(last.phase) && original ? original.index_open || null : null;
            } else { candidate = null; }
            paint();
        }
        function stop() {
            paused = true; o.clearTimeout(timer);
            if (task) { task.cancelled = true; if (task.abort) { task.abort(); } task = null; }
            busy = false; review = null; close(false);
        }
        async function resume() {
            if (!enabled) { return; } paused = false;
            if (pending || invalid) { show(); if (pending) { details(pending.preview,pending.request.target); await check(false); } }
            paint();
        }
        async function init(supported) {
            enabled = !!supported; paint(); if (!enabled) { return; }
            try { const saved = o.loadPending(); if (saved) { if (saved.length > 131072) { throw Error('Saved approval limit'); } pending = journal(JSON.parse(saved),o.session(),P); } }
            catch (_) { invalid = true; error = 'Saved approval is invalid. No request was sent. Close this dialog and End session to clear it safely.'; }
            await resume();
        }
        button.onclick = open;
        el('index-open-close').onclick = function () { close(true); };
        el('index-open-approve').onclick = approve;
        el('index-open-check').onclick = function () { return check(true); };
        el('index-open-cancel').onclick = cancel;
        doc.addEventListener('keydown',function (e) {
            if (!opened) { return; } e.stopPropagation();
            if (e.isComposing) { return; }
            if (e.key === 'Escape') { e.preventDefault(); close(true); return; }
            if (e.key === 'Tab') {
                const items = Array.from(panel.querySelectorAll('button,input,select,[tabindex="0"]')).filter(function (n) { return !n.disabled && n.getClientRects().length; });
                const at = items.indexOf(doc.activeElement);
                if (!items.length) { e.preventDefault(); panel.focus(); }
                else if (at < 0 || e.shiftKey && at === 0 || !e.shiftKey && at === items.length-1) { e.preventDefault(); items[e.shiftKey ? items.length-1 : 0].focus(); }
            }
        },true);
        doc.addEventListener('focusin',function (e) { if (opened && !panel.contains(e.target)) { panel.focus(); } },true);
        return {init:init,observe:observe,changed:paint,blocked:blocked,pending:function () { return !!pending || invalid; },stop:stop,resume:resume,
            clear:function () { save(null); invalid=false; candidate=null; }};
    }
    const api = {bind:bind,preview:preview,journal:journal,resultText:resultText};
    if (typeof module === 'object' && module.exports) { module.exports = api; } else { root.FloeIndexOpen = api; }
}(typeof window === 'object' ? window : globalThis));
