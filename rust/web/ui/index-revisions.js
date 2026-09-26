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
    root.FloeIndexRevisions={selection:selection,candidate:candidate,matches:matches,useRequest:useRequest};
    if(typeof module==='object'&&module.exports){module.exports=root.FloeIndexRevisions;}
}(typeof window==='object'?window:globalThis));
