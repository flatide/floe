/* Explicit synthetic native QA only. No file/clipboard/storage or network API.
 * Called repeatedly by the host, but starts exactly one asynchronous walk. */
function (compare, configure, fingerprint) {
    const key='__floeNativeLayoutParity';
    let q=window[key];
    if(!q) {
        q=window[key]={reports:[],done:false,failed:false,stage:'configure'};
        const e=id=>document.getElementById(id);
        function ready(previous,checkPosition=true) {
            const f=e('canvas'),m=e('margin-canvas');
            return !document.hidden&&!location.hash&&e('logout')&&!e('logout').disabled&&
                e('fit')&&!e('fit').disabled&&e('empty').hidden&&e('rendering').hidden&&
                /^Live.*margin crop/.test(e('status').textContent)&&!m.hidden&&
                f.dataset.frameId&&m.dataset.frameId&&f.dataset.renderRev&&
                f.dataset.renderRev===m.dataset.renderRev&&f.dataset.renderRev!==previous&&
                !/Prefetching/.test(e('margin-info').textContent)&&
                e('detail').value==='high'&&(!checkPosition||(Number(e('goto-width').value)===300&&
                    (!q.pixels||(Number(e('goto-x').value)===200&&Number(e('goto-y').value)===200))))&&
                (!q.pixels||(f.width===q.pixels[0]&&f.height===q.pixels[1]));
        }
        async function settled(previous,checkPosition=true) {
            const end=performance.now()+30000;
            while(performance.now()<end) {
                if(ready(previous,checkPosition)) {
                    await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
                    if(ready(previous,checkPosition)){return;}
                }
                await new Promise(resolve=>setTimeout(resolve,10));
            }
            throw Error('Synthetic frame deadline');
        }
        (async()=>{
            if(configure) {
                q.stage='initial';await settled();
                q.stage='configure';q.pixels=configure();
                // Product resize preserves scale, not the world width. Finish
                // the normal resize before one explicit QA goto; never race
                // startup's original goto or alter product resize semantics.
                q.stage='resize';await settled(undefined,false);
                if(Number(e('goto-width').value)!==300||Number(e('goto-x').value)!==200||Number(e('goto-y').value)!==200) {
                    q.stage='goto';const revision=e('canvas').dataset.renderRev;
                    e('goto-x').value='200';e('goto-y').value='200';e('goto-width').value='300';
                    if(e('goto').disabled){throw Error('QA goto unavailable');}
                    e('goto').click();await settled(revision);
                }
            }
            let previous;
            for(let phase=0;phase<3;phase++) {
                if(phase) {
                    previous=e('canvas').dataset.renderRev;
                    const toggle=e(phase===1?'labels':'frames');
                    toggle.checked=false;
                    toggle.dispatchEvent(new Event('change'));
                }
                q.stage='ready'+phase;
                await settled(previous);
                q.stage='compare'+phase;
                if(phase===0&&(!e('labels').checked||!e('frames').checked)) {throw Error('Wrong initial display');}
                const r=compare();
                if(!r.foreground_lit||!r.margin_lit||(phase&&r.changed_pixels!==0)) {throw Error('Synthetic geometry mismatch');}
                q.reports.push('layout-metric '+[phase,...r.pixels,devicePixelRatio,r.foreground_lit,r.margin_lit,r.changed_pixels].join(' '));
                if(fingerprint){q.stage='fingerprint'+phase;q.reports.push('layout-cross '+phase+' '+await fingerprint(r));}
            }
            q.done=true;
        })().catch(()=>{
            q.failed=true;
            try {
                const f=e('canvas'),m=e('margin-canvas');
                const flags=[!document.hidden,!location.hash,!e('logout').disabled,!e('fit').disabled,
                    e('empty').hidden,e('rendering').hidden,/^Live.*margin crop/.test(e('status').textContent),
                    !m.hidden,!!f.dataset.frameId,!!m.dataset.frameId,f.dataset.renderRev===m.dataset.renderRev,
                    !/Prefetching/.test(e('margin-info').textContent)];
                const mask=flags.reduce((n,b,i)=>n+(b?2**i:0),0);
                q.failureState='layout-state '+[mask,f.width,f.height,devicePixelRatio,
                    Number(e('goto-x').value),Number(e('goto-y').value),Number(e('goto-width').value)]
                    .map(n=>Number.isFinite(n)?n:-1).join(' ');
            } catch(_) { /* The fixed failure stage still makes this a failure. */ }
        });
    }
    if(q.failed&&q.failureState){const state=q.failureState;q.failureState=null;return state;}
    if(q.failed){return 'layout-failed-'+q.stage;}
    return q.reports.shift()||(q.done?'layout-ok':'wait');
}
