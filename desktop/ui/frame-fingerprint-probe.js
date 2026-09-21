/* Raw foreground and margin-crop fingerprints from the same visible revision.
 * No PNG decoder, screenshot/resampling, UI chrome or serialized credentials. */
async function (comparison) {
    const f=document.getElementById('canvas'),m=document.getElementById('margin-canvas');
    const ids=[f.dataset.frameId,m.dataset.frameId,f.dataset.renderRev,m.dataset.renderRev];
    const bbox=f.dataset.bboxDbu,marginBbox=m.dataset.bboxDbu;
    if(document.hidden||m.hidden||!ids.every(Boolean)||ids[2]!==ids[3]||
       JSON.stringify(JSON.parse(bbox).map(Number))!==JSON.stringify(comparison.bbox)||
       JSON.stringify(JSON.parse(marginBbox).map(Number))!==JSON.stringify(comparison.margin_bbox)||
       f.width!==comparison.pixels[0]||f.height!==comparison.pixels[1]||f.width*f.height>16*1024*1024||
       comparison.offset.length!==2||comparison.offset.some(n=>!Number.isSafeInteger(n)||n<0)||
       comparison.offset[0]+f.width>m.width||comparison.offset[1]+f.height>m.height) {
        throw Error('Mismatched fingerprint frame');
    }
    const foreground=f.getContext('2d').getImageData(0,0,f.width,f.height).data;
    const margin=m.getContext('2d').getImageData(...comparison.offset,f.width,f.height).data;
    const hashes=[];
    for(const data of [foreground,margin]) {
        const digest=await crypto.subtle.digest('SHA-256',data);
        hashes.push(Array.from(new Uint8Array(digest),b=>b.toString(16).padStart(2,'0')).join(''));
    }
    if(document.hidden||m.hidden||bbox!==f.dataset.bboxDbu||marginBbox!==m.dataset.bboxDbu||
       ids.some((id,i)=>id!==[f.dataset.frameId,m.dataset.frameId,f.dataset.renderRev,m.dataset.renderRev][i])) {
        throw Error('Fingerprint frame changed during digest');
    }
    return [...comparison.pixels,devicePixelRatio,...comparison.bbox,...hashes].join(' ');
}
