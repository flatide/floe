/* Document-start injection ONLY in the no-argument synthetic native QA mode.
 * Never inspect request bodies, authentication headers, cookies or responses.
 * Drop one successful save ACK at the UI transport boundary, not the disk write.
 */
(function () {
    'use strict';
    const key='floe.desktop.review.qa',XHR=window.XMLHttpRequest;
    const open=XHR.prototype.open,send=XHR.prototype.send,requests=new WeakMap();
    let state;
    function persist(){sessionStorage.setItem(key,JSON.stringify(state));}
    function fail(){state.failed=true;persist();}
    try {
        const raw=sessionStorage.getItem(key);
        state=raw===null?{notes:0,waives:0,drop:'',dropped:'',failed:false}:JSON.parse(raw);
        if(!state||Object.keys(state).sort().join(',')!=='drop,dropped,failed,notes,waives'||
            !['notes','waives'].every(function(k){return Number.isInteger(state[k])&&state[k]>=0&&state[k]<=2;})||
            !['','notes','waives'].includes(state.drop)||!['','notes','waives'].includes(state.dropped)||
            typeof state.failed!=='boolean'){throw Error('Invalid QA state');}
        persist();
        XHR.prototype.open=function(method,url){
            const target=new URL(url,location.href);
            const kind=target.origin===location.origin&&method==='POST'&&
                /^\/api\/v1\/drc\/review\/(notes|waives)$/.exec(target.pathname);
            requests.set(this,kind?kind[1]:null);
            return open.apply(this,arguments);
        };
        XHR.prototype.send=function(){
            const kind=requests.get(this);
            if(kind){
                ++state[kind];if(state[kind]>2){fail();throw Error('Unexpected QA save');}persist();
                if(state.drop===kind){
                    state.drop='';persist();
                    const loaded=this.onload;
                    this.onload=function(event){
                        if(this.status>=200&&this.status<300&&typeof this.onerror==='function'){
                            state.dropped=kind;persist();this.onerror(new Event('error'));
                        }else{fail();if(loaded){loaded.call(this,event);}}
                    };
                }
            }
            return send.apply(this,arguments);
        };
        window.__floeReviewTransport={
            counts:function(){return {notes:state.notes,waives:state.waives,dropped:state.dropped,failed:state.failed};},
            arm:function(kind){
                if(!['notes','waives'].includes(kind)||state[kind]!==0||state.drop||state.failed){throw Error('QA arm refused');}
                state.drop=kind;state.dropped='';persist();
            }
        };
    }catch(_){window.__floeReviewTransport={counts:function(){return {failed:true};}};}
}());
