/* Structural navigation chrome: palette pixels and screen coordinates only.
 * Depth geometry, viewport projection and click world math belong to Rust. */
(function(root){
    'use strict';
    const SIZE=180, COLORS=['#000000','#141414','#666666','#46565f','#8ecdf5'];
    function bind(port){
        const el=port.el, canvas=el('minimap'), ctx=canvas.getContext('2d',{alpha:false});
        let identity='', cache=new Map(), flight=null, failed='', stamp='', stopped=false, suspended=false;
        canvas.width=canvas.height=SIZE; ctx.imageSmoothingEnabled=false;
        function current(){const s=port.state();return !stopped&&!suspended&&s&&s.minimap&&s.connection_epoch?s:null;}
        function key(s){return s.view_id+':'+s.dataset_revision+':'+s.connection_epoch;}
        function projection(m){
            const rect=r=>Array.isArray(r)&&r.length===5&&r.every(Number.isInteger)&&r[0]>=0&&r[1]>=0&&r[2]>0&&r[3]>0&&r[0]+r[2]<=SIZE&&r[1]+r[3]<=SIZE&&r[4]>=0&&r[4]<COLORS.length;
            if(m.size!==SIZE||typeof m.base!=='string'||!(/^(full|[0-9]|[12][0-9]|3[01])$/).test(m.base)||!Array.isArray(m.marks)||m.marks.length>6||!m.marks.every(rect)||
               (m.die!==null&&(!Array.isArray(m.die)||!rect(m.die.concat([1]))))){throw new Error('Invalid overview projection');}
        }
        function paint(s){
            const m=s.minimap, base=cache.get(m.base), next=JSON.stringify([identity,m,!!base,failed===m.base]);
            if(stamp===next){return;} stamp=next;ctx.fillStyle=COLORS[0];ctx.fillRect(0,0,SIZE,SIZE);
            if(base){ctx.drawImage(base,0,0);}
            else if(m.die){ctx.fillStyle=COLORS[1];ctx.fillRect.apply(ctx,m.die);}
            m.marks.forEach(function(r){ctx.fillStyle=COLORS[r[4]];ctx.fillRect(r[0],r[1],r[2],r[3]);});
            el('minimap-note').textContent=failed===m.base?'Overview unavailable; retry to reload.':!base?'Loading structural overview…':m.base==='full'?'Die outline · current view':'Baked depth '+m.base+' · current view';
            el('minimap-retry').hidden=failed!==m.base;
        }
        async function fetchBase(s){
            const id=identity, base=s.minimap.base, request={};flight=request;
            try{
                const r=await port.http('GET','/api/v1/views/'+s.view_id+'/minimap/'+base);
                if(stopped||identity!==id){return;}
                if(r.view_id!==s.view_id||r.dataset_revision!==s.dataset_revision||r.base!==base||r.size!==SIZE||typeof r.pixels!=='string'||r.pixels.length!==SIZE*SIZE||/[^0-3]/.test(r.pixels)){throw new Error('Invalid overview base');}
                const image=port.document.createElement('canvas');image.width=image.height=SIZE;
                const b=image.getContext('2d',{alpha:false});b.imageSmoothingEnabled=false;
                // Runs avoid a separate RGBA allocation/decoder. No native
                // thumbnail request, image smoothing, or geometry traversal.
                for(let y=0;y<SIZE;y++){for(let x=0;x<SIZE;){const c=r.pixels[y*SIZE+x];let end=x+1;while(end<SIZE&&r.pixels[y*SIZE+end]===c){end++;}b.fillStyle=COLORS[Number(c)];b.fillRect(x,y,end-x,1);x=end;}}
                cache.set(base,image);while(cache.size>3){cache.delete(cache.keys().next().value);} failed='';
            }catch(e){if(!stopped&&identity===id){failed=base;}}
            finally{if(flight===request){flight=null;} if(!stopped){changed();}}
        }
        function changed(){
            const s=current();el('minimap-panel').hidden=!s||stopped;canvas.setAttribute('aria-disabled',String(!s||!port.ready()));
            if(!s||stopped){identity='';cache.clear();stamp='';return;}
            try{projection(s.minimap);canvas.hidden=false;}catch(e){el('minimap-note').textContent=e.message;canvas.hidden=true;el('minimap-panel').hidden=false;canvas.setAttribute('aria-disabled','true');return;}
            const id=key(s);if(id!==identity){identity=id;cache.clear();failed='';stamp='';}
            paint(s);if(!flight&&!cache.has(s.minimap.base)&&failed!==s.minimap.base){fetchBase(s);}
        }
        function jump(point){
            const s=current();if(!s||stopped||!port.ready()){return;}
            try{projection(s.minimap);port.navigate({kind:'minimap',point:point});port.focus();}catch(e){el('minimap-note').textContent=e.message;}
        }
        canvas.addEventListener('mousedown',function(e){
            if(e.button!==0||(e.buttons!==undefined&&e.buttons!==1)||e.detail>1||e.altKey||e.ctrlKey||e.metaKey){return;}
            const r=canvas.getBoundingClientRect();if(!(r.width>0&&r.height>0)){return;}
            e.preventDefault();jump([(e.clientX-r.left)*SIZE/r.width,(e.clientY-r.top)*SIZE/r.height]);
        });
        canvas.addEventListener('keydown',function(e){if(!e.isComposing&&!e.ctrlKey&&!e.metaKey&&!e.altKey&&(e.key==='Enter'||e.key===' ')){e.preventDefault();jump([SIZE/2,SIZE/2]);}});
        el('minimap-retry').onclick=function(){failed='';changed();};
        function suspend(){suspended=true;identity='';cache.clear();stamp='';el('minimap-panel').hidden=true;}
        function resume(){if(!stopped){suspended=false;changed();}}
        function stop(){stopped=true;suspend();}
        changed();return Object.freeze({changed:changed,stop:stop,suspend:suspend,resume:resume});
    }
    if(typeof module!=='undefined'&&module.exports){module.exports={bind:bind};}else{root.FloeMinimap={bind:bind};}
}(typeof window==='undefined'?this:window));
