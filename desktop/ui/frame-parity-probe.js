/* Explicit synthetic QA only. Compare retained Canvas pixels, not screenshots,
 * so browser chrome, CSS scaling and different capture times are excluded. */
function () {
    const f=document.getElementById('canvas'),m=document.getElementById('margin-canvas');
    if(document.hidden||!f||!m||m.hidden||!f.dataset.frameId||!m.dataset.frameId||
       !f.dataset.renderRev||f.dataset.renderRev!==m.dataset.renderRev){throw Error('Unmatched synthetic frames');}
    const fb=JSON.parse(f.dataset.bboxDbu).map(Number),mb=JSON.parse(m.dataset.bboxDbu).map(Number);
    if(fb.length!==4||mb.length!==4){throw Error('Invalid synthetic bounds');}
    const fs=[(fb[2]-fb[0])/f.width,(fb[3]-fb[1])/f.height];
    const ms=[(mb[2]-mb[0])/m.width,(mb[3]-mb[1])/m.height];
    const raw=[(fb[0]-mb[0])/ms[0],(mb[3]-fb[3])/ms[1]],off=raw.map(Math.round);
    if(!fb.concat(mb,fs,ms,raw).every(Number.isFinite)||fs.some((v,i)=>v<=0||Math.abs(v/ms[i]-1)>1e-9)||
       off.some((v,i)=>v<0||v%16!==0||Math.abs(raw[i]-v)>1e-3)||
       off[0]+f.width>m.width||off[1]+f.height>m.height||f.width*f.height>16*1024*1024){throw Error('Unaligned synthetic frames');}
    const a=f.getContext('2d').getImageData(0,0,f.width,f.height).data;
    const b=m.getContext('2d').getImageData(off[0],off[1],f.width,f.height).data;
    let changed=0,foregroundOnly=0,marginOnly=0,maxDelta=0,foregroundLit=0,marginLit=0;
    const bounds=[f.width,f.height,-1,-1],examples=[];
    for(let i=0;i<a.length;i+=4){
        const alit=!!(a[i]||a[i+1]||a[i+2]),blit=!!(b[i]||b[i+1]||b[i+2]);
        if(alit){foregroundLit++;}if(blit){marginLit++;}
        if(a[i]===b[i]&&a[i+1]===b[i+1]&&a[i+2]===b[i+2]&&a[i+3]===b[i+3]){continue;}
        changed++;
        const x=(i/4)%f.width,y=Math.floor(i/4/f.width);
        if(examples.length<8){examples.push({x,y,a:Array.from(a.slice(i,i+4)),b:Array.from(b.slice(i,i+4))});}
        bounds[0]=Math.min(bounds[0],x);bounds[1]=Math.min(bounds[1],y);
        bounds[2]=Math.max(bounds[2],x);bounds[3]=Math.max(bounds[3],y);
        if(alit&&!blit){foregroundOnly++;}if(blit&&!alit){marginOnly++;}
        for(let c=0;c<4;c++){maxDelta=Math.max(maxDelta,Math.abs(a[i+c]-b[i+c]));}
    }
    return {pixels:[f.width,f.height],offset:off,bbox:fb,margin_bbox:mb,compared_pixels:f.width*f.height,changed_pixels:changed,
        foreground_only:foregroundOnly,margin_only:marginOnly,foreground_lit:foregroundLit,margin_lit:marginLit,
        max_channel_delta:maxDelta,bounds:changed?bounds:null,examples};
}
