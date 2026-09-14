/* ES2017. One catalogue page and one original text/hex chunk, never HTML. */
(function(root){
    'use strict';
    const PAGE=65536,TOTAL=134217728,COUNT=4096;
    function bad(){throw new Error('Invalid portable notice response');}
    function uint(n,max){return Number.isSafeInteger(n)&&n>=0&&n<=max;}
    function hash(v){return typeof v==='string'&&/^[a-f0-9]{40}$/.test(v);}
    function metadata(v){
        if(!v||!['available','not_packaged','unavailable'].includes(v.status)||v.page_bytes!==PAGE||v.list_size!==64||!uint(v.files,COUNT)||!uint(v.total_bytes,TOTAL)){bad();}
        if(v.status==='available'){if(!hash(v.index_id)||v.files<1){bad();}}
        else if(v.index_id!==null||v.files!==0||v.total_bytes!==0){bad();}return v;
    }
    function file(v,m){
        if(!v||!uint(v.id,m.files-1)||typeof v.name!=='string'||!v.name.startsWith('NOTICES/')||v.name.length>1024||/[\x00-\x1f\x7f\\]/.test(v.name)||
            v.name.split('/').some(function(p){return !p||p==='.'||p==='..';})||!uint(v.bytes,m.total_bytes)||
            !uint(v.pages,Math.floor(v.bytes/(PAGE-3))+1)||v.pages<1||!['utf8','hex'].includes(v.encoding)){bad();}return v;
    }
    function listing(v,m,start){
        if(!v||v.index_id!==m.index_id||v.start!==start||v.total!==m.files||!Array.isArray(v.files)||v.files.length!==Math.min(64,m.files-start)||
            v.next!==(start+64<m.files?start+64:null)){bad();}
        v.files.forEach(function(f,i){file(f,m);if(f.id!==start+i){bad();}});return v;
    }
    function chunk(v,m,f,page){
        if(!v||v.index_id!==m.index_id||v.page!==page||!uint(v.page,f.pages-1)||!v.file||
            ['id','name','bytes','pages','encoding'].some(function(k){return v.file[k]!==f[k];})||
            !uint(v.offset,f.bytes)||!uint(v.bytes,PAGE)||v.offset+v.bytes>f.bytes||typeof v.text!=='string'||v.text.length>3*PAGE||
            (page===0&&v.offset!==0)||(page===f.pages-1&&v.offset+v.bytes!==f.bytes)||
            (page<f.pages-1&&v.bytes<PAGE-3)||(v.bytes===0&&f.bytes!==0)){bad();}
        if(f.encoding==='utf8'){if(new TextEncoder().encode(v.text).length!==v.bytes){bad();}}
        else if(v.text.length!==v.bytes*3||!/^(?:[0-9a-f]{2}[ \n])*$/.test(v.text)){bad();}return v;
    }
    function bind(o){
        const el=o.el,doc=o.document;let model=null,list=null,selected=null,page=0,listTask=null,readTask=null;
        function abort(t){if(t){t.cancelled=true;if(t.abort){t.abort();}}}
        function clearRead(){abort(readTask);readTask=null;selected=null;page=0;el('notice-title').textContent='';el('notice-text').textContent='';el('notice-page-status').textContent='';}
        function controls(){
            el('notice-list-prev').disabled=!list||!!listTask||list.start===0;
            el('notice-list-next').disabled=!list||!!listTask||list.next===null;
            el('notice-list-retry').disabled=!model||!!listTask;
            el('notice-prev').disabled=!selected||!!readTask||page===0;
            el('notice-next').disabled=!selected||!!readTask||page+1>=selected.pages;
            el('notice-go').disabled=!selected||!!readTask;el('notice-jump').disabled=!selected||!!readTask;
            el('notice-retry').disabled=!selected||!!readTask;
        }
        function close(){abort(listTask);listTask=null;clearRead();model=null;list=null;el('notice-files').textContent='';el('notice-catalog').hidden=true;el('notice-availability').textContent='';el('notice-list-status').textContent='';controls();}
        async function load(start){
            if(!model){return;}abort(listTask);clearRead();list=null;el('notice-files').textContent='';
            el('notice-list-status').textContent='Reading notice list…';const m=model,t={cancelled:false,abort:null};listTask=t;controls();
            try{const v=listing(await o.http('GET','/api/v1/about/notices/'+start,undefined,false,t),m,start);
                if(t.cancelled||model!==m||listTask!==t){return;}list=v;
                v.files.forEach(function(f){const b=doc.createElement('button');b.textContent=f.name+' · '+f.bytes+' bytes';b.onclick=function(){read(f,0);};el('notice-files').appendChild(b);});
                el('notice-list-status').textContent=(start+1)+'–'+(start+v.files.length)+' of '+v.total+' files';
            }catch(e){if(!t.cancelled&&listTask===t){el('notice-list-status').textContent='Notice list unavailable. '+e.message;}}
            finally{if(listTask===t){listTask=null;controls();}}
        }
        async function read(f,p){
            if(!model||!uint(p,f.pages-1)){return;}abort(readTask);selected=f;page=p;el('notice-jump').value=String(p+1);
            el('notice-title').textContent=f.name;el('notice-text').textContent='';el('notice-page-status').textContent='Reading verified chunk…';
            const m=model,t={cancelled:false,abort:null};readTask=t;controls();
            try{const v=chunk(await o.http('GET','/api/v1/about/notices/'+f.id+'/'+p,undefined,false,t),m,f,p);
                if(t.cancelled||model!==m||readTask!==t){return;}el('notice-text').textContent=v.text;
                el('notice-page-status').textContent='Page '+(p+1)+' / '+f.pages+' · bytes '+v.offset+'–'+(v.offset+v.bytes)+' of '+f.bytes+
                    (f.encoding==='hex'?' · binary shown as hex':' · UTF-8 source; HTML is not executed');
            }catch(e){if(!t.cancelled&&readTask===t){el('notice-page-status').textContent='Chunk unavailable or changed. No partial/stale text shown. '+e.message;}}
            finally{if(readTask===t){readTask=null;controls();}}
        }
        el('notice-list-prev').onclick=function(){if(list){load(Math.max(0,list.start-64));}};
        el('notice-list-next').onclick=function(){if(list&&list.next!==null){load(list.next);}};
        el('notice-list-retry').onclick=function(){load(list?list.start:0);};
        el('notice-prev').onclick=function(){if(selected){read(selected,page-1);}};
        el('notice-next').onclick=function(){if(selected){read(selected,page+1);}};
        el('notice-retry').onclick=function(){if(selected){read(selected,page);}};
        el('notice-go').onclick=function(){const s=el('notice-jump').value;if(selected&&/^[1-9][0-9]{0,3}$/.test(s)&&Number(s)<=selected.pages){read(selected,Number(s)-1);}else{el('notice-page-status').textContent='Enter a page number within this file.';}};
        function open(v){
            close();v=metadata(v);
            if(v.status!=='available'){
                el('notice-availability').textContent=v.status==='not_packaged'?'This executable has no compiled portable notice index. The embedded font notice remains available.':'The compiled portable notice index could not be verified. Check the installation with verify.sh and selfcheck; restart after repair. The viewer remains usable.';return;
            }
            model=v;el('notice-catalog').hidden=false;
            el('notice-availability').textContent=v.files+' original files · '+v.total_bytes+' bytes. Index pinned to this executable; each requested chunk is checked. This is not publisher authentication.';
            load(0);
        }
        close();return {open:open,close:close};
    }
    const api={metadata:metadata,listing:listing,chunk:chunk,bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeNotices=api;}
}(typeof window!=='undefined'?window:this));
