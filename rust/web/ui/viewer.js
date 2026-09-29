/* ES2017. Shared viewer behaviour; no credentials, HTTP, storage or owner APIs. */
(function (root) {
    'use strict';
    function dimensions(P, r, dpr, limit) {
        const left = Math.ceil(r.left * dpr), top = Math.ceil(r.top * dpr);
        let w = Math.max(1, Math.floor(r.right * dpr) - left), h = Math.max(1, Math.floor(r.bottom * dpr) - top);
        if (limit) {
            const scale = Math.min(1, limit.axis / w, limit.axis / h, Math.sqrt(limit.area / (w * h)));
            w = Math.max(1, Math.floor(w * scale)); h = Math.max(1, Math.floor(h * scale));
        }
        P.pixels(w, h);
        return {pixels: [w, h], dpr: dpr, left: left / dpr - r.left, top: top / dpr - r.top};
    }
    function screen(r, pixels, native) {
        // Do not stretch a sub-device-pixel edge remainder. Uncapped sessions
        // use the exact same device alignment as the standalone viewport.
        if (native && native.pixels[0] === pixels[0] && native.pixels[1] === pixels[1]) { return native; }
        const scale = Math.min(r.width / pixels[0], r.height / pixels[1]);
        return {pixels: pixels, dpr: 1 / scale, left: (r.width - pixels[0] * scale) / 2, top: (r.height - pixels[1] * scale) / 2};
    }
    function position(target, size, p) {
        target.style.width = (target.width / size.dpr) + 'px'; target.style.height = (target.height / size.dpr) + 'px';
        target.style.left = (size.left - p[0] / size.dpr) + 'px'; target.style.top = (size.top - p[1] / size.dpr) + 'px';
    }
    function centered(target, size) {
        return [-Math.round((size.pixels[0] - target.width) / 2), -Math.round((size.pixels[1] - target.height) / 2)];
    }
    function panDelta(P, state, edits, drag) {
        const delta = [0, 0];
        if (!state) { return null; }
        for (let i = 0; i < edits.length; ++i) {
            const n = edits[i].navigation;
            if (Object.keys(edits[i]).length !== 1 || !n || n.kind !== 'pan') { return null; }
            const period = n.snap ? 16 : 1;
            delta[0] += P.roundEven(n.x * state.pixels[0] / period) * period;
            delta[1] -= P.roundEven(n.y * state.pixels[1] / period) * period;
        }
        if (drag) { delta[0] += drag[0]; delta[1] += drag[1]; }
        return delta;
    }
    // Placement permits native-phase crops only. A non-placeable frame stays
    // visible as a frozen reference, never as a current receipt/query target.
    function compose(o) {
        const s = o.state, size = o.size, delta = o.delta;
        const sameSize = s && size.pixels[0] === s.pixels[0] && size.pixels[1] === s.pixels[1];
        const at = function (h) { const p = sameSize && delta && o.protocol.placement(h, s); return p && [p[0] + delta[0], p[1] + delta[1]]; };
        const mp = at(o.margin), fp = at(o.foreground), margin = o.marginCanvas;
        if (margin) { margin.hidden = !mp; if (mp) { position(margin, size, mp); } }
        const full = !!(mp && o.margin.complete && mp[0] >= 0 && mp[1] >= 0 &&
            mp[0] + size.pixels[0] <= margin.width && mp[1] + size.pixels[1] <= margin.height);
        o.canvas.hidden = full;
        const foreground = fp || centered(o.canvas, size); position(o.canvas, size, foreground);
        return {pixels: size.pixels, margin: mp, full: full, foreground: foreground};
    }
    function paint(target, header, draw) {
        if (target.width !== header.width || target.height !== header.height) { target.width = header.width; target.height = header.height; }
        const context = target.getContext('2d', {alpha: false}); context.imageSmoothingEnabled = false; draw(context);
    }
    function freeze(o) {
        const p = o.placement, canvas = o.canvas, margin = o.marginCanvas;
        if (!p) { return false; }
        const w = p.pixels[0], h = p.pixels[1], mp = p.margin, fp = p.foreground;
        const copy = !!mp || fp[0] !== 0 || fp[1] !== 0 || canvas.width !== w || canvas.height !== h;
        if (copy && p.full && mp) {
            paint(canvas, {width: w, height: h}, function (context) { context.drawImage(margin, mp[0], mp[1], w, h, 0, 0, w, h); });
        } else if (copy) {
            const temp = o.document.createElement('canvas'); temp.width = w; temp.height = h;
            const ctx = temp.getContext('2d', {alpha: false}); ctx.imageSmoothingEnabled = false;
            if (mp) { ctx.drawImage(margin, -mp[0], -mp[1]); }
            if (!p.full) { ctx.drawImage(canvas, -fp[0], -fp[1]); }
            paint(canvas, {width: w, height: h}, function (context) { context.drawImage(temp, 0, 0); });
            temp.width = temp.height = 1;
        }
        canvas.hidden = false; if (margin) { margin.hidden = true; } position(canvas, o.size, centered(canvas, o.size));
        return copy;
    }
    function sameSource(h, s) {
        return !!h && !!s && ['view_id', 'connection_epoch', 'dataset_revision', 'worker_epoch'].every(function (k) { return h[k] === s[k]; });
    }
    function frameStatus(h) {
        const perf = h.perf || {}, fitted = ['fit_pct', 'fit_cull', 'fit_over', 'fit_thin'].some(function (k) { return Number(perf[k] || 0) > 0; });
        return (!h.final ? 'Refining' : h.complete ? 'Live' : 'INCOMPLETE') + (h.approximate ? ' · approximate' : '') +
            (fitted ? ' · budget fit' : '') + (Number(perf.shape_cut || 0) > 0 ? ' · short-side cut' : '') +
            (h.labels_truncated ? ' · labels partial' : '') + (h.deck_skipped !== '0' ? ' · skipped ' + h.deck_skipped : '') + ' · gen ' + h.generation;
    }
    function busy(P, c) {
        if (!c.active || c.state && (c.state.failure || ['failed', 'closed'].includes(c.state.status))) { return false; }
        if (c.connecting) { return true; }
        if (!c.connected) { return false; }
        // A frozen composite can lose its query receipt without starting new
        // work (e.g. rejected/no-op pan). Its last native completion is only
        // a waiting hint, never permission to query or navigate a stale image.
        const s = c.state, h = c.frame || c.completedFrame;
        return !!(c.pending || c.decoding || c.presenting || !s || ['opening', 'rendering', 'cancelling'].includes(s.status) ||
            !h || !h.final || !P.matches(h, s));
    }
    function cursor(shell, viewport, waiting, gesture, crosshair) {
        shell.setAttribute('data-busy', String(waiting)); viewport.setAttribute('aria-busy', String(waiting));
        viewport.style.cursor = waiting ? 'wait' : gesture && gesture.bandActive() ? 'crosshair' :
            gesture && gesture.active() ? 'grabbing' : crosshair ? 'crosshair' : '';
    }
    function band(el, b) {
        const box = el('zoom-band'), hint = el('zoom-band-hint'); box.hidden = hint.hidden = !b; if (!b) { return; }
        const d = b.dimensions, x = b.start[0] * d.pixels[0], y = b.start[1] * d.pixels[1], ex = b.end[0] * d.pixels[0], ey = b.end[1] * d.pixels[1];
        box.style.left = ((d.left || 0) + Math.round(Math.min(x, ex)) / d.dpr) + 'px';
        box.style.top = ((d.top || 0) + Math.round(Math.min(y, ey)) / d.dpr) + 'px';
        box.style.width = (Math.max(1, Math.round(Math.abs(ex - x))) / d.dpr) + 'px';
        box.style.height = (Math.max(1, Math.round(Math.abs(ey - y))) / d.dpr) + 'px';
        box.style.borderWidth = (1 / d.dpr) + 'px';
        hint.textContent = (b.outward ? 'Zoom out' : 'Zoom in') + ' · release to apply · Esc cancels';
    }
    function wheel(o, event) {
        const c = o.context(); if (!c.active) { return; } event.preventDefault();
        if (!c.connected || busy(o.protocol, c) || !c.state || c.state.status !== 'idle' || !c.frame || !c.frame.final ||
            !o.protocol.matches(c.frame, c.state) || !c.acked || c.gesture || !c.size ||
            c.size.pixels.some(function (v, i) { return v !== c.state.pixels[i]; })) { return; }
        const n = o.gestures.wheelNavigation(event, c.size, o.viewport.getBoundingClientRect()); if (n) { o.navigate(n); }
    }
    function keyName(e) {
        const key = e.key;
        return key.length === 1 && key.charCodeAt(0) > 127 && /^Key[A-Z]$/.test(e.code || '') ?
            (e.shiftKey ? e.code.slice(3) : e.code.slice(3).toLowerCase()) : key;
    }
    function bindControls(o) {
        const el = o.el, P = o.protocol, fields = ['goto-x', 'goto-y', 'goto-width'], now = o.now || Date.now;
        let dirty = false, revision = 0, view = '', lastDigit = '', lastDigitAt = 0;
        function navigate(n) { if (n.kind === 'zoom' && !n.anchor) { n.anchor = [0.5, 0.5]; } return o.edit({navigation: n}); }
        function syncGoto(force) {
            const c = o.context(); if (!c || !c.state || c.gotoBlocked) { return; }
            if (view !== c.id) { view = c.id; dirty = false; ++revision; force = true; }
            if (!force && (dirty || fields.some(function (k) { return o.document.activeElement === el(k); }))) { return; }
            const camera = c.state.camera_um;
            if (camera !== null && camera !== undefined && (!Array.isArray(camera) || camera.length !== 3 || Number(P.decimal(camera[2])) <= 0)) { throw Error('Invalid camera coordinates'); }
            const values = camera == null ? ['', '', ''] : camera.map(P.decimal);
            fields.forEach(function (k, i) { el(k).value = values[i]; });
        }
        function sync() {
            const c = o.context(), s = c && c.state; if (!s) { return; }
            ['depth', 'detail', 'thin', 'font-px'].forEach(function (k) { if (o.document.activeElement !== el(k)) { el(k).value = k === 'font-px' ? s.font_px : s[k]; } });
            ['frames', 'labels', 'mono'].forEach(function (k) { el(k).checked = s[k]; });
            el('max-depth').textContent = s.max_depth == null ? '' : '/ ' + s.max_depth; syncGoto(false);
        }
        ['depth', 'detail', 'thin'].forEach(function (k) { el(k).onchange = function () {
            const value = el(k).value;
            if (k === 'depth' && !/^(full|[0-9]{1,3})$/.test(value)) { o.notice('Depth must be full or an integer.'); return; }
            const body = {}; body[k] = value; o.edit(body);
        }; });
        ['frames', 'labels', 'mono'].forEach(function (k) { el(k).onchange = function () { const body = {}; body[k] = el(k).checked; o.edit(body); }; });
        el('font-px').onchange = function () {
            const n = Number(el('font-px').value);
            if (!Number.isInteger(n) || n < 6 || n > 96) { o.notice('Label size must be 6–96 device pixels.'); return; }
            o.edit({font_px: n});
        };
        el('fit').onclick = function () { navigate({kind: 'fit'}); };
        el('zoom-in').onclick = function () { navigate({kind: 'zoom', factor: 0.8}); };
        el('zoom-out').onclick = function () { navigate({kind: 'zoom', factor: 1.25}); };
        el('goto-form').onsubmit = function (e) {
            e.preventDefault();
            try {
                const values = fields.map(function (k) { return P.decimal(el(k).value); }), rev = revision, c = o.context();
                if (Number(values[2]) <= 0) { throw Error('View width must be positive.'); }
                dirty = true;
                o.edit({navigation: {kind: 'goto', center_um: values.slice(0, 2), width_um: values[2]}}, function (error) {
                    const current = o.context(); if (!error && current && c && c.id === current.id && rev === revision) { dirty = false; syncGoto(true); }
                });
            } catch (error) { o.notice(error.message); }
        };
        fields.forEach(function (k) {
            el(k).addEventListener('input', function () { dirty = true; ++revision; });
            el(k).addEventListener('keydown', function (e) {
                if (e.key === 'Escape' && !e.isComposing && e.keyCode !== 229 && !e.ctrlKey && !e.metaKey && !e.altKey) {
                    e.preventDefault(); dirty = false; ++revision; syncGoto(true);
                }
            });
        });
        function key(e) {
            const c = o.context(); if (!c || !c.state || !c.ready || e.isComposing || e.keyCode === 229) { return; }
            const g = o.gesture && o.gesture();
            if (g && g.active()) { if (e.key === 'Escape') { e.preventDefault(); g.cancel(); } return; }
            if (e.metaKey || e.altKey) { return; }
            const k = keyName(e); let action = null;
            function focus(name) { el(name).focus(); if (name === 'goto-x') { el(name).select(); } }
            if (e.ctrlKey) {
                if (k.toLowerCase() === 'a') { action = function () { navigate({kind: 'fit'}); }; }
                else if (k.toLowerCase() === 'z') { action = function () { navigate({kind: 'zoom', factor: 0.5}); }; }
                else if (k === '.') { action = function () { focus('goto-x'); }; }
            } else {
                const amount = e.shiftKey ? 0.1 : 0.5;
                const dirs = {ArrowLeft: [-amount, 0], ArrowRight: [amount, 0], ArrowUp: [0, amount], ArrowDown: [0, -amount]};
                if (dirs[k]) { action = function () { navigate({kind: 'pan', x: dirs[k][0], y: dirs[k][1], snap: true}); }; }
                else if (['+', '=', '-', 'Z'].includes(k)) { action = function () { navigate({kind: 'zoom', factor: k === '-' ? 1.25 : k === 'Z' ? 2 : 0.8}); }; }
                else if (k === 'Home') { action = function () { navigate({kind: 'fit'}); }; }
                else if (k === 'f' || k === 'C') { action = function () { o.edit({frames: !c.state.frames}); }; }
                else if (k === 'b') { action = function () { o.edit({mono: !c.state.mono}); }; }
                else if (k === 'g' || k === 'd') { action = function () { focus(k === 'g' ? 'goto-x' : 'detail'); }; }
                else if (k === '<' || k === '>') { action = function () { o.edit({depth_step: k === '<' ? -1 : 1}); }; }
                else if (/^[0-9]$/.test(k)) { action = function () {
                    const depth = k === '9' && lastDigit === '9' && now() - lastDigitAt < 1000 ? 'full' : k;
                    lastDigit = depth === 'full' ? '' : k; lastDigitAt = now(); o.edit({depth: depth});
                }; }
            }
            if (action) { e.preventDefault(); action(); }
        }
        return {sync: sync, syncGoto: syncGoto, key: key, navigate: navigate, dirty: function () { return dirty; }};
    }
    const api = {dimensions: dimensions, screen: screen, position: position, centered: centered, panDelta: panDelta,
        compose: compose, paint: paint, freeze: freeze, sameSource: sameSource, busy: busy, cursor: cursor, band: band,
        wheel: wheel, keyName: keyName, bindControls: bindControls, frameStatus: frameStatus};
    if (typeof module === 'object' && module.exports) { module.exports = api; } else { root.FloeViewer = api; }
}(typeof window === 'object' ? window : this));
