/* ES2017. Read-only receipt selection; never implicitly build or switch. */
(function(root){'use strict';
    function selection(s){
        if(s&&s.mode==='all'){return 'all';}
        if(!s||s.mode!=='only'||!Array.isArray(s.ids)||!s.ids.length||s.ids.length>4096||
            !s.ids.every(function(n){return typeof n==='string'&&/^-?(0|[1-9][0-9]{0,18})$/.test(n)&&n!=='-0';})||new Set(s.ids).size!==s.ids.length){throw Error('Invalid revision level selection');}
        return s.ids.slice().sort().join(',');
    }
    function candidate(history){
        const list=(history||[]).filter(function(v){return v.kind==='check_revision';});
        const v=list[list.length-1];
        if(!v||v.phase!=='succeeded'||typeof v.source_id!=='string'||!/^[0-9a-f]{64}$/.test(v.source_id)||
            typeof v.index_revision!=='string'||!/^[0-9a-f]{32}$/.test(v.index_revision)){return null;}
        try{selection(v.levels);}catch(_){return null;} return v;
    }
    function matches(v,source,levels){return !!v&&v.source_id===source&&selection(v.levels)===selection(levels);}
    function useRequest(v,source,levels,mode,pixels,current){
        if(!matches(v,source,levels)){throw Error('Check the published revision for this exact source and level selection first.');}
        if(current&&(current.source_id!==source||current.mode!==mode||selection(current.levels) !== selection(levels))){
            throw Error('Close the current view before changing source, loaded levels or mode.');
        }
        return {kind:'use_revision',source_id:source,levels:levels,mode:mode,revision:v.index_revision,approved:true,pixels:pixels,
            target:current?{kind:'replace',view_id:current.view_id,state_rev:current.state_rev}:{kind:'empty'}};
    }
    function usage(history){
        const list=(history||[]).filter(function(v){return v.kind==='revision_usage';}),v=list[list.length-1];
        if(!v||v.phase!=='succeeded'||typeof v.source_id!=='string'||!/^[0-9a-f]{64}$/.test(v.source_id)){return null;}
        const inv=v.inventory,bytes=function(n){return typeof n==='string'&&/^(0|[1-9][0-9]{0,19})$/.test(n);};
        if(!inv||!bytes(inv.logical_bytes)||!Array.isArray(inv.rows)||inv.rows.length>256){return null;}
        const lines=[(inv.partial?'PARTIAL — observed ':'Observed ')+inv.logical_bytes+' logical file bytes; '+inv.stores_scanned+' stores.',
            'Unknown entries: '+inv.unknown_entries+'; unavailable: '+inv.unavailable_entries+'. No deletion performed.'];
        for(let i=0;i<inv.rows.length;i++){
            const r=inv.rows[i];
            if(!r||!/^[0-9a-f]{32}$/.test(r.revision)||!bytes(r.logical_bytes)){return null;}
            lines.push((r.kind==='set'?'set':'source #'+r.source_number)+' '+r.revision+' — '+r.logical_bytes+' bytes; '+
                (r.current?'CURRENT; ':'')+(r.current_unknown?'CURRENT UNKNOWN; ':'')+'format '+(r.format||'?')+'; '+r.seal+'; '+r.readers+'; '+r.owner+
                (r.set_revision?' (set '+r.set_revision+')':'')+(r.extra_entries?'; extra entries — protected':''));
        }
        const recoveries=inv.recoveries||[],choices=new Map();
        if(!Array.isArray(recoveries)||recoveries.length>256||!recoveries.every(id)){return null;}
        inv.rows.filter(function(r){return r.kind==='set';}).forEach(function(r){choices.set(r.revision,{revision:r.revision,label:r.revision+(r.current?' — CURRENT, protected':' — format '+r.format),current:!!r.current});});
        recoveries.forEach(function(r){choices.set(r,{revision:r,label:r+' — recovery evidence, unverified',current:choices.has(r)&&choices.get(r).current});});
        if(recoveries.length){lines.push('Recovery evidence (may be incomplete, invalid or already complete):\n'+recoveries.join('\n'));}
        lines.push('Not allocated/freeable space. Idle is only a scan-time observation. Legacy, incomplete and unknown revisions remain protected. Refresh to update.');
        return {source_id:v.source_id,text:lines.join('\n'),choices:Array.from(choices.values())};
    }
    function id(v){return typeof v==='string'&&/^[0-9a-f]{32}$/.test(v);}
    function token(v){return typeof v==='string'&&/^[0-9a-f]{64}$/.test(v);}
    function bytes(v){return typeof v==='string'&&/^(0|[1-9][0-9]{0,19})$/.test(v)&&(v.length<20||v<='18446744073709551615');}
    function count(v,max){return Number.isInteger(v)&&v>=0&&v<=max;}
    function reclaimCandidate(history){
        // The server invalidates the preview on ANY subsequently accepted
        // operation, not merely on another reclamation. GET never approves.
        const r=(history||[]).slice(-1)[0],p=r&&r.preview;
        if(!r||r.kind!=='prepare_reclaim'||r.phase!=='succeeded'||!token(r.source_id)||!p||
            !id(p.revision)||!token(p.token)||!bytes(p.logical_bytes)||!count(p.files,7170)||!count(p.sources,1024)||p.sources===0||
            typeof p.complete!=='boolean'||typeof p.recovery!=='boolean'||!count(p.expires_in_s,300)||p.expires_in_s===0||
            Object.keys(p).sort().join(',')!=='complete,expires_in_s,files,logical_bytes,recovery,revision,sources,token'||
            p.complete&&(p.files!==0||p.logical_bytes!=='0'||!p.recovery)){return null;}
        return r;
    }
    function reclaimRequest(r,source,revision,approved){
        if(!approved||!r||reclaimCandidate([r])!==r||r.source_id!==source||r.preview.revision!==revision||r.preview.complete){
            throw Error('Prepare this exact source/set again and explicitly approve the listed deletion.');
        }
        return {kind:'reclaim_revision',source_id:source,revision:revision,token:r.preview.token,approved:true};
    }
    function reclaimText(history){
        const r=(history||[]).filter(function(v){return v.kind==='prepare_reclaim'||v.kind==='reclaim_revision';}).slice(-1)[0];
        if(!r){return 'No reclamation preview. No automatic cleanup.';}
        if(r.kind==='prepare_reclaim'&&reclaimCandidate([r])){
            const p=r.preview;
            return p.complete?'Recovery verified: already complete; no files left. No deletion is needed.':
                (p.recovery?'Recovery preview':'Deletion preview')+' · set '+p.revision+' · '+p.files+' files / '+p.logical_bytes+' logical bytes / '+p.sources+' sources.\n'+
                'These files and their empty revision directories will be removed irreversibly. Approval expires 5 minutes after server preparation; the server rechecks current/readers/files. Other operations invalidate this preview.';
        }
        const o=r.outcome;
        if(r.kind==='reclaim_revision'&&['succeeded','incomplete'].includes(r.phase)&&o&&id(o.revision)&&
            ['complete','interrupted','outcome_unknown'].includes(o.status)&&bytes(o.removed_logical_bytes)&&count(o.removed_files,7170)&&typeof o.sync_warning==='boolean'&&
            Object.keys(o).sort().join(',')==='removed_files,removed_logical_bytes,revision,status,sync_warning'){
            return 'Set '+o.revision+' · '+o.status+' · acknowledged deletion: '+o.removed_files+' files / '+o.removed_logical_bytes+' logical bytes.'+
                (o.sync_warning?' Sync warning: crash durability unconfirmed.':'')+
                (o.status==='complete'?' Current/new revisions preserved.':' Actual remaining files must be checked. Refresh usage, prepare the same set again, then explicitly approve recovery. No automatic retry.');
        }
        return 'Reclamation '+String(r.phase||'unavailable')+'. No automatic retry; refresh usage and prepare again. Busy/current, old-format, unpublished, changed or network-filesystem targets remain protected.';
    }
    root.FloeIndexRevisions={selection:selection,candidate:candidate,matches:matches,useRequest:useRequest,usage:usage,id:id,reclaimCandidate:reclaimCandidate,reclaimRequest:reclaimRequest,reclaimText:reclaimText};
    if(typeof module==='object'&&module.exports){module.exports=root.FloeIndexRevisions;}
}(typeof window==='object'?window:globalThis));
