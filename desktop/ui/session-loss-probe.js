/* Fixed empty-workspace QA only. No storage value or credential leaves JS. */
function (stage) {
    'use strict';
    try {
        const logout=document.getElementById('logout');
        if(location.hash||!logout){return 'wait';}
        if(typeof window.__floeSessionLossCounts!=='function'){return 'loss-failed-observer';}
        const c=window.__floeSessionLossCounts();
        if(stage==='ready'||stage==='erase-storage'){
            if(logout.disabled){return 'wait';}
            if(c.exchanges!==1){return 'loss-failed-exchange';}
            if(c.writes!==0){return 'loss-failed-write';}
            const picker=document.getElementById('browse-dialog'),close=document.getElementById('browse-close');
            if(!picker||picker.hidden||!close||close.disabled){return 'wait';}
            // Empty startup opens its picker and admits one read-only listing
            // through the queued browse POST protocol. No selection is clicked.
            if(c.browsePosts===0){return 'wait';}
            if(c.browsePosts!==1){return 'loss-failed-write';}
            if(stage==='erase-storage'){
                // Remove only this newly created session's exact key; no reads,
                // enumeration, clear-all, pending journal or profile operations.
                sessionStorage.removeItem('floe-session:'+location.origin);
                return 'loss-erased';
            }
            return 'loss-ready';
        }
        if(stage!=='check-storage'&&stage!=='check-cookie'){return 'loss-failed';}
        if(c.exchanges!==0||c.browsePosts!==0||c.writes!==0){return 'loss-failed-replay';}
        const state=document.getElementById('connection');
        if(!logout.disabled||!state||state.getAttribute('data-session-state')!=='restart-required'){return 'wait';}
        // Fixed UI outcomes distinguish a real 401 (cookie missing, stored auth
        // still usable for the GET) from an absent session key (no auth GET).
        const expected=stage==='check-cookie'?'Session expired':'Not connected';
        return state.textContent===expected?'loss-confirmed':'loss-failed';
    }catch(_){return 'loss-failed-exception';}
}
