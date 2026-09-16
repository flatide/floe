/* Scoped read-only palette: only visibility of an Explorer's own view changes.
 * Reads may follow a newer camera; display edits are never replayed. */
(function(root){
    'use strict';
    function bind(o){
        const el=o.el,doc=o.document,P=o.protocol;
        let identity='',loaded='',task=null,generation=0,start=0,next=null,rows=[],total=0,allTotal=0,ready=false,waiting=false;
        let fold={closed:false,exceptions:[]};
        let picked=[],names=[];
        function highlight(pairs){picked=pairs.map(function(p){return p.join('/');});names.forEach(function(n){n.node.className=picked.includes(n.key)?'guest-layer-picked':'';});}
        function key(c){return c?c.view_id+':'+c.epoch:'';}
        function version(c){return c?key(c)+':'+c.render_key:'';}
        function note(s){el('gl-status').textContent=s;}
        function pair(p){if(!Array.isArray(p)||p.length!==2||p.some(function(n){return !Number.isInteger(n)||n<0||n>4294967295;})){throw Error('Invalid layer key');}return p;}
        function same(a,b){return a[0]===b[0]&&a[1]===b[1];}
        function context(){const c=o.context();return c&&key(c)===identity?c:null;}
        function controls(){const c=context(),readable=!!c&&!task,editable=readable&&ready&&!waiting&&!c.pending&&c.mode==='explore';
            el('gl-panel').hidden=!identity;el('gl-reload').disabled=!c;
            ['all','none'].forEach(function(k){el('gl-'+k).disabled=!editable;});
            ['collapse','expand'].forEach(function(k){el('gl-'+k).disabled=!readable||!ready;});
            el('gl-prev').disabled=!readable||!ready||start===0;el('gl-next').disabled=!readable||!ready||next===null;
            return {readable:readable,editable:editable};
        }
        function change(body){const c=context();if(!controls().editable){return;}waiting=true;render();
            if(!o.edit(body,c)){waiting=false;note('View input changed; choose the layer again.');render();}}
        function render(){const available=controls();el('gl-rows').textContent='';names=[];
            rows.forEach(function(r){const line=doc.createElement('div');line.className='guest-layer-row'+(r.parent?' guest-layer-child':'');
                if(r.children){const toggle=doc.createElement('button');toggle.textContent=r.closed?'▸':'▾';toggle.disabled=!available.readable;
                    toggle.setAttribute('aria-label',(r.closed?'Expand ':'Collapse ')+r.name);toggle.onclick=function(){if(!controls().readable){return;}
                        const n=fold.exceptions.findIndex(function(p){return same(p,r.pair);});
                        if(n>=0){fold.exceptions.splice(n,1);}else{if(fold.exceptions.length>=4096){note('Fold exception limit; use Expand/Collapse all.');return;}fold.exceptions.push(r.pair.slice());}
                        start=0;load();};line.appendChild(toggle);}
                const label=doc.createElement('label'),box=doc.createElement('input'),text=doc.createElement('span');box.type='checkbox';
                const group=r.synthetic||r.closed;box.checked=group?r.all_visible:r.visible;box.indeterminate=group&&r.mixed;box.disabled=!available.editable;
                box.onchange=function(){change({layer_visibility:{pair:r.pair.slice(),group:group,visible:box.checked}});};
                text.textContent=r.pair[0]+'.'+r.pair[1]+' '+r.name;text.style.color=r.color;
                const key=r.pair.join('/');names.push({node:text,key:key});text.className=picked.includes(key)?'guest-layer-picked':'';
                label.appendChild(box);label.appendChild(text);line.appendChild(label);el('gl-rows').appendChild(line);
            });
            el('gl-count').textContent=ready?(rows.length?(start+1)+'–'+(start+rows.length):'0')+' / '+total+' shown rows · '+allTotal+' approved rows':'';
        }
        function cancel(){generation++;if(task){task.cancelled=true;if(task.abort){task.abort();}}task=null;}
        function reset(){cancel();identity=loaded='';rows=[];picked=[];ready=waiting=false;start=0;next=null;total=allTotal=0;fold={closed:false,exceptions:[]};render();}
        function valid(t){return task===t&&!t.cancelled&&t.generation===generation&&key(o.context())===identity;}
        function decode(v,c){if(!v||v.view_id!==c.view_id||!v.data){throw Error('Wrong palette identity');}const d=v.data;
            if(P.counter(d.state_rev)!==c.state_rev||P.counter(d.render_key)!==c.render_key){throw Error('Wrong palette revision');}
            ['start','total','all_total'].forEach(function(k){if(!Number.isSafeInteger(d[k])||d[k]<0){throw Error('Invalid palette size');}});
            if(d.start!==start||d.total>d.all_total||!Array.isArray(d.rows)||d.rows.length>64||d.start+d.rows.length>d.total){throw Error('Invalid palette page');}
            if(d.next!==null&&d.next!==d.start+d.rows.length||d.next!==null&&(!d.rows.length||d.next>=d.total)||d.next===null&&d.start+d.rows.length!==d.total){throw Error('Invalid palette cursor');}
            let last=null;d.rows.forEach(function(r){pair(r.pair);if(r.parent!==null){pair(r.parent);}if(last&&(r.pair[0]<last[0]||r.pair[0]===last[0]&&r.pair[1]<=last[1])){throw Error('Invalid palette order');}last=r.pair;
                if(typeof r.name!=='string'||r.name.length>256||!/^#[0-9a-f]{6}$/i.test(r.color)||!Number.isSafeInteger(r.children)||r.children<0){throw Error('Invalid palette row');}
                ['visible','head','synthetic','closed','all_visible','mixed'].forEach(function(k){if(typeof r[k]!=='boolean'){throw Error('Invalid palette flag');}});
                if(r.closed&&!r.children||r.synthetic&&!r.head){throw Error('Invalid palette group');}
            });return d;
        }
        async function load(){const c=context();if(!c||task){return;}const t={generation:generation,cancelled:false,abort:null};task=t;render();let retry=false;
            try{const v=await o.http('POST','/layers',{view_id:c.view_id,state_rev:c.state_rev,body:{start:start,fold:{closed:fold.closed,exceptions:fold.exceptions.map(function(p){return p.slice();})}}},t);
                if(!valid(t)){return;}if(version(context())!==version(c)){retry=true;return;}const d=decode(v,c);
                rows=d.rows;total=d.total;allTotal=d.all_total;next=d.next;loaded=version(c);ready=true;
                note(c.mode==='follow'?'Following owner visibility · no layer edits.':'Only approved layers can be shown. Collapsed group checks affect approved members only.');
            }catch(e){if(valid(t)){retry=!!context()&&context().state_rev!==c.state_rev&&(!e.status||e.status===409);
                    if(!retry){ready=false;rows=[];note('Palette unavailable: '+e.message+' · Reload layers. No change was replayed.');}}}
            finally{if(valid(t)){task=null;render();if(retry){load();}}}
        }
        function changed(){const c=o.context();if(key(c)!==identity){reset();if(!c){return;}identity=key(c);load();return;}
            if(!c){return;}let refresh=version(c)!==loaded;if(waiting&&!c.pending){waiting=false;refresh=true;}
            if(refresh&&!task&&!c.pending){load();}else{render();}}
        el('gl-reload').onclick=function(){cancel();ready=false;rows=[];load();};
        el('gl-prev').onclick=function(){if(!controls().readable||!ready||!start){return;}start=Math.max(0,start-64);load();};
        el('gl-next').onclick=function(){if(!controls().readable||!ready||next===null){return;}start=next;load();};
        ['collapse','expand'].forEach(function(k){el('gl-'+k).onclick=function(){if(!controls().readable||!ready){return;}fold={closed:k==='collapse',exceptions:[]};start=0;load();};});
        el('gl-all').onclick=function(){change({layers:{mode:'all'}});};el('gl-none').onclick=function(){change({layers:{mode:'none'}});};
        render();return {changed:changed,reset:reset,highlight:highlight};
    }
    const api={bind:bind};if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeGuestLayers=api;}
}(typeof window==='object'?window:this));
