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
        lines.push('Not allocated/freeable space. Idle is only a scan-time observation. Legacy, incomplete and unknown revisions remain protected. Refresh to update.');
        return {source_id:v.source_id,text:lines.join('\n')};
    }
    root.FloeIndexRevisions={selection:selection,candidate:candidate,matches:matches,useRequest:useRequest,usage:usage};
    if(typeof module==='object'&&module.exports){module.exports=root.FloeIndexRevisions;}
}(typeof window==='object'?window:globalThis));
