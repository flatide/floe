/* Immutable defaults and a separate session slot table. Assignment is Rust-owned. */
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
        let live=null, slotFlight=null, slotFailure='', slotIdentity='';
        function available(){return !stopped&&port.available();}
        function context(){const c=port.context();return available()&&port.slotEditor.valid(c)?c:null;}
        function identity(c){return c?c.id+':'+c.epoch+':'+c.slotKey:'';}
        function current(){return live&&live.identity===identity(context());}
        function cancelSlots(){if(slotFlight){const f=slotFlight;slotFlight=null;f.token.cancelled=true;if(f.token.abort){f.token.abort();}}}
        const editor=port.slotEditor.bind({el:el,document:port.document,window:port.window,preview:preview,
            context:context,submit:port.editSlot,changed:controls});
        function cancel(){if(flight){const f=flight;flight=null;f.token.cancelled=true;if(f.token.abort){f.token.abort();}}}
        function controls(){
            const c=context(),has=current(),dev=has&&live.value.editable&&c.fillEdit;
            buttons.forEach(function(b){b.disabled=!available()||!port.enabled()||(b.fillSlot&&!has);});
            el('presets-fills').hidden=!has;
            el('presets-retry').hidden=!failed&&!slotFailure;el('presets-retry').disabled=!available()||!!flight||!!slotFlight;
            el('fill-slot-tools').hidden=!dev;
            el('fill-slot-open').disabled=!dev||!c.idle||editor.active();
            panel.setAttribute('aria-busy',String(!!flight||!!slotFlight));
        }
        function openSlot(name){
            const c=context();if(!current()||!c.idle||!c.fillEdit||!live.value.editable){return;}
            const value=live.value.fills.find(function(p){return p.name===name;}),base=data.fills.find(function(p){return p.name===name;});
            if(value&&base){editor.open(name,value.rows,base.rows);}
        }
        function paint(p){
            ['colors','fills'].forEach(function(kind){el('presets-'+kind).textContent='';});buttons=[];
            ['colors','fills'].forEach(function(kind){p[kind].forEach(function(p){
                const b=port.document.createElement('button');b.type='button';b.className='preset-swatch';
                b.title=p.name+(kind==='colors'?' ('+p.color+')':'');
                b.setAttribute('aria-label','Apply '+(kind==='colors'?'color ':'fill ')+p.name+' to selected layers');
                if(kind==='colors'){b.style.backgroundColor=p.color;}
                else{const canvas=port.document.createElement('canvas');canvas.setAttribute('aria-hidden','true');preview(canvas,p.rows);b.appendChild(canvas);}
                b.fillSlot=kind==='fills'?p.name:null;
                b.onclick=function(){if(available()&&port.enabled()&&(kind==='colors'||current())){port.apply(kind==='colors'?{color:p.color}:{fill_slot:p.name});}};
                if(kind==='fills'){b.oncontextmenu=function(e){const c=context();if(current()&&c.fillEdit&&p.name!=='solid'&&p.name!=='clear'){e.preventDefault();openSlot(p.name);}};}
                el('presets-'+kind).appendChild(b);buttons.push(b);
            });});
        }
        async function loadSlots(){
            const c=context(),key=identity(c);if(!c||!panel.open||!data||current()||slotFlight||slotFailure===key){controls();return;}
            const f={identity:key,context:c,token:{}};slotFlight=f;el('presets-note').textContent='Loading session fill slots…';controls();
            try{
                const value=await port.http('GET','/api/v1/views/'+c.id+'/fill-slots/'+c.slotKey,undefined,false,f.token);
                if(slotFlight!==f||f.token.cancelled||identity(context())!==key){return;}
                if(!value||value.version!==1||value.view_id!==c.id||value.fill_slots_key!==c.slotKey||typeof value.editable!=='boolean'||
                    !Array.isArray(value.fills)||value.fills.length!==data.fills.length){throw Error('Invalid session fill slots.');}
                value.fills.forEach(function(p,i){
                    if(!p||p.name!==data.fills[i].name){throw Error('Invalid session fill slot name.');}
                    port.slotEditor.rows(p.rows);
                    if((p.name==='solid'||p.name==='clear')&&p.rows.some(function(n,y){return n!==data.fills[i].rows[y];})){throw Error('Invalid fixed fill slot.');}
                });
                live={identity:key,value:value};
                buttons.filter(function(b){return !!b.fillSlot;}).forEach(function(b){const p=value.fills.find(function(p){return p.name===b.fillSlot;});preview(b.children[0],p.rows);});
                el('presets-note').textContent='Click a swatch to apply it to the selection. Fill swatches reference the current session slot.';
            }catch(e){if(slotFlight===f&&!f.token.cancelled){live=null;slotFailure=key;el('presets-note').textContent=e.message;}}
            finally{if(slotFlight===f){slotFlight=null;}controls();}
        }
        async function load(){
            if(!available()||!panel.open||data||flight||failed){controls();return;}
            const f={token:{}};flight=f;el('presets-note').textContent='Loading palette…';controls();
            try{
                const value=await port.http('GET','/api/v1/palette/presets',undefined,false,f.token);
                if(flight!==f||f.token.cancelled||!available()){return;}
                const checked=validate(value);paint(checked);data=checked;
                const select=el('fill-slot-choice');select.textContent='';data.fills.filter(function(p){return p.name!=='solid'&&p.name!=='clear';}).forEach(function(p){const option=port.document.createElement('option');option.value=p.name;option.textContent=p.name;select.appendChild(option);});
                loadSlots();
            }catch(e){
                if(flight===f&&!f.token.cancelled){failed=true;buttons=[];el('presets-colors').textContent='';el('presets-fills').textContent='';el('presets-note').textContent=e.message;}
            }finally{if(flight===f){flight=null;}controls();}
        }
        function changed(){
            const key=identity(context());if(key!==slotIdentity){cancelSlots();live=null;slotFailure='';slotIdentity=key;}
            editor.changed();
            if(!available()||!panel.open){cancel();cancelSlots();editor.cancel('Palette closed; any sent bitmap may already have committed.');}
            else{load();loadSlots();}controls();
        }
        panel.ontoggle=changed;
        el('presets-retry').onclick=function(){if(available()&&!flight&&!slotFlight){failed=false;slotFailure='';load();loadSlots();}};
        el('fill-slot-open').onclick=function(){openSlot(el('fill-slot-choice').value);};
        return Object.freeze({changed:changed,stop:function(){stopped=true;changed();}});
    }
    const api={bind:bind,validate:validate,preview:preview};
    if(typeof module!=='undefined'&&module.exports){module.exports=api;}else{root.FloePresets=api;}
}(typeof window==='undefined'?this:window));
