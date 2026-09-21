/* Fixed synthetic fixture only. DOM events exercise the real shipped UI and
 * Rust service; not OS keyboard/IME/crash injection or a general scripting API.
 * Return fixed markers only; no credentials, approval tokens or note data.
 */
function (stage) {
    'use strict';
    const el=function(id){return document.getElementById(id);};
    const shown=function(id){const n=el(id);return n&&!n.hidden;};
    const ready=function(id){const n=el(id);return n&&!n.disabled;};
    const text=function(id){const n=el(id);return n?n.textContent:'';};
    const note='Synthetic native recovery — 한글';
    try {
        if(location.hash||!ready('logout')){return 'wait';}
        const wire=window.__floeReviewTransport;
        if(!wire||wire.counts().failed){return 'review-failed';}
        if(text('notes-owner')!=='Reviewer: native-recovery-test'||
            text('waives-owner')!=='Reviewer: native-recovery-test'){return 'wait';}
        let local=window.__floeReviewProbe;
        if(!local||local.stage!==stage){local={stage:stage,step:0};window.__floeReviewProbe=local;}
        function counts(notes,waives){const s=wire.counts();if(s.failed||s.notes!==notes||s.waives!==waives){throw Error('QA count mismatch');}}
        function select(){
            const list=el('drc-errors');if(!list||list.children.length!==2){return false;}
            const b=list.children[0];if(b.getAttribute('aria-label')!=='Error 1, global 1'){throw Error('Fixture mismatch');}
            b.click();return true;
        }
        function saved(kind){return kind==='notes'?/Saved · #1\b/.test(text('notes-status')):
            /Save completed · #1\b/.test(text('waives-status'));}
        if(stage==='note-start'||stage==='waive-start'){
            const kind=stage==='note-start'?'notes':'waives';
            if(local.step===0){if(!select()){return 'wait';}local.step=1;return 'wait';}
            if(local.step===1){if(!ready(kind+'-read')){return 'wait';}el(kind+'-read').click();local.step=2;return 'wait';}
            if(local.step===2){
                if(!shown(kind+'-editor')||!ready(kind==='notes'?'notes-text':'waives-action')){return 'wait';}
                if(kind==='notes'){el('notes-text').value=note;el('notes-text').dispatchEvent(new Event('input',{bubbles:true}));}
                else{el('waives-action').value='waive';el('waives-action').dispatchEvent(new Event('change',{bubbles:true}));}
                if(!ready(kind+'-prepare')){return 'wait';}el(kind+'-prepare').click();local.step=3;return 'wait';
            }
            if(local.step===3){
                if(!shown(kind+'-review')||!ready(kind+'-consent')){return 'wait';}
                if(kind==='notes'?text('notes-preview')!==note:!/^Waive 1 selected errors/.test(text('waives-preview'))){throw Error('Preview mismatch');}
                wire.arm(kind);el(kind+'-consent').click();
                if(!ready(kind+'-approve')){throw Error('Approval unavailable');}
                el(kind+'-approve').click();local.step=4;return 'wait';
            }
            if(wire.counts().dropped===kind&&shown(kind+'-uncertain')&&saved(kind)){
                counts(kind==='notes'?1:2,kind==='notes'?0:1);
                return kind==='notes'?'note-lost':'waive-lost';
            }
            return 'wait';
        }
        if(stage==='note-check'||stage==='waive-check'){
            const kind=stage==='note-check'?'notes':'waives';
            if(local.step===0){
                counts(kind==='notes'?1:2,kind==='notes'?0:1);
                if(!shown(kind+'-uncertain')||!saved(kind)||!ready(kind+'-resolve')){return 'wait';}
                if(el('notes-autosave').checked||el('waives-autosave').checked){throw Error('Autosave persisted');}
                // Reload/status polling must not replay. This explicit action may
                // resend only the approved request preserved by the product UI.
                el(kind+'-resolve').click();local.step=1;return 'wait';
            }
            if(!shown(kind+'-uncertain')&&saved(kind)){
                counts(2,kind==='notes'?0:2);
                if(kind==='waives'&&(!/current review metadata matches/.test(text('waives-status'))||shown('waives-paused'))){return 'wait';}
                return kind==='notes'?'note-resolved':'waive-resolved';
            }
            return 'wait';
        }
        if(stage==='read-back'){
            counts(2,2);
            if(local.step===0){if(!select()){return 'review-wait-list';}local.step=1;return 'wait';}
            if(local.step===1){
                if(!ready('notes-read')){return 'review-wait-read';}
                el('notes-read').click();local.step=2;return 'wait';
            }
            if(local.step===2){
                if(!shown('notes-editor')||!ready('notes-text')){return 'review-wait-snapshot';}
                if(el('notes-text').value!==note){throw Error('Read-back mismatch');}
                el('notes-discard').click();local.step=3;return 'wait';
            }
            if(local.step===3){
                // These readers share bounded snapshot/HTTP resources. Release
                // the first snapshot before acquiring the second, as a user does.
                if(shown('notes-editor')||!ready('waives-read')||!ready('transfer-export')){return 'review-wait-read';}
                el('waives-read').click();local.step=4;return 'wait';
            }
            if(local.step===4){
                if(!shown('waives-editor')||!ready('waives-action')){return 'review-wait-snapshot';}
                if(!text('waives-target').includes('1 already waived · 0 reserved statuses')){throw Error('Read-back mismatch');}
                el('waives-discard').click();local.step=5;return 'wait';
            }
            if(!shown('waives-editor')){return 'review-ok';}
            return 'wait';
        }
        return 'review-failed';
    }catch(_){return 'review-failed';}
}
