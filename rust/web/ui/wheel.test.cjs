'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const {wheelNavigation: wheel} = require('./gestures.js');
const size = {pixels:[100,80], dpr:1, left:0, top:0}, rect = {left:0,top:0};
const event = values => ({deltaY:-1,deltaMode:1,buttons:0,clientX:25,clientY:20,...values});
for (const mode of [0,1,2]) {
    for (const dy of [-1e9,-100,-1,-.5,-.001,.001,.5,1,100,1e9]) {
        const got=wheel(event({deltaY:dy,deltaMode:mode}),size,rect);
        assert(got.factor>=.96 && got.factor<=1/.96);
        assert.equal(got.kind,'zoom');assert.deepEqual(got.anchor,[.25,.25]);
        assert(Math.abs(got.factor*Math.pow(.96,Math.max(-1,Math.min(1,dy)))-1)<1e-14);
    }
}
for(const values of [{deltaY:0,deltaX:500},{deltaY:-0},{deltaY:NaN},{deltaY:Infinity},
    {deltaY:'1'},{deltaY:null},{deltaMode:3},{deltaMode:'0'},{clientX:NaN},{clientY:Infinity},
    {buttons:1},{buttons:2},{buttons:4},{buttons:5}]){
    assert.equal(wheel(event(values),size,rect),null);
}
// Device-aligned viewport starts inside a fractional CSS box. Browser zoom
// changes DPR, not the meaning of the emitted normalized anchor.
for(const dpr of [1,1.25,1.5,2,3]){
    const box={left:10.2,top:20.3};
    const s={pixels:[100,80],dpr,left:Math.ceil(box.left*dpr)/dpr-box.left,top:Math.ceil(box.top*dpr)/dpr-box.top};
    const got=wheel(event({clientX:box.left+s.left+25/dpr,clientY:box.top+s.top+20/dpr}),s,box);
    got.anchor.forEach((v,i)=>assert(Math.abs(v-[.25,.25][i])<1e-14));
    assert.deepEqual(wheel(event({clientX:-100,clientY:1000}),s,box).anchor,[0,1]);
}
if(process.argv[2]){
    const cases=JSON.parse(fs.readFileSync(process.argv[2],'utf8'));
    for(const c of cases){
        const got=wheel(event(c.event),size,rect);
        if(!c.expected.length){assert.equal(got,null,c.name);}
        else{
            assert.equal(c.expected.length,1);
            assert(got,c.name);
            assert(Math.abs(got.factor-c.expected[0][2])<1e-14,c.name);
            assert.deepEqual(got.anchor,[c.expected[0][0]/100,c.expected[0][1]/80]);
        }
    }
    console.log('GTK WHEEL ORACLE: ALL OK ('+cases.length+' actual GTK event cases; physical-device calibration unverified)');
}
console.log('WEB WHEEL POLICY: ALL OK (4% event cap, fractional deltas, all DOM units, invalid/zero/chords, device-aligned anchors)');
