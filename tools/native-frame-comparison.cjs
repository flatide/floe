'use strict';
// Numeric synthetic render metadata only; never accept an arbitrary file path.
function parse(lines,prefix) {
    const rows=lines.filter(s=>s.startsWith(prefix));
    if(rows.length!==3)throw Error('Missing/duplicate cross-host phases');
    return rows.map((line,phase)=>{
        const f=line.slice(prefix.length).split(' ');
        if(f.length!==10||!f.slice(0,8).every(s=>/^-?\d+(?:\.\d+)?$/.test(s))||
           !f.slice(8).every(s=>/^[a-f0-9]{64}$/.test(s)))throw Error('Invalid cross-host record');
        const n=f.slice(0,8).map(Number),[p,w,h,dpr,x0,y0,x1,y1]=n;
        if(!n.every(Number.isFinite)||p!==phase||dpr<0.25||dpr>4||w!==800*dpr||h!==600*dpr||
           !Number.isInteger(w)||!Number.isInteger(h)||x0>=x1||y0>=y1||n.slice(4).some(v=>Math.abs(v)>Number.MAX_SAFE_INTEGER)||
           (phase!==0&&f[8]!==f[9]))throw Error('Mismatched or unbounded cross-host geometry');
        return {phase,pixels:[w,h],dpr,bbox:[x0,y0,x1,y1],foreground:f[8],margin:f[9]};
    });
}
function compare(wk,electron) {
    const a=parse(wk,'DESKTOP CROSS: '),b=parse(electron,'ELECTRON CROSS: ');
    for(let i=0;i<3;i++) {
        // This driver owns valmini (1 nm DBU), goto 200,200,300 um and 4:3.
        if(JSON.stringify(a[i].bbox)!=='[50000,87500,350000,312500]'||a[i].dpr!==a[0].dpr) {
            throw Error('Synthetic target camera or DPR changed');
        }
        if(JSON.stringify(a[i])!==JSON.stringify(b[i]))throw Error('Cross-host viewport, DPR, bbox or RGBA mismatch');
    }
    return a;
}
module.exports={parse,compare};
