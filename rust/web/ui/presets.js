/* Compiled palette display only. Assignment and inheritance belong to Rust. */
(function(root){
    'use strict';
    function validate(value) {
        if (!value || value.version !== 1) { throw Error('Unsupported preset palette.'); }
        ['colors','fills'].forEach(function(kind){
            const rows=value[kind], names=new Set();
            if (!Array.isArray(rows) || !rows.length || rows.length>256) { throw Error('Invalid preset palette.'); }
            rows.forEach(function(p){
                if (!p || typeof p.name!=='string' || !/^[a-z0-9_]{1,64}$/.test(p.name) || names.has(p.name)) { throw Error('Invalid preset name.'); }
                names.add(p.name);
                if(kind==='colors') { if(typeof p.color!=='string'||!/^#[0-9a-f]{6}$/i.test(p.color)) { throw Error('Invalid preset color.'); } }
                else {
                    if(!Array.isArray(p.rows)||p.rows.length!==16||!p.rows.every(function(n){return Number.isInteger(n)&&n>=0&&n<=65535;})||
                        !p.fill||!['solid','clear','speckle','pattern'].includes(p.fill.kind)) { throw Error('Invalid preset bitmap.'); }
                    if(p.fill.kind==='pattern'&&(!Array.isArray(p.fill.rows)||p.fill.rows.length!==16||p.rows.some(function(n,i){return n!==p.fill.rows[i];}))) { throw Error('Invalid preset fill.'); }
                }
            });
        });
        return value;
    }
    function preview(canvas, rows) {
        canvas.width=16; canvas.height=16; const ctx=canvas.getContext('2d');
        if(!ctx){throw Error('Canvas preview is unavailable.');}
        ctx.fillStyle='#ffffff';ctx.fillRect(0,0,16,16);ctx.fillStyle='#000000';
        for(let y=0;y<16;y++){for(let x=0;x<16;x++){if(rows[y]&(1<<(15-x))){ctx.fillRect(x,y,1,1);}}}
    }
    function bind(port) {
        const el=port.el, panel=el('palette-presets');
        let data=null, flight=null, failed=false, stopped=false, buttons=[];
        function available(){return !stopped&&port.available();}
        function cancel(){if(flight){const f=flight;flight=null;f.token.cancelled=true;if(f.token.abort){f.token.abort();}}}
        function controls(){
            buttons.forEach(function(b){b.disabled=!available()||!port.enabled();});
            el('presets-retry').hidden=!failed;el('presets-retry').disabled=!available()||!!flight;
            panel.setAttribute('aria-busy',String(!!flight));
        }
        function paint(p){
            ['colors','fills'].forEach(function(kind){el('presets-'+kind).textContent='';});buttons=[];
            ['colors','fills'].forEach(function(kind){p[kind].forEach(function(p){
                const b=port.document.createElement('button');b.type='button';b.className='preset-swatch';
                b.title=p.name+(kind==='colors'?' ('+p.color+')':'');
                b.setAttribute('aria-label','Apply '+(kind==='colors'?'color ':'fill ')+p.name+' to selected layers');
                if(kind==='colors'){b.style.backgroundColor=p.color;}
                else{const canvas=port.document.createElement('canvas');canvas.setAttribute('aria-hidden','true');preview(canvas,p.rows);b.appendChild(canvas);}
                b.onclick=function(){if(available()&&port.enabled()){port.apply(kind==='colors'?{color:p.color}:{fill:p.fill});}};
                el('presets-'+kind).appendChild(b);buttons.push(b);
            });});
        }
        async function load(){
            if(!available()||!panel.open||data||flight||failed){controls();return;}
            const f={token:{}};flight=f;el('presets-note').textContent='Loading palette…';controls();
            try{
                const value=await port.http('GET','/api/v1/palette/presets',undefined,false,f.token);
                if(flight!==f||f.token.cancelled||!available()){return;}
                const checked=validate(value);paint(checked);data=checked;el('presets-note').textContent='Click a swatch to apply it to the current selection.';
            }catch(e){
                if(flight===f&&!f.token.cancelled){failed=true;buttons=[];el('presets-colors').textContent='';el('presets-fills').textContent='';el('presets-note').textContent=e.message;}
            }finally{if(flight===f){flight=null;}controls();}
        }
        function changed(){if(!available()||!panel.open){cancel();}else{load();}controls();}
        panel.ontoggle=changed;
        el('presets-retry').onclick=function(){if(available()&&!flight){failed=false;load();}};
        return Object.freeze({changed:changed,stop:function(){stopped=true;cancel();controls();}});
    }
    const api={bind:bind,validate:validate,preview:preview};
    if(typeof module!=='undefined'&&module.exports){module.exports=api;}else{root.FloePresets=api;}
}(typeof window==='undefined'?this:window));
