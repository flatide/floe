/* ES2017, tab-local permission to submit CONFIRMED selection edits. This is
 * not server authorization or background autosave; never serialize the grant. */
(function(root) {
    'use strict';
    function bind(o){
        let reviewer='',scope='',ready=false,grant=null;
        o.toggle.checked=false;
        function paint(){
            o.toggle.checked=!!grant;
            // Opt-out remains available while a read/prepare/submit is pending.
            o.toggle.disabled=!reviewer||(!grant&&!ready);
            const message=reviewer?'Reviewer: '+reviewer+' · '+(grant?
                'confirmed '+o.kind+' edits save automatically in this tab. Unchecking stops unsent edits, not committed saves.':
                'automatic save is off. Each edit needs preview and approval.'):
                'A launcher-enabled reviewer is required. This setting does not grant file permissions.';
            if(o.hint.textContent!==message){o.hint.textContent=message;}
        }
        o.toggle.onchange=function(){
            grant=o.toggle.checked&&ready&&reviewer?{}:null;
            paint();o.changed();
        };
        paint();
        return {
            sync:function(owner,session,available){
                if(owner!==reviewer||session!==scope){grant=null;}
                reviewer=owner;scope=session;ready=available===true;paint();
            },
            on:function(){return !!grant;},
            capture:function(){return ready?grant:null;},
            valid:function(value){return ready&&!!value&&value===grant;},
            reset:function(){grant=null;paint();}
        };
    }
    const api={bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeReviewSaveMode=api;}
}(typeof window==='object'?window:this));
