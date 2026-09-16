/* ES2017. Owner invitations require explicit approval; no automatic issuance. */
(function(root){
    'use strict';
    function bind(o){
        const el=o.el,doc=o.document,panel=el('share-dialog'),button=el('share-open');
        let enabled=false,opened=false,busy=false,scope=null,generation=0,listing=0,prior=null,hidden=[],drc=null;
        function note(s){el('share-status').textContent=s;}
        function clearLink(){el('share-link').value='';el('share-result').hidden=true;el('share-visit').removeAttribute('href');}
        function current(){const c=o.context();return c&&scope&&c.view_id===scope.view_id&&c.state_rev===scope.state_rev&&c.source_id===scope.source_id;}
        function changed(){button.hidden=!enabled;button.disabled=!enabled||!o.context();
            el('share-create').disabled=!opened||busy||!current()||!el('share-consent').checked;
            el('share-refresh').disabled=busy;el('share-mode').disabled=busy;
            el('share-drc-consent').disabled=busy||!drc||!current();
            if(opened&&scope&&!current()){el('share-consent').checked=false;el('share-drc-consent').checked=false;note('View changed. Close and reopen this dialog to approve its current scope.');}
        }
        function close(restore){generation++;opened=false;scope=null;drc=null;busy=false;panel.hidden=true;clearLink();el('share-consent').checked=false;el('share-drc-consent').checked=false;button.setAttribute('aria-expanded','false');
            hidden.forEach(function(p){if(p[1]===null){p[0].removeAttribute('aria-hidden');}else{p[0].setAttribute('aria-hidden',p[1]);}});hidden=[];
            if(restore){(prior&&doc.contains(prior)?prior:button).focus();}prior=null;changed();}
        async function refresh(){if(!enabled||!opened||busy){return;}const token=generation,read=++listing;
            try{const v=await o.http('GET','/api/v1/shares');if(token!==generation||read!==listing||!opened){return;}
                if(!v||!Array.isArray(v.shares)||v.shares.length>4){throw Error('Invalid invitation list');}
                const rows=el('share-list');rows.textContent='';
                v.shares.forEach(function(s){if(!/^[0-9a-f]{64}$/.test(s.share_id)||!['follow','explore'].includes(s.mode)){throw Error('Invalid invitation');}
                    const row=doc.createElement('div'),text=doc.createElement('span'),revoke=doc.createElement('button');
                    text.textContent=s.mode+' · '+s.share_id.slice(0,12)+(s.drc?' · includes DRC result':' · layout only');revoke.textContent='Revoke';
                    revoke.onclick=async function(){if(!opened||busy||token!==generation){return;}busy=true;listing++;changed();revoke.disabled=true;
                        try{await o.http('DELETE','/api/v1/shares/'+s.share_id);if(token===generation){clearLink();note('Invitation/session revoked. Already received pixels cannot be recalled.');}}
                        catch(_){if(token===generation){note('Revocation unconfirmed. Refresh the list before retrying.');}}
                        finally{if(token===generation){busy=false;changed();refresh();}}};
                    row.className='button-row';row.appendChild(text);row.appendChild(revoke);rows.appendChild(row);
                });if(!v.shares.length){rows.textContent='No active invitations or guest sessions.';}
            }catch(_){if(token===generation&&read===listing){note('Could not refresh invitations. No command was retried.');}}
        }
        async function readDRC(){const token=generation;drc=null;el('share-drc').hidden=true;el('share-drc-consent').checked=false;el('share-drc-status').textContent='Checking optional DRC result…';
            try{const v=await o.http('GET','/api/v1/drc');if(token!==generation||!opened||!current()){return;}
                const d=v&&v.drc;
                if(d&&d.phase==='ready'&&d.source_id===scope.source_id&&typeof d.id==='string'&&/^[0-9a-f]{64}$/.test(d.id)&&typeof d.revision==='string'&&d.revision.length<=128&&d.metadata){
                    drc={id:d.id,revision:d.revision};el('share-drc-title').textContent='Optional DRC result: '+String(d.title)+' · '+d.metadata.checks+' rules · '+d.metadata.errors+' errors';el('share-drc').hidden=false;
                    el('share-drc-status').textContent='Unchecked: layout only. DRC requires its separate whole-result approval.';
                }else{el('share-drc-status').textContent='No ready DRC result is bound to this source. Invitation will contain layout only.';}
            }catch(_){if(token===generation&&opened){el('share-drc-status').textContent='DRC unavailable. Layout-only sharing remains available; reopen to check DRC again.';}}
            finally{if(token===generation){changed();}}
        }
        function open(){if(!enabled||opened){return;}const c=o.context();if(!c){return;}opened=true;scope={view_id:c.view_id,state_rev:c.state_rev,source_id:c.source_id};generation++;prior=doc.activeElement;
            panel.hidden=false;clearLink();note('Opening this dialog does not create an invitation.');el('share-consent').checked=false;el('share-mode').value='follow';button.setAttribute('aria-expanded','true');
            ['app-header','app-workspace'].forEach(function(id){const n=el(id);hidden.push([n,n.getAttribute('aria-hidden')]);n.setAttribute('aria-hidden','true');});
            el('share-cancel').focus();changed();refresh();readDRC();}
        button.onclick=open;el('share-cancel').onclick=function(){close(true);};el('share-refresh').onclick=refresh;el('share-consent').onchange=changed;
        el('share-create').onclick=async function(){if(!enabled||!opened||busy||!current()||!el('share-consent').checked){return;}
            const mode=el('share-mode').value;if(!['follow','explore'].includes(mode)){return;}const token=generation;
            busy=true;listing++;clearLink();changed();const request={view_id:scope.view_id,base_state_rev:scope.state_rev,mode:mode,approve:true};
            if(el('share-drc-consent').checked){if(!drc){busy=false;changed();return;}request.drc={id:drc.id,revision:drc.revision,approve:true};}
            try{const v=await o.http('POST','/api/v1/shares',request);if(token!==generation||!opened){return;}
                if(!v||!['share_id','invite'].every(function(k){return /^[0-9a-f]{64}$/.test(v[k]);})||v.mode!==mode||
                    (request.drc?(!v.drc||v.drc.id!==request.drc.id||v.drc.revision!==request.drc.revision):!!v.drc)){throw Error('Invalid invitation response');}
                const url=o.origin+'/guest/'+v.share_id+'#invite='+v.invite;
                el('share-link').value=url;el('share-visit').setAttribute('href',url);el('share-result').hidden=false;
                note('Created. Exchange once within '+v.invite_seconds+' seconds; the resulting guest session lasts at most '+v.session_seconds+' seconds. Do not log this credential.');
            }catch(_){if(token===generation){note('Creation failed or its outcome is unknown. Refresh and revoke any unwanted entry before creating another. No automatic retry.');}}
            finally{if(token===generation){busy=false;el('share-consent').checked=false;el('share-drc-consent').checked=false;changed();refresh();}}
        };
        panel.addEventListener('click',function(e){if(e.target===panel){close(true);}});
        doc.addEventListener('keydown',function(e){if(!opened){return;}e.stopPropagation();if(e.isComposing){return;}if(e.key==='Escape'){e.preventDefault();close(true);return;}
            if(e.key==='Tab'){const items=Array.prototype.slice.call(panel.querySelectorAll('button,input,select,a')).filter(function(n){return !n.disabled&&!n.hidden&&(!n.closest||!n.closest('[hidden]'));}),i=items.indexOf(doc.activeElement);
                if(items.length&&(i<0||e.shiftKey&&i===0||!e.shiftKey&&i===items.length-1)){e.preventDefault();items[e.shiftKey?items.length-1:0].focus();}}
        },true);
        doc.addEventListener('focusin',function(e){if(opened&&!panel.contains(e.target)){el('share-cancel').focus();}},true);
        return {init:function(on){enabled=on===true;changed();},changed:changed,stop:function(){enabled=false;close(false);},suspend:function(){close(false);}};
    }
    const api={bind:bind};if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeSharing=api;}
}(typeof window==='object'?window:this));
