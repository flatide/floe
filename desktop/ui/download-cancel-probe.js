/* New empty QA document only. A synthetic blob, no source or credentials. */
(function () {
    const b=document.getElementById('logout'),p=document.getElementById('browse-dialog'),
        c=document.getElementById('browse-close'),r=document.getElementById('browse-refresh');
    if(!b||b.disabled||!p||p.hidden||!c||c.disabled||!r||r.disabled||location.hash){return 'wait';}
    if(window.__floeDownloadCancelStarted){return 'download-started';}
    window.__floeDownloadCancelStarted=true;
    const url=URL.createObjectURL(new Blob(['Synthetic download cancellation QA'],{type:'text/plain'}));
    const a=document.createElement('a');a.href=url;a.download='synthetic-download-cancel.txt';
    document.body.appendChild(a);a.click();a.remove();
    // Keep the blob alive until WebKit reaches its destination delegate. The
    // new QA document owns it and releases it with the document at session end.
    return 'download-started';
}())
