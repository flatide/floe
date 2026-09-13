/* Native shape selection and opt-in snap probe. Accepted geometry is an
 * annotation, not a replacement renderer; truncated outlines stay open. */
(function (root) {
    'use strict';
    function bind(o) {
        const P = o.protocol, Q = o.query, el = function (id) { return o.document.getElementById(id); };
        const canvas = el('query-canvas'), ctx = canvas.getContext('2d');
        let selections = [], snap = null, cycle = null, bound = '', queryStamp = '', waiting = false;
        let projection = null, size = null, painting = null, pickTurn = 0, snapTurn = 0, overlayVisible = true;
        const transport = Q.bind({protocol: P, context: o.context, send: o.send, now: o.now,
            setTimeout: o.setTimeout, clearTimeout: o.clearTimeout});
        function info(s) { el('pick-status').textContent = s; }
        function identity(c) { return c && c.connected ? [c.id, c.state.connection_epoch, c.state.dataset_revision, c.state.worker_epoch, c.state.render_key].join(':') : ''; }
        function geometryKey(h) { return JSON.stringify([h.pair, h.cell_name, h.bbox_dbu, h.area_dbu2, h.points_dbu, h.points_truncated]); }
        function paintLater() {
            if (painting === null) { painting = o.window.requestAnimationFrame(function () { painting = null; draw(projection, size); }); }
        }
        function refresh() {
            const h = selections.length ? selections[selections.length - 1] : null;
            el('pick-details').textContent = h ? h.layer_name + ' · ' + h.pair.join('/') + '\n' + h.cell_name +
                '\nArea ' + h.area_dbu2 + ' DBU²\nBounds (DBU): ' + h.bbox_dbu.join(', ') +
                (h.points_truncated ? '\nOutline truncated: showing an open prefix and dashed bounds.' : '') : '';
            el('pick-clear').disabled = !selections.length && !waiting;
            el('pick-prev').disabled = el('pick-next').disabled = !cycle || waiting;
            o.layers(selections.map(function (s) { return s.pair; })); paintLater();
        }
        function invalidate() { ++pickTurn; ++snapTurn; waiting = false; transport.cancel('pick'); transport.cancel('snap'); snap = null; el('snap-status').textContent = ''; }
        function clear() { invalidate(); selections = []; cycle = null; info('Selection cleared.'); refresh(); }
        function changed() {
            const c = o.context(), key = identity(c), s = transport.changed();
            if (bound !== key) { invalidate(); selections = []; cycle = null; bound = key; info('Click a shape to inspect it.'); refresh(); }
            const stamp = s ? s.key : '';
            if (queryStamp !== stamp) {
                if (waiting) { info('View changed; pending selection was discarded.'); }
                ++pickTurn; ++snapTurn; waiting = false; snap = null; el('snap-status').textContent = ''; cycle = null; queryStamp = stamp; refresh();
            }
            el('snap-probe').disabled = !c || !c.connected || !c.state.capabilities.query;
            el('query-availability').textContent = !c || !c.connected ? 'Connect a layout to inspect geometry.' :
                !c.state.capabilities.query ? 'Jobdeck geometry queries are not supported.' :
                !s ? 'Waiting for a current displayed geometry frame.' :
                c.frame.query_scene.summary_layers !== '0' ? 'Summary layers are not queryable; zoom in or hide them.' : '';
            return s;
        }
        function pick(p, nth, mode) {
            const c = o.context(), s = changed(); if (!s) { info('Wait for a current displayed geometry frame.'); return false; }
            const turn = ++pickTurn; waiting = true; info('Selecting…'); refresh();
            const ok = transport.request('pick', p, Math.min(64, 3 * c.size.dpr), nth, function (v) {
                if (turn !== pickTurn) { return; }
                waiting = false;
                if (v.status !== 'ok') { cycle = null; info(v.message); refresh(); return; }
                if (!v.hit) {
                    if (mode === 'replace') { selections = []; }
                    cycle = null; info('No shape at this point.'); refresh(); return;
                }
                const h = v.hit, key = geometryKey(h), kept = selections.filter(function (r) { return geometryKey(r) !== key; });
                if (mode === 'replace') { selections = [h]; }
                else if (mode === 'toggle' && kept.length !== selections.length) { selections = kept; }
                else if (kept.length < 64) { selections = kept.concat([h]); }
                else { cycle = null; info('64 selected shapes: clear a selection before adding more.'); refresh(); return; }
                cycle = mode === 'replace' ? {position: p.slice(), index: Number(h.index), count: Number(h.count), stamp: s.key} : null;
                info(selections.length + ' selected · overlap ' + (Number(h.index) + 1) + '/' + h.count);
                refresh();
            });
            if (!ok) { waiting = false; info('Query could not start. Select again.'); refresh(); }
            return ok;
        }
        function click(x, y, modifiers) {
            const c = o.context(), s = changed(); if (!s) { return false; }
            const p = Q.position(c, x, y); if (!p) { return false; }
            const m = modifiers || {}, mode = m.ctrlKey || m.metaKey ? 'toggle' : m.shiftKey ? 'add' : 'replace';
            const near = cycle && cycle.stamp === s.key && p.every(function (v, i) { return Math.abs(v - cycle.position[i]) * c.size.pixels[i] <= 8 * c.size.dpr; });
            const nth = mode === 'replace' && near ? String((cycle.index + 1) % cycle.count) : '0';
            return pick(p, nth, mode);
        }
        function next(delta) {
            if (!changed() || !cycle) { return; }
            pick(cycle.position, String((cycle.index + delta + cycle.count) % cycle.count), 'replace');
        }
        function move(x, y) {
            const c = o.context(), s = changed();
            const p = s && Number.isFinite(x) && Number.isFinite(y) ? Q.position(c, x, y) : null;
            const hadSnap = snap !== null;
            ++snapTurn; snap = null; el('snap-status').textContent = '';
            // With the probe off, hover must not repaint every selected outline.
            if (hadSnap) { paintLater(); }
            if (!p || !el('snap-probe').checked) { transport.cancel('snap'); return; }
            const turn = snapTurn;
            transport.request('snap', p, Math.min(64, 10 * c.size.dpr), '0', function (v) {
                if (turn !== snapTurn) { return; }
                snap = v.status === 'ok' ? v.hit : null;
                el('snap-status').textContent = v.status !== 'ok' ? v.message : !snap ? 'No snap point nearby.' :
                    snap.snap + ' · DBU ' + snap.point_dbu.join(', ');
                paintLater();
            });
        }
        function toggleSnap() {
            if (el('snap-probe').disabled) { return false; }
            el('snap-probe').checked = !el('snap-probe').checked; move(NaN, NaN); return true;
        }
        function paint(p, s) {
            projection = p; size = s;
            paintLater();
        }
        function draw(p, s) {
            if (!overlayVisible || !p || !s || !bound || (!selections.length && !snap)) { canvas.hidden = true; return; }
            const w = s.pixels[0], h = s.pixels[1], dpr = s.dpr;
            if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
            canvas.style.width = w / dpr + 'px'; canvas.style.height = h / dpr + 'px';
            canvas.style.left = s.left + 'px'; canvas.style.top = s.top + 'px'; canvas.hidden = false;
            ctx.clearRect(0, 0, w, h); ctx.save(); ctx.beginPath(); ctx.rect(0, 0, w, h); ctx.clip();
            function point(a) { return [(Number(a[0]) - p.bbox[0]) / p.step[0] - p.origin[0], (p.bbox[3] - Number(a[1])) / p.step[1] - p.origin[1]]; }
            function trace(points, closed, dashed, color) {
                if (!points.length || points.some(function (xy) { return !xy.every(Number.isFinite); })) { return; }
                ctx.setLineDash(dashed ? [4 * dpr, 3 * dpr] : []); ctx.beginPath(); ctx.moveTo(points[0][0], points[0][1]);
                points.slice(1).forEach(function (xy) { ctx.lineTo(xy[0], xy[1]); }); if (closed) { ctx.closePath(); }
                ctx.strokeStyle = '#101010'; ctx.lineWidth = 3 * dpr; ctx.stroke();
                ctx.strokeStyle = color; ctx.lineWidth = dpr; ctx.stroke();
            }
            selections.forEach(function (v) {
                const b = v.bbox_dbu;
                if (v.points_truncated || v.points_dbu.length < 2) {
                    trace([[b[0],b[1]],[b[2],b[1]],[b[2],b[3]],[b[0],b[3]]].map(point), true, true, '#ffd819');
                }
                trace(v.points_dbu.map(point), !v.points_truncated && v.points_dbu.length > 2, false, '#ffd819');
            });
            if (snap) {
                const xy = point(snap.point_dbu), r = 5 * dpr;
                trace([[xy[0]-r,xy[1]],[xy[0]+r,xy[1]]], false, false, '#72e8da');
                trace([[xy[0],xy[1]-r],[xy[0],xy[1]+r]], false, false, '#72e8da');
            }
            ctx.restore();
        }
        el('pick-clear').onclick = clear;
        el('pick-prev').onclick = function () { next(-1); };
        el('pick-next').onclick = function () { next(1); };
        el('snap-probe').onchange = function () { move(NaN, NaN); };
        return {changed: changed, paint: paint, click: click, move: move, receive: function (m) { changed(); return transport.receive(m); },
            showOverlay:function (show) { overlayVisible=!!show;draw(projection,size); },
            flush:function () { if(painting!==null){o.window.cancelAnimationFrame(painting);painting=null;}draw(projection,size); },
            selection:function () { changed(); return selections.map(function (s) { return s.bbox_dbu.slice(); }); },
            interrupt: function () { if (waiting) { info('Selection request cancelled.'); } invalidate(); cycle = null; refresh(); },
            key: function (key) {
                if (key === 'm') { return toggleSnap(); }
                if (key === 'Escape' && (waiting || selections.length || snap)) { clear(); return true; } return false;
            }, stop: function () { transport.stop(); invalidate(); selections = []; cycle = null; bound = ''; o.layers([]); canvas.hidden = true; canvas.width = 1; canvas.height = 1;
                if (painting !== null) { o.window.cancelAnimationFrame(painting); painting = null; } },
            resume: function () { transport.resume(); changed(); }};
    }
    const api = {bind: bind};
    if (typeof module === 'object' && module.exports) { module.exports = api; } else { root.FloeInspect = api; }
}(typeof window === 'object' ? window : this));
