/* ES2017. Read-only modal, independent of layout/view and render generations. */
(function(root) {
    'use strict';
    const N=typeof module==='object'&&module.exports?require('./notices.js'):root.FloeNotices;
    function parse(v,bundle) {
        function text(s,max){return typeof s==='string'&&s.length>0&&s.length<=max;}
        if(!v||v.product!=='floe2-web'||v.bundle!==bundle||v.python_runtime!==false||
            v.desktop_acceptance!=='unverified'||!v.notices||v.notice_scope!==(v.notices.status==='available'?'portable_manifest':'embedded_font_only')||
            v.font_name!=='Noto Sans Mono'||!text(v.font_notice,16384)){throw new Error('Invalid About response');}
        N.metadata(v.notices);
        if(v.build!==null){
            if(!v.build||['app_version','source_revision','target','index_compatibility','renderd_compatibility'].some(function(k){
                return !text(v.build[k],128)||!/^[\x21-\x7e]+$/.test(v.build[k]);
            })){throw new Error('Invalid build identity');}
        }
        return v;
    }
    function describe(v) {
        const b=v.build;
        return (b?['App: '+b.app_version,'Source revision: '+b.source_revision,'Target: '+b.target,
            'Expected index compatibility: '+b.index_compatibility,'Expected renderd compatibility: '+b.renderd_compatibility]:
            ['Launcher build identity unavailable.'])
            .concat(['Web bundle: '+v.bundle,'Python runtime: not required','Desktop acceptance: unverified']).join('\n');
    }
    function bind(o) {
        const el=o.el,doc=o.document,panel=el('about-dialog'),button=el('about-open');
        const notices=N.bind({el:el,document:doc,http:o.http});
        let enabled=false,opened=false,task=null,prior=null,hidden=[];
        function cancel(){if(task){task.cancelled=true;if(task.abort){task.abort();}task=null;}}
        function close(restore) {
            cancel();notices.close();if(!opened){return;}opened=false;panel.hidden=true;button.setAttribute('aria-expanded','false');
            hidden.forEach(function(p){if(p[1]===null){p[0].removeAttribute('aria-hidden');}else{p[0].setAttribute('aria-hidden',p[1]);}});hidden=[];
            if(restore){const target=prior&&doc.contains(prior)?prior:button;target.focus();}prior=null;
        }
        async function open() {
            if(!enabled||opened){return;}opened=true;prior=doc.activeElement;panel.hidden=false;button.setAttribute('aria-expanded','true');
            el('about-build').textContent='';el('about-font').textContent='';el('about-status').textContent='Reading build identity…';el('about-close').focus();
            ['app-header','app-workspace'].forEach(function(id){const n=el(id);hidden.push([n,n.getAttribute('aria-hidden')]);n.setAttribute('aria-hidden','true');});
            const t={cancelled:false,abort:null};task=t;
            try{const v=parse(await o.http('GET','/api/v1/about',undefined,false,t),o.bundle);
                if(t.cancelled||task!==t||!opened){return;}
                el('about-build').textContent=describe(v);el('about-font').textContent=v.font_notice;el('about-status').textContent='Read-only · no render or selfcheck was started.';
                notices.open(v.notices);
            }catch(e){if(!t.cancelled&&task===t&&opened){el('about-status').textContent='About could not be read. Close and reopen to retry. '+e.message;}}
            finally{if(task===t){task=null;}}
        }
        button.onclick=open;el('about-close').onclick=function(){close(true);};
        panel.addEventListener('click',function(e){if(e.target===panel){close(true);}});
        doc.addEventListener('keydown',function(e){
            if(!opened){return;}e.stopPropagation();
            if(e.key==='Escape'){e.preventDefault();close(true);return;}
            if(e.key==='Tab'){
                const items=Array.from(panel.querySelectorAll('button, input, [tabindex="0"]')).filter(function(n){return !n.disabled&&n.getClientRects().length>0;});
                const index=items.indexOf(doc.activeElement);
                if(index<0||e.shiftKey&&index===0||!e.shiftKey&&index===items.length-1){
                    e.preventDefault();items[e.shiftKey?items.length-1:0].focus();
                }
            }
        },true);
        doc.addEventListener('focusin',function(e){if(opened&&!panel.contains(e.target)){el('about-close').focus();}},true);
        return {init:function(){enabled=true;button.disabled=false;},stop:function(){enabled=false;button.disabled=true;close(false);}};
    }
    const api={parse:parse,describe:describe,bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeAbout=api;}
}(typeof window!=='undefined'?window:this));
