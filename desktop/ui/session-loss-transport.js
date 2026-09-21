/* Document-start, empty QA WebView only. Counts request paths/methods, never
 * reads bodies, headers, cookies, session storage or authentication values. */
(function () {
    'use strict';
    const XHR=window.XMLHttpRequest,open=XHR.prototype.open,send=XHR.prototype.send;
    const requests=new WeakMap(),counts={exchanges:0,browsePosts:0,writes:0};
    XHR.prototype.open=function(method,url){
        const target=new URL(url,location.href),verb=String(method).toUpperCase();
        requests.set(this,target.origin!==location.origin||['GET','HEAD'].includes(verb)?null:
            verb==='POST'&&target.pathname==='/api/v1/session/exchange'?'exchanges':
            verb==='POST'&&target.pathname==='/api/v1/browse'?'browsePosts':'writes');
        return open.apply(this,arguments);
    };
    XHR.prototype.send=function(){
        const key=requests.get(this);if(key){++counts[key];}
        return send.apply(this,arguments);
    };
    window.__floeSessionLossCounts=function(){return {exchanges:counts.exchanges,browsePosts:counts.browsePosts,writes:counts.writes};};
}());
