/* ES2017. Confirmation only; the existing session endpoint owns shutdown. */
(function (root) {
    'use strict';
    function bind(o) {
        const el=o.el,doc=o.document,panel=el('session-exit-dialog'),button=el('logout');
        let enabled=false,opened=false,committed=false,prior=null,hidden=[];
        function close(restore) {
            if(!opened){return;}opened=false;panel.hidden=true;button.setAttribute('aria-expanded','false');
            hidden.forEach(function(p){if(p[1]===null){p[0].removeAttribute('aria-hidden');}else{p[0].setAttribute('aria-hidden',p[1]);}});hidden=[];
            if(restore){(prior&&doc.contains(prior)?prior:button).focus();}prior=null;
        }
        function open() {
            if(!enabled||committed||opened){return;}opened=true;prior=doc.activeElement;
            panel.hidden=false;button.setAttribute('aria-expanded','true');
            el('session-exit-cancel').focus();
            ['app-header','app-workspace'].forEach(function(id){const n=el(id);hidden.push([n,n.getAttribute('aria-hidden')]);n.setAttribute('aria-hidden','true');});
        }
        function stop(){enabled=false;button.disabled=true;close(false);}
        button.onclick=open;el('session-exit-cancel').onclick=function(){close(true);};
        el('session-exit-confirm').onclick=function(){
            if(!opened||!enabled||committed){return;}committed=true;stop();return o.confirm();
        };
        panel.addEventListener('click',function(e){if(e.target===panel){close(true);}});
        doc.addEventListener('keydown',function(e){
            if(!opened){return;}e.stopPropagation();if(e.isComposing){return;}
            if(e.key==='Escape'){e.preventDefault();close(true);return;}
            if(e.key==='Tab'){
                const first=el('session-exit-cancel'),last=el('session-exit-confirm');
                if(doc.activeElement!==first&&doc.activeElement!==last||e.shiftKey&&doc.activeElement===first||!e.shiftKey&&doc.activeElement===last){
                    e.preventDefault();(e.shiftKey?last:first).focus();
                }
            }
        },true);
        doc.addEventListener('focusin',function(e){if(opened&&!panel.contains(e.target)){el('session-exit-cancel').focus();}},true);
        return {open:open,stop:stop,init:function(){if(!committed){enabled=true;button.disabled=false;}}};
    }
    const api={bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeSessionExit=api;}
}(typeof window!=='undefined'?window:this));
