/* ES2017 wire validation + display-only pixel projection. Never sends world math. */
(function (root) {
    'use strict';
    const MAX_U64 = '18446744073709551615';
    const MAX_PAYLOAD = 80 * 1024 * 1024;
    function fail(message) { throw new Error(message); }
    function counter(s, zero) {
        if (typeof s !== 'string' || !/^(0|[1-9][0-9]*)$/.test(s) ||
            (!zero && s === '0') || s.length > 20 ||
            (s.length === 20 && s > MAX_U64)) { fail('Invalid sequence'); }
        return s;
    }
    function compare(a, b) {
        counter(a, true); counter(b, true);
        return a.length === b.length ? (a === b ? 0 : (a < b ? -1 : 1)) :
            (a.length < b.length ? -1 : 1);
    }
    function next(s) {
        counter(s, true);
        if (s === MAX_U64) { fail('Sequence exhausted; start a new session'); }
        const a = s.split('');
        let i = a.length - 1;
        while (i >= 0 && a[i] === '9') { a[i--] = '0'; }
        if (i < 0) { a.unshift('1'); } else { a[i] = String(Number(a[i]) + 1); }
        return a.join('');
    }
    function pair(p) {
        return Array.isArray(p) && p.length === 2 && p.every(function (n) {
            return Number.isInteger(n) && n >= 0 && n <= 4294967295;
        });
    }
    function decimal(s) {
        if (typeof s !== 'string' || s.length > 64 || s.trim() !== s ||
            !/^[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$/.test(s) ||
            !Number.isFinite(Number(s))) { fail('Invalid coordinate'); }
        return s;
    }
    function bbox(b) {
        if (!Array.isArray(b) || b.length !== 4) { fail('Invalid bounds'); }
        b.forEach(decimal);
        if (!(Number(b[0]) < Number(b[2]) && Number(b[1]) < Number(b[3]))) {
            fail('Invalid bounds');
        }
        return b;
    }
    function pixels(w, h) {
        if (!Number.isInteger(w) || !Number.isInteger(h) || w < 1 || h < 1 ||
            w > 8192 || h > 8192 || w * h > 16 * 1024 * 1024) {
            fail('Canvas exceeds 8192 px/axis or 16 Mpx; reduce the window size');
        }
    }
    function equal(a, b) {
        return Array.isArray(a) && Array.isArray(b) && a.length === b.length &&
            a.every(function (v, i) { return v === b[i]; });
    }
    function matches(h, s) {
        if (h.purpose === 'margin') {
            return !!s && !!s.margin && s.margin.frame_id === h.frame_id && h.final && placement(h, s) !== null;
        }
        return !!s && ['view_id', 'connection_epoch', 'dataset_revision',
            'worker_epoch', 'render_rev', 'render_key'].every(function (k) {
            return h[k] === s[k];
        }) && h.width === s.pixels[0] && h.height === s.pixels[1] &&
            equal(h.bbox_dbu, s.bbox_dbu);
    }
    function roundEven(n) {
        const lo = Math.floor(n), f = n - lo;
        return f < 0.5 ? lo : (f > 0.5 ? lo + 1 : (lo % 2 === 0 ? lo : lo + 1));
    }
    // Only unscaled, integer-device-pixel placement. Rust independently checks
    // the same condition before suppressing a foreground render.
    function placement(h, s) {
        if (!s || !h || !['view_id', 'connection_epoch', 'dataset_revision', 'worker_epoch', 'render_key'].every(function (k) { return h[k] === s[k]; })) { return null; }
        const a = h.bbox_dbu.map(Number), b = s.bbox_dbu.map(Number);
        const sx = (a[2] - a[0]) / h.width, sy = (a[3] - a[1]) / h.height;
        if (!(sx > 0 && sy > 0) || Math.abs((b[2] - b[0]) / s.pixels[0] / sx - 1) > 1e-9 ||
            Math.abs((b[3] - b[1]) / s.pixels[1] / sy - 1) > 1e-9) { return null; }
        const p = [(b[0] - a[0]) / sx, (a[3] - b[3]) / sy], out = p.map(roundEven);
        if (out.some(function (v, i) { return !Number.isFinite(v) || Math.abs(v) > 2147483647 || Math.abs(v - p[i]) > 1e-3 || v % 16 !== 0; })) { return null; }
        return out;
    }
    function packet(buffer) {
        if (!(buffer instanceof ArrayBuffer) || buffer.byteLength < 4 ||
            buffer.byteLength > MAX_PAYLOAD + 65540) { fail('Frame size limit'); }
        const view = new DataView(buffer);
        const length = view.getUint32(0, true);
        if (!length || length > 65536 || length + 4 > buffer.byteLength) {
            fail('Frame header limit');
        }
        const h = JSON.parse(new TextDecoder('utf-8', {fatal: true}).decode(
            new Uint8Array(buffer, 4, length)));
        if (!h || h.type !== 'frame' || h.protocol !== 1 || h.row0 !== 'top' ||
            !['foreground', 'margin'].includes(h.purpose) || !['raw', 'png'].includes(h.format)) {
            fail('Unsupported frame protocol');
        }
        ['view_id', 'connection_epoch'].forEach(function (k) {
            if (typeof h[k] !== 'string' || !/^[0-9a-f]{64}$/.test(h[k])) {
                fail('Invalid frame identity');
            }
        });
        ['frame_id', 'dataset_revision', 'state_rev', 'render_rev', 'render_key',
            'worker_epoch', 'generation', 'round', 'payload_length'].forEach(function (k) {
            counter(h[k], false);
        });
        counter(h.deferred, true); counter(h.deck_skipped, true);
        ['final', 'partial', 'labels_truncated', 'complete', 'approximate', 'query'].forEach(function (k) {
            if (typeof h[k] !== 'boolean') { fail('Invalid frame status'); }
        });
        const scene = h.query_scene;
        if (!scene || typeof scene !== 'object' || Array.isArray(scene) ||
            Object.keys(scene).sort().join(',') !== 'complete,generation,round,summary_layers' ||
            typeof scene.complete !== 'boolean' || (scene.generation === null) !== (scene.round === null)) { fail('Invalid query scene'); }
        counter(scene.summary_layers, true);
        if (scene.generation !== null) { counter(scene.generation); counter(scene.round); }
        if ((scene.complete && scene.generation === null) || h.query !== (scene.generation !== null && scene.complete)) { fail('Invalid query capability'); }
        if (h.complete && (!h.final || h.partial || h.labels_truncated ||
            h.deferred !== '0' || h.deck_skipped !== '0')) { fail('Inconsistent frame status'); }
        pixels(h.width, h.height); bbox(h.bbox_dbu);
        const size = buffer.byteLength - 4 - length;
        if (size > MAX_PAYLOAD || String(size) !== h.payload_length) { fail('Frame payload mismatch'); }
        const data = new Uint8Array(buffer, length + 4, size);
        const dv = new DataView(buffer, length + 4, size);
        const magic = h.format === 'raw' ? [70, 76, 79, 69, 82, 65, 87, 49] :
            [137, 80, 78, 71, 13, 10, 26, 10];
        if (size < (h.format === 'raw' ? 16 : 33) ||
            !magic.every(function (n, i) { return data[i] === n; })) { fail('Invalid image header'); }
        if (h.format === 'raw') {
            if (size !== 16 + h.width * h.height * 4 || dv.getUint32(8, true) !== h.width ||
                dv.getUint32(12, true) !== h.height) { fail('Raw dimensions mismatch'); }
        } else if (dv.getUint32(8) !== 13 || dv.getUint32(12) !== 0x49484452 ||
            dv.getUint32(16) !== h.width || dv.getUint32(20) !== h.height) {
            fail('PNG dimensions mismatch');
        }
        return {header: h, data: data};
    }
    const api = Object.freeze({counter: counter, compare: compare, next: next,
        decimal: decimal, bbox: bbox, pixels: pixels, pair: pair, matches: matches, packet: packet,
        placement: placement, roundEven: roundEven});
    if (typeof module !== 'undefined' && module.exports) { module.exports = api; }
    else { root.FloeProtocol = api; }
}(typeof window === 'undefined' ? this : window));
