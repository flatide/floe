/* Fixed, empty-workspace native QA only; never read or return credentials. */
function (stage) {
    'use strict';
    const key='floe.desktop.recovery.probe',flag='__floeDesktopRecoveryProbe';
    try {
        if(location.hash){return 'recovery-failed';}
        const logout=document.getElementById('logout');
        if(!logout||logout.disabled){return 'wait';}
        if(stage==='arm'){
            if(sessionStorage.getItem(key)!==null||Object.prototype.hasOwnProperty.call(window,flag)){
                return 'recovery-failed';
            }
            sessionStorage.setItem(key,'kept');window[flag]=true;return 'armed';
        }
        if(stage!=='check'){return 'recovery-failed';}
        if(window[flag]===true){return 'wait';}
        if(sessionStorage.getItem(key)!=='kept'){return 'recovery-failed';}
        const picker=document.getElementById('browse-dialog'),close=document.getElementById('browse-close');
        if(!picker||picker.hidden||!close||close.disabled){return 'wait';}
        sessionStorage.removeItem(key);return 'recovered';
    }catch(_){return 'recovery-failed';}
}
