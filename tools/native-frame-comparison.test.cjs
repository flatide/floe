'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const {parse,compare}=require('./native-frame-comparison.cjs');
function lines(prefix='DESKTOP CROSS: ') {
    return [0,1,2].map(i=>prefix+[i,1600,1200,2,50000,87500,350000,312500,'a'.repeat(64),(i?'a':'b').repeat(64)].join(' '));
}
test('same-world same-physical counterpart equality; accepted intra-frame label difference retained',()=>{
    const r=compare(lines(),lines('ELECTRON CROSS: '));
    assert.equal(r.length,3);assert.notEqual(r[0].foreground,r[0].margin);
    assert.deepEqual(r[0].pixels,[1600,1200]);
});
test('missing, reordered, duplicate, malformed or geometric mismatch cannot pass',()=>{
    for(const rows of [lines().slice(1),[...lines(),lines()[0]],lines().reverse(),
        lines().map(s=>s.replace('1600','1599')),lines().map(s=>s.replace(' 2 50000',' NaN 50000')),
        lines().map(s=>s.replace('350000','40000')),lines().map(s=>s+' extra'),
        lines().map(s=>s.replace('a'.repeat(64),'secret'))]) {
        assert.throws(()=>parse(rows,'DESKTOP CROSS: '));
    }
});
test('neither different valid DPR/bbox nor changed counterpart hash counts as equivalent',()=>{
    for(const rows of [lines('ELECTRON CROSS: ').map(s=>s.replace('1600 1200 2','800 600 1')),
        lines('ELECTRON CROSS: ').map(s=>s.replace('87500','87501')),
        lines('ELECTRON CROSS: ').map(s=>s.replaceAll('a'.repeat(64),'c'.repeat(64)))]) {
        assert.throws(()=>compare(lines(),rows));
    }
    assert.throws(()=>compare(lines().map(s=>s.replace('87500','87501')),
        lines('ELECTRON CROSS: ').map(s=>s.replace('87500','87501'))));
});
