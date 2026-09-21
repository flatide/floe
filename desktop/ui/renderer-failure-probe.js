/* Fixed DOM markers only, in the no-argument synthetic renderer QA session. */
function (mode) {
    'use strict';
    const el=function(id){return document.getElementById(id);};
    const status=el('status'),notice=el('notice'),canvas=el('canvas'),margin=el('margin-canvas');
    if(!status||!notice||!canvas||!margin||!el('logout')||el('logout').disabled){return 'wait';}
    if(mode==='ready'){
        // Force a real landed margin, the case that formerly hid failure under Live.
        if(document.hidden){return 'renderer-document-hidden';}
        if(!margin.dataset.frameId){return 'renderer-await-margin';}
        if(margin.hidden){return 'renderer-margin-hidden';}
        if(!/Live.*margin crop/.test(status.textContent)){return 'renderer-await-live-crop';}
        return !margin.hidden&&!!margin.dataset.frameId&&/Live.*margin crop/.test(status.textContent)&&
            !el('fit').disabled&&!el('close').disabled&&el('empty').hidden?'renderer-ready':'wait';
    }
    if(mode==='failed'){
        if(!/renderer failed/i.test(notice.textContent)){return 'wait';}
        return /^failed/.test(status.textContent)&&status.textContent.includes('last displayed image')&&
            el('fit').disabled&&el('zoom-in').disabled&&!el('close').disabled&&
            el('rendering').hidden&&el('empty').hidden?'renderer-failed':'renderer-status-invalid';
    }
    if(mode==='closed'){
        return status.textContent==='View closed'&&canvas.width===1&&margin.width===1&&
            !canvas.dataset.frameId&&!margin.dataset.frameId&&!el('empty').hidden&&
            el('close').disabled&&!el('open').disabled?'renderer-closed':'wait';
    }
    return 'wait';
}
