/* ES2017. A local bitmap draft, never a file write or an automatic save. */
(function(root){
    'use strict';
    function rows(value){
        if(!Array.isArray(value)||value.length!==16||!value.every(function(n){return Number.isInteger(n)&&n>=0&&n<=65535;})){throw Error('Invalid 16×16 bitmap.');}
        return value.slice();
    }
    function draft(value,defaults){
        let bits=rows(value),base=rows(defaults),ink=null;
        function inside(x,y){return Number.isInteger(x)&&Number.isInteger(y)&&x>=0&&x<16&&y>=0&&y<16;}
        function paint(x,y){if(ink===null||!inside(x,y)){return;}const mask=1<<(15-x);bits[y]=ink?(bits[y]|mask):(bits[y]&~mask);}
        return {
            rows:function(){return bits.slice();},
            press:function(x,y){if(!inside(x,y)){return;}ink=!(bits[y]&(1<<(15-x)));paint(x,y);},
            move:paint,release:function(){ink=null;},
            operation:function(kind){
                if(!['clear','solid','invert','reset'].includes(kind)){throw Error('Invalid bitmap operation.');}
                ink=null;bits=kind==='reset'?base.slice():bits.map(function(n){return kind==='clear'?0:kind==='solid'?65535:n^65535;});
            }
        };
    }
    function valid(c){return c&&c.ready&&/^[a-f0-9]{64}$/.test(c.id)&&/^[a-f0-9]{64}$/.test(c.epoch)&&/^[1-9][0-9]*$/.test(c.rev)&&/^[a-f0-9]{40}$/.test(c.slotKey);}
    function bind(o){
        const el=o.el,form=el('fill-slot-editor'),grid=el('fill-slot-grid');
        let job=null,cells=[],pointer=null,mouse=false,focus=0;
        function context(){const c=o.context();return valid(c)?c:null;}
        function same(j,revision){const c=context();return c&&c.fillEdit&&c.id===j.context.id&&c.epoch===j.context.epoch&&(!revision||(c.rev===j.context.rev&&c.slotKey===j.context.slotKey));}
        function editable(){const c=context();return job&&job.phase==='draft'&&same(job,true)&&c.idle;}
        function status(text){el('fill-slot-status').textContent=text||'';}
        function release(){pointer=null;mouse=false;if(job){job.draft.release();}}
        function controls(){
            const edit=!!editable();form.hidden=!job;
            ['clear','solid','invert','reset','apply'].forEach(function(k){el('fill-slot-'+k).disabled=!edit;});
            cells.forEach(function(b){b.disabled=!edit;});
            form.setAttribute('aria-busy',String(!!job&&job.phase==='apply'));
        }
        function paint(){if(!job){return;}const bits=job.draft.rows();cells.forEach(function(b,i){const on=!!(bits[i>>4]&(1<<(15-(i%16))));b.setAttribute('aria-pressed',String(on));b.dataset.on=String(on);b.tabIndex=i===focus?0:-1;});o.preview(el('fill-slot-preview'),bits);}
        function finish(j,text){if(job!==j){return;}release();job=null;cells=[];grid.textContent='';controls();status(text);o.changed();if(!el('fill-slot-tools').hidden&&!el('fill-slot-open').disabled){el('fill-slot-open').focus();}}
        function cancel(text){const j=job;if(!j){return;}const sent=j.phase==='apply';if(j.cancel){j.cancel();}finish(j,text||(sent?'Bitmap submission may already have committed. The current view is authoritative.':'Bitmap draft discarded.'));}
        function changed(){
            if(job&&!same(job,job.phase==='draft')){cancel('View or connection changed; the bitmap was not replayed.');}
            if(!editable()){release();}controls();
        }
        function point(e){const r=grid.getBoundingClientRect();if(!r.width||!r.height){return [-1,-1];}return [Math.floor((e.clientX-r.left)*16/r.width),Math.floor((e.clientY-r.top)*16/r.height)];}
        function down(e){
            if(!editable()||e.button!==0||pointer!==null||mouse){return false;}
            const p=point(e);if(p[0]<0||p[0]>=16||p[1]<0||p[1]>=16){return false;}
            e.preventDefault();focus=p[1]*16+p[0];job.draft.press(p[0],p[1]);paint();cells[focus].focus();return true;
        }
        function move(e){if(!editable()){release();return;}if(e.buttons!==undefined&&(e.buttons&1)===0){release();return;}const p=point(e);job.draft.move(p[0],p[1]);paint();}
        if(typeof o.window.PointerEvent==='function'){
            grid.onpointerdown=function(e){
                if(down(e)){
                    pointer=e.pointerId;
                    if(grid.setPointerCapture){try{grid.setPointerCapture(e.pointerId);}catch(error){release();status('Drag capture is unavailable. Use individual clicks or the keyboard.');}}
                }
            };
            grid.onpointermove=function(e){if(pointer===e.pointerId){move(e);}};
            grid.onpointerup=grid.onpointercancel=grid.onlostpointercapture=function(e){if(pointer===e.pointerId){release();}};
        }else{
            grid.onmousedown=function(e){if(down(e)){mouse=true;}};
            o.document.addEventListener('mousemove',function(e){if(mouse){move(e);}});
            o.document.addEventListener('mouseup',release);
        }
        o.window.addEventListener('blur',release);
        grid.onkeydown=function(e){
            if(e.isComposing){return;}if(e.key==='Escape'){e.preventDefault();e.stopPropagation();cancel();return;}
            if(!editable()||!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End'].includes(e.key)){return;}
            e.preventDefault();e.stopPropagation();release();
            if(e.key==='Home'){focus=e.ctrlKey?0:focus-(focus%16);}
            else if(e.key==='End'){focus=e.ctrlKey?255:focus-(focus%16)+15;}
            else{focus=Math.max(0,Math.min(255,focus+({ArrowLeft:-1,ArrowRight:1,ArrowUp:-16,ArrowDown:16})[e.key]));}
            paint();cells[focus].focus();
        };
        ['clear','solid','invert','reset'].forEach(function(k){el('fill-slot-'+k).onclick=function(){if(editable()){release();job.draft.operation(k);paint();}};});
        el('fill-slot-cancel').onclick=function(){cancel();};
        form.onsubmit=function(e){
            e.preventDefault();if(!editable()){changed();return;}release();const j=job;j.phase='apply';controls();status('Applying bitmap to every reference…');o.changed();
            try{j.cancel=o.submit(j.context,{name:j.name,rows:j.draft.rows()},function(error){finish(j,error||'Bitmap applied to the session. Use Native JSON to save slots and references.');});}
            catch(error){finish(j,error.message||'Bitmap was not applied.');}
        };
        controls();
        return {
            open:function(name,value,defaults){
                const c=context();if(job||!c||!c.idle||!c.fillEdit||!name||name==='solid'||name==='clear'){return false;}
                job={name:name,context:Object.assign({},c),draft:draft(value,defaults),phase:'draft'};focus=0;grid.textContent='';cells=[];
                el('fill-slot-title').textContent='Edit bitmap · '+name;
                for(let i=0;i<256;i++){
                    const b=o.document.createElement('button');b.type='button';b.className='fill-bit';b.setAttribute('aria-label','Row '+((i>>4)+1)+', column '+((i%16)+1));
                    b.onclick=function(e){if(editable()&&e.detail===0){focus=i;job.draft.press(i%16,i>>4);job.draft.release();paint();}};
                    b.onfocus=function(){focus=i;cells.forEach(function(c,n){c.tabIndex=n===i?0:-1;});};
                    grid.appendChild(b);cells.push(b);
                }
                paint();controls();status('Local draft only. Apply changes every reference to this slot; no file is written.');cells[0].focus();o.changed();return true;
            },
            active:function(){return !!job;},changed:changed,cancel:cancel
        };
    }
    const api={rows:rows,draft:draft,valid:valid,bind:bind};
    if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeFillEditor=api;}
}(typeof window==='object'?window:this));
