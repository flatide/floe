/* Explicit synthetic native QA only. No file/clipboard/storage or network API.
 * Called repeatedly by the host, but starts exactly one asynchronous walk. */
function (compare) {
    const key='__floeNativeLayoutParity';
    let q=window[key];
    if(!q) {
        q=window[key]={reports:[],done:false,failed:false};
        const e=id=>document.getElementById(id);
        function ready(previous) {
            const f=e('canvas'),m=e('margin-canvas');
            return !document.hidden&&!location.hash&&e('logout')&&!e('logout').disabled&&
                e('fit')&&!e('fit').disabled&&e('empty').hidden&&e('rendering').hidden&&
                /^Live.*margin crop/.test(e('status').textContent)&&!m.hidden&&
                f.dataset.frameId&&m.dataset.frameId&&f.dataset.renderRev&&
                f.dataset.renderRev===m.dataset.renderRev&&f.dataset.renderRev!==previous&&
                !/Prefetching/.test(e('margin-info').textContent)&&
                e('detail').value==='high'&&Number(e('goto-width').value)===300;
        }
        async function settled(previous) {
            const end=performance.now()+30000;
            while(performance.now()<end) {
                if(ready(previous)) {
                    await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
                    if(ready(previous)){return;}
                }
                await new Promise(resolve=>setTimeout(resolve,10));
            }
            throw Error('Synthetic frame deadline');
        }
        (async()=>{
            let previous;
            for(let phase=0;phase<3;phase++) {
                if(phase) {
                    previous=e('canvas').dataset.renderRev;
                    const toggle=e(phase===1?'labels':'frames');
                    toggle.checked=false;
                    toggle.dispatchEvent(new Event('change'));
                }
                await settled(previous);
                if(phase===0&&(!e('labels').checked||!e('frames').checked)) {throw Error('Wrong initial display');}
                const r=compare();
                if(!r.foreground_lit||!r.margin_lit||(phase&&r.changed_pixels!==0)) {throw Error('Synthetic geometry mismatch');}
                q.reports.push('layout-metric '+[phase,...r.pixels,devicePixelRatio,r.foreground_lit,r.margin_lit,r.changed_pixels].join(' '));
            }
            q.done=true;
        })().catch(()=>{q.failed=true;});
    }
    if(q.failed){return 'layout-failed';}
    return q.reports.shift()||(q.done?'layout-ok':'wait');
}
