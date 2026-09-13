/* ES2017 CD presentation only. Distances/geometry come from Rust; projection
 * uses the displayed frame, never the pending requested viewport. */
(function (root) {
    'use strict';
    function decode(v, target, P) {
        if (!v || v.check !== target.check || v.local !== target.error || !Array.isArray(v.segments) || v.segments.length > 3) { throw new Error('Invalid CD response'); }
        P.counter(v.global);
        const roles = v.segments.length === 2 ? ['Width', 'Height'] : ['Gap', 'X', 'Y'];
        return {global: v.global, segments: v.segments.map(function (s, i) {
            if (!s || typeof s.offset !== 'boolean' || !Array.isArray(s.endpoints_um) || s.endpoints_um.length !== 2) { throw new Error('Invalid CD segment'); }
            const ends = s.endpoints_um.map(function (xy) {
                if (!Array.isArray(xy) || xy.length !== 2) { throw new Error('Invalid CD endpoint'); }
                return xy.map(function (n) { return Number(P.decimal(n)); });
            });
            const d = Number(P.decimal(s.distance_um));
            if (!(d > 0)) { throw new Error('Invalid CD distance'); }
            const role = v.segments.length === 1 && s.offset ? 'Length' : roles[i];
            return {ends: ends, distance: s.distance_um, offset: s.offset, role: role,
                label: role + ' ' + (d < .0001 || d >= 1e9 ? d.toExponential(4) : d.toFixed(4)) + ' µm'};
        })};
    }
    function offset(a, b) {
        const dx = b[0] - a[0], dy = b[1] - a[1], length = Math.hypot(dx, dy);
        if (!(length > 0)) { return [a, b]; }
        let nx = -dy / length, ny = dx / length;
        if (ny > 1e-12 || (Math.abs(ny) <= 1e-12 && nx < 0)) { nx = -nx; ny = -ny; }
        return [[a[0] + nx * 14, a[1] + ny * 14], [b[0] + nx * 14, b[1] + ny * 14]];
    }
    function project(s, toPixel, dpr) {
        const original = s.ends.map(function (v) { return toPixel(v[0], v[1]).map(function (n) { return n / dpr; }); });
        if (!original.every(function (xy) { return xy.every(Number.isFinite); })) { return null; }
        const ends = s.offset ? offset(original[0], original[1]) : original;
        if (!ends.every(function (xy) { return xy.every(Number.isFinite); })) { return null; }
        return {original: original, ends: ends, label: s.label, offset: s.offset};
    }
    // Liang-Barsky clipping also bounds the label anchors for very long lines.
    function clipped(a, b, rect) {
        let lo = 0, hi = 1;
        const dx = b[0] - a[0], dy = b[1] - a[1];
        if (!Number.isFinite(dx) || !Number.isFinite(dy)) { return null; }
        const p = [-dx, dx, -dy, dy], q = [a[0] - rect[0], rect[2] - a[0], a[1] - rect[1], rect[3] - a[1]];
        for (let i = 0; i < 4; ++i) {
            if (p[i] === 0) { if (q[i] < 0) { return null; } }
            else { const t = q[i] / p[i]; if (p[i] < 0) { lo = Math.max(lo, t); } else { hi = Math.min(hi, t); } }
            if (lo > hi) { return null; }
        }
        return [[a[0] + lo * dx, a[1] + lo * dy], [a[0] + hi * dx, a[1] + hi * dy]];
    }
    function overlap(a, b) { return a[0] < b[2] + 3 && b[0] < a[2] + 3 && a[1] < b[3] + 3 && b[1] < a[3] + 3; }
    function leader(ends, r) {
        const a = ends[0], b = ends[1], dx = b[0] - a[0], dy = b[1] - a[1], length = Math.hypot(dx, dy);
        const cx = (r[0] + r[2]) / 2, cy = (r[1] + r[3]) / 2;
        const t = length ? Math.max(0, Math.min(1, ((cx - a[0]) * (dx / length) + (cy - a[1]) * (dy / length)) / length)) : 0;
        const foot = [a[0] + dx * t, a[1] + dy * t];
        return [[Math.max(r[0], Math.min(r[2], foot[0])), Math.max(r[1], Math.min(r[3], foot[1]))], foot];
    }
    function labelSpot(ends, width, height, placed, lines, leaders, vw, vh) {
        if (width + 4 > vw || height + 4 > vh) { return null; }
        const a = ends[0], b = ends[1], dx = b[0] - a[0], dy = b[1] - a[1], len = Math.hypot(dx, dy) || 1;
        const nx = -dy / len, ny = dx / len, lift = (Math.abs(nx) * width + Math.abs(ny) * height) / 2 + 12;
        let fallback = null;
        const first = ny > 0 || (Math.abs(ny) < 1e-12 && nx < 0) ? -1 : 1;
        for (const t of [.5, .34, .66, .2, .8]) {
            for (const side of [first, -first]) {
                const x = Math.max(2, Math.min(vw - width - 2, a[0] + dx * t + nx * side * lift - width / 2));
                const y = Math.max(2, Math.min(vh - height - 2, a[1] + dy * t + ny * side * lift - height / 2));
                const r = [x, y, x + width, y + height], link = leader(ends, r);
                if (placed.some(function (v) { return overlap(v, r) || clipped(link[0], link[1], v); }) ||
                    leaders.some(function (v) { return clipped(v[0], v[1], r); })) { continue; }
                if (!fallback) { fallback = r; }
                if (!lines.some(function (v) { return clipped(v[0], v[1], r); })) { return r; }
            }
        }
        // Do not hide a different CD behind a chip; every value is also in the
        // accessible panel when a tiny viewport has no collision-free position.
        return fallback;
    }
    function line(ctx, ends) { ctx.beginPath(); ctx.moveTo(ends[0][0], ends[0][1]); ctx.lineTo(ends[1][0], ends[1][1]); ctx.stroke(); }
    function arrow(ctx, a, b, vw, vh) {
        if (a[0] < 0 || a[0] > vw || a[1] < 0 || a[1] > vh) { return; }
        const dx = b[0] - a[0], dy = b[1] - a[1], len = Math.hypot(dx, dy);
        if (!(len > 0)) { return; }
        const x = dx / len, y = dy / len, n = Math.min(8, len / 3), h = Math.min(3, n / 2);
        ctx.beginPath(); ctx.moveTo(a[0], a[1]); ctx.lineTo(a[0] + n * x - h * y, a[1] + n * y + h * x);
        ctx.lineTo(a[0] + n * x + h * y, a[1] + n * y - h * x); ctx.closePath(); ctx.fill();
    }
    function paint(ctx, segments, toPixel, size) {
        const dpr = size.dpr, w = size.pixels[0] / dpr, h = size.pixels[1] / dpr;
        const visible = segments.map(function (s) { return project(s, toPixel, dpr); }).filter(function (s) {
            if (!s) { return false; } s.visible = clipped(s.ends[0], s.ends[1], [0, 0, w, h]); return !!s.visible;
        });
        ctx.save(); ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.lineWidth = 1; ctx.globalAlpha = 1;
        ctx.strokeStyle = '#ffffff'; ctx.fillStyle = '#ffffff'; ctx.font = '12px sans-serif'; ctx.textBaseline = 'top';
        visible.forEach(function (s) {
            if (s.offset) { ctx.setLineDash([2, 3]); line(ctx, [s.original[0], s.ends[0]]); line(ctx, [s.original[1], s.ends[1]]); }
            ctx.setLineDash([]); line(ctx, s.visible);
            arrow(ctx, s.ends[0], s.ends[1], w, h); arrow(ctx, s.ends[1], s.ends[0], w, h);
        });
        const placed = [], leaders = [];
        visible.forEach(function (s, i) {
            const r = labelSpot(s.visible, ctx.measureText(s.label).width + 10, 20, placed,
                visible.filter(function (_, j) { return i !== j; }).map(function (v) { return v.visible; }), leaders, w, h);
            if (!r) { return; }
            const link = leader(s.visible, r); placed.push(r); leaders.push(link);
            ctx.setLineDash([2, 3]); line(ctx, link); ctx.setLineDash([]);
            ctx.fillStyle = '#101010'; ctx.fillRect(r[0], r[1], r[2] - r[0], r[3] - r[1]);
            ctx.fillStyle = '#ffffff'; ctx.fillText(s.label, r[0] + 5, r[1] + 3);
        });
        ctx.restore();
    }
    const api = {decode: decode, offset: offset, project: project, clipped: clipped, labelSpot: labelSpot, paint: paint};
    if (typeof module === 'object' && module.exports) { module.exports = api; } else { root.FloeRulers = api; }
}(typeof window === 'object' ? window : this));
