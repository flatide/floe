/* Product readiness only: no authentication, storage or page text is read. */
function () {
    const s=document.getElementById('connection');
    if(s&&s.getAttribute('data-session-state')==='restart-required'){return 'restart-required';}
    const b=document.getElementById('logout');
    if(b&&!b.disabled){return document.hidden?'ready-hidden':'ready';}
    return document.hidden?'hidden':'waiting';
}
