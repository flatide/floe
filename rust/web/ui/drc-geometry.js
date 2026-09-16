/* Shared display-only geometry helpers. No HTTP, storage or owner controllers. */
(function(root){
    'use strict';
    function projection(frame, origin, unit) {
        if (!frame || !origin) { return null; }
        const b = frame.bbox_dbu.map(Number), dbu = Number(unit === undefined ? frame.dbu_um : unit);
        const sx = (b[2] - b[0]) / frame.width, sy = (b[3] - b[1]) / frame.height;
        if (!(sx > 0 && sy > 0 && dbu > 0)) { return null; }
        return {bbox: b, dbu: dbu, step: [sx, sy], origin: origin.slice()};
    }
    function point(p, x, y) {
        return [(x / p.dbu - p.bbox[0]) / p.step[0] - p.origin[0],
            (p.bbox[3] - y / p.dbu) / p.step[1] - p.origin[1]];
    }
    function shifted(p, delta) {
        return p && {bbox: p.bbox, dbu: p.dbu, step: p.step,
            origin: [p.origin[0] + delta[0], p.origin[1] + delta[1]]};
    }
    function vertices(page, format, P) {
        const um = page.points_um !== undefined, dbu = page.points_dbu !== undefined;
        if (um === dbu || (format === 'ascii' && !um) || (format === 'ice' && !dbu)) { throw new Error('Invalid DRC geometry units'); }
        const precision = Number(P.decimal(page.precision)), rows = um ? page.points_um : page.points_dbu;
        if (!(precision > 0) || !Number.isFinite(precision) || !Array.isArray(rows) || !rows.length || rows.length > 2048) { throw new Error('Invalid DRC geometry page'); }
        const values = new Float64Array(rows.length * 2);
        rows.forEach(function (xy, i) {
            if (!Array.isArray(xy) || xy.length !== 2) { throw new Error('Invalid DRC vertex'); }
            xy.forEach(function (s, axis) {
                const n = Number(P.decimal(s)) / (um ? 1 : precision);
                if (!Number.isFinite(n)) { throw new Error('Unrepresentable DRC vertex'); }
                values[i * 2 + axis] = n;
            });
        });
        return {mode: um ? 'um' : 'dbu', precision: precision, points: values};
    }
    const api={projection:projection,point:point,shifted:shifted,vertices:vertices};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeDRCGeometry=api;}
}(typeof window==='object'?window:this));
