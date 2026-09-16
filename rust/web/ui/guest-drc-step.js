/* Bounded guest traversal. Continuations require an explicit user action. */
(function(root){
    'use strict';
    function bounds(value,P){
        if(!Array.isArray(value)||value.length!==4){throw Error('Invalid step bounds');}
        const b=value.map(function(n){return Number(P.decimal(n));});
        if(b[0]>b[2]||b[1]>b[3]){throw Error('Invalid step bounds');}return b;
    }
    function decode(page,request,P,record,contains){
        if(!page||request.kind!=='filtered_step'||page.hit===undefined||page.next===undefined||
            page.selection_rev!==request.selection_rev){throw Error('Invalid guest step response');}
        P.counter(page.scanned,true);
        if(P.compare(page.scanned,'262144')>0||(page.hit!==null&&page.next!==null)){throw Error('Invalid step limit');}
        let area=null;
        if(request.in_view){area=bounds(page.bbox_um,P);}else if(page.bbox_um!==null){throw Error('Unexpected step bounds');}
        if(page.next!==null){
            if(request.selection_rev!==null||page.scanned==='0'){throw Error('Invalid step continuation');}
            P.counter(page.next.next,true);P.counter(page.next.remaining);
            if(request.cursor&&P.compare(page.next.remaining,request.cursor.remaining)>=0){throw Error('Step cursor did not progress');}
            return {hit:null,continuation:Object.assign({},request,{after:null,cursor:{next:page.next.next,remaining:page.next.remaining}})};
        }
        if(page.hit===null){return {hit:null,continuation:null};}
        const hit=record(page.hit),b=bounds(hit.bbox_um,P);
        if(hit.check!==request.check||request.waived!==null&&(hit.status===1)!==request.waived||
            request.selection_rev!==null&&!contains(hit)||
            area&&(area[0]>b[2]||area[2]<b[0]||area[1]>b[3]||area[3]<b[1])){throw Error('Step filter mismatch');}
        return {hit:hit,continuation:null};
    }
    const api={decode:decode};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeGuestDRCStep=api;}
}(typeof window==='object'?window:this));
