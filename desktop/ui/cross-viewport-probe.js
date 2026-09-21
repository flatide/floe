/* Explicit synthetic comparison only. Normal UI sizing/DPR are not changed.
 * ResizeObserver and the product's ordinary resize request must do the work. */
function () {
    const v=document.getElementById('viewport'),dpr=devicePixelRatio;
    if(!v||!Number.isFinite(dpr)||dpr<0.25||dpr>4||
       !Number.isInteger(800*dpr)||!Number.isInteger(600*dpr)) {throw Error('Unsupported QA viewport');}
    Object.assign(v.style,{flex:'none',width:'800px',minWidth:'800px',maxWidth:'800px',
        height:'600px',minHeight:'600px',maxHeight:'600px',padding:'0',border:'0',boxSizing:'border-box'});
    return [800*dpr,600*dpr];
}
