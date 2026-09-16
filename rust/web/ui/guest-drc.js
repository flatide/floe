/* Read-only guest review. All navigation/selection state is private to the grant.
 * One HTTP operation at a time; uncertain mutations require an explicit reload. */
(function(root){
    'use strict';
    function bind(o){
        const P=o.protocol,G=o.geometry,el=o.el,doc=o.document;
        let bound='',turn=0,task=null,ready=false,metadata=null,panelRev='1',groups=null;
        let data=null,rules=[],rows=[],selected=null,points=null,complete=false;
        let ruleNext=null,errorNext=null,listRev=null,liveDirty=false;
        const ruleBack=[],errorBack=[];
        function defaults(){return {search:'',metric:null,rule_start:'0',check:null,error_start:'0',query:null,in_view:false,selected_only:false,waived:null,selected:null,
            markers:true,shown:true,jump_scale:null,zoom_lock:false,jump_active:false,focus_visible:false,cd:null};}
        function key(c){return c&&c.drc?c.view_id+':'+c.epoch+':'+c.drc.id+':'+c.drc.revision:'';}
        function context(){const c=o.context();return c&&key(c)===bound?c:null;}
        function note(s){el('gd-status').textContent=s;}
        function controls(){const c=context(),enabled=!!c&&ready&&!task;
            ['search','filter','waived','in-view','selected-only','markers','rule-prev','rule-next','error-prev','error-next','clear','go','fit'].forEach(function(n){el('gd-'+n).disabled=!enabled;});
            el('gd-rule-prev').disabled=!enabled||!ruleBack.length;el('gd-rule-next').disabled=!enabled||ruleNext===null;
            el('gd-error-prev').disabled=!enabled||!errorBack.length;el('gd-error-next').disabled=!enabled||errorNext===null;
            ['go','fit'].forEach(function(n){el('gd-'+n).disabled=!enabled||!selected||c.mode!=='explore'||c.pending;});
            el('gd-reload').disabled=!c;el('gd-panel').hidden=!bound;
        }
        function sync(){if(!data){return;}el('gd-search').value=data.search;el('gd-waived').value=data.waived===null?'all':data.waived?'yes':'no';
            ['in-view','selected-only','markers'].forEach(function(n){el('gd-'+n).checked=data[n.replace('-','_')];});}
        function cancel(){turn++;if(task){task.cancelled=true;if(task.abort){task.abort();}}task=null;}
        function reset(){cancel();bound='';ready=false;metadata=groups=data=null;rules=rows=[];selected=points=null;complete=false;listRev=null;liveDirty=false;
            ruleNext=errorNext=null;ruleBack.length=errorBack.length=0;el('gd-rules').textContent=el('gd-errors').textContent=el('gd-description').textContent=el('gd-selected').textContent='';controls();o.repaint();}
        function valid(t){return task===t&&!t.cancelled&&t.turn===turn&&key(o.context())===bound;}
        function check(t){if(!valid(t)){throw Error('Review context changed');}}
        async function call(t,method,path,body,view){check(t);const c=context();
            const request=body===undefined?undefined:Object.assign({view_id:c.view_id,revision:c.drc.revision},body);
            if(view&&request){request.state_rev=c.state_rev;}
            let v;try{v=await o.http(method,'/drc'+path,request,t);}catch(e){
                // A view-fenced read may be rejected by the server before it
                // has a reply body. Only this read (never a panel/selection
                // mutation) may follow the newly accepted camera.
                if(view&&valid(t)&&context().state_rev!==c.state_rev){e.viewChanged=true;}throw e;
            }check(t);
            if(!v||v.view_id!==c.view_id||v.revision!==c.drc.revision||!v.data){throw Error('Wrong guest DRC response');}
            if(view&&context().state_rev!==c.state_rev){const e=Error('View changed during review read');e.viewChanged=true;throw e;}
            return v.data;
        }
        async function read(t,body,view){return call(t,'POST','/read',{body:body},view);}
        function selection(v){const c=context();groups=o.selection.decode({view_id:c.view_id,revision:c.drc.revision,state:v},{view:c.view_id,revision:c.drc.revision},P);}
        function contains(r){return groups&&groups.rules.has(r.check)&&groups.rules.get(r.check).has(r.local);}
        function bbox(b){if(!Array.isArray(b)||b.length!==4){throw Error('Invalid DRC bounds');}b.forEach(P.decimal);const n=b.map(Number);if(n[0]>n[2]||n[1]>n[3]){throw Error('Invalid DRC bounds');}return n;}
        function record(r){if(!r){throw Error('Missing DRC record');}['check','local','global','points'].forEach(function(k){P.counter(r[k],true);});
            if(!['p','e'].includes(r.kind)||![0,1].includes(r.status)){throw Error('Invalid DRC record');}bbox(r.bbox_um);return r;}
        function recordRows(v){if(!Array.isArray(v)||v.length>64){throw Error('Invalid DRC page');}return v.map(record);}
        function render(){
            if(!selected){el('gd-selected').textContent='';}
            el('gd-rules').textContent='';rules.forEach(function(r){const b=doc.createElement('button');b.textContent=r.name+' · '+r.errors+' errors';b.disabled=!ready||!!task;
                b.setAttribute('aria-pressed',String(data.check===r.check));b.onclick=function(){run(async function(t){data.check=r.check;data.error_start='0';data.selected=null;errorBack.length=0;selected=points=null;complete=false;await details(t);await errors(t);await save(t);});};el('gd-rules').appendChild(b);});
            el('gd-errors').textContent='';rows.forEach(function(r){const b=doc.createElement('button');b.textContent=(contains(r)?'✓ ':'')+'#'+r.global+' · '+(r.status?'waived':'active');b.disabled=!ready||!!task;
                b.setAttribute('aria-pressed',String(!!selected&&selected.check===r.check&&selected.local===r.local));
                b.onclick=function(e){run(async function(t){const mode=e&&(e.ctrlKey||e.metaKey)?'toggle':e&&e.shiftKey?'add':'replace';
                    selection(await call(t,'POST','/selection',{base_selection_rev:groups.revision,body:{kind:'apply',check:r.check,errors:[r.local],mode:mode}}));
                    selected=r;data.selected={check:r.check,error:r.local};points=null;complete=false;await save(t);await geometry(t);if(data.selected_only){await errors(t);}});};el('gd-errors').appendChild(b);});
            el('gd-selection').textContent=groups?groups.total+' selected · private to this guest':'';controls();o.repaint();
        }
        async function save(t){const v=await call(t,'POST','/panel',{base_panel_rev:panelRev,body:data});P.counter(v.panel_rev);
            if(P.compare(v.panel_rev,panelRev)<0||!v.body||typeof v.body!=='object'){throw Error('Invalid panel save');}panelRev=v.panel_rev;
        }
        async function details(t){if(data.check===null){el('gd-description').textContent='';return;}const v=await read(t,{kind:'rule',check:data.check});
            if(v.check!==data.check||typeof v.description!=='string'){throw Error('Invalid rule details');}el('gd-description').textContent=v.description;
        }
        async function loadRules(t){const v=await read(t,{kind:'rules',start:data.rule_start,search:data.search,limit:32,waived:data.waived});
            if(!Array.isArray(v.rows)||v.rows.length>32){throw Error('Invalid rule page');}let last=null;
            v.rows.forEach(function(r){P.counter(r.check,true);P.counter(r.errors,true);if(typeof r.name!=='string'||r.name.length>4096||P.compare(r.check,data.rule_start)<0||last!==null&&P.compare(r.check,last)<=0){throw Error('Invalid rule row');}last=r.check;});
            if(v.next!==null){P.counter(v.next,true);if(P.compare(v.next,data.rule_start)<=0||last!==null&&P.compare(v.next,last)<=0){throw Error('Invalid rule cursor');}}
            rules=v.rows;ruleNext=v.next;
        }
        async function errors(t){if(data.check===null){rows=[];errorNext=null;return;}const rev=context().state_rev;
            const v=await read(t,{kind:'list',check:data.check,start:data.error_start,waived:data.waived,limit:64,in_view:data.in_view,selection_rev:data.selected_only?groups.revision:null},data.in_view);
            const values=recordRows(v.rows);let last=null;
            values.forEach(function(r){if(r.check!==data.check||P.compare(r.local,data.error_start)<0||last!==null&&P.compare(r.local,last)<=0||data.waived!==null&&(r.status===1)!==data.waived||data.selected_only&&!contains(r)){throw Error('Invalid filtered error');}last=r.local;});
            if(v.next!==null){P.counter(v.next,true);if(P.compare(v.next,data.error_start)<=0||last!==null&&P.compare(v.next,last)<=0){throw Error('Invalid error cursor');}}
            rows=values;errorNext=v.next;listRev=rev;
        }
        async function geometry(t){if(!selected){return;}const r=selected;let start='0',total=null,units=null;points=null;complete=false;
            for(let page=0;page<128;page++){
                const v=await read(t,{kind:'geometry',check:r.check,error:r.local,start:start,limit:2048});P.counter(v.total,true);P.counter(v.start,true);
                const n=Number(v.total),part=G.vertices(v,metadata.format,P),stamp=part.mode+':'+part.precision;
                if(v.check!==r.check||v.local!==r.local||v.global!==r.global||v.kind!==r.kind||v.start!==start||n<1||n>262144){throw Error('Invalid outline page');}
                if(total===null){total=n;units=stamp;points=new Float64Array(n*2);}else if(total!==n||units!==stamp){throw Error('Outline changed');}
                const offset=Number(start),end=offset+part.points.length/2;if(end>total){throw Error('Invalid outline bounds');}points.set(part.points,offset*2);
                el('gd-selected').textContent='Error #'+r.global+' · '+end+'/'+total+' vertices'+(v.next===null?'':' · bounding-box preview');o.repaint();
                if(v.next===null){if(end!==total){throw Error('Incomplete outline');}complete=true;return;}
                if(P.counter(v.next,true)!==String(end)){throw Error('Invalid outline cursor');}start=v.next;
            }throw Error('Outline page limit');
        }
        async function restore(t){metadata=await call(t,'GET','');if(!['ice','ascii'].includes(metadata.format)||metadata.read_only!==true){throw Error('Invalid DRC metadata');}
            ['checks','errors','truncated_records'].forEach(function(k){P.counter(metadata[k],true);});P.decimal(metadata.precision);
            el('gd-summary').textContent=metadata.checks+' rules · '+metadata.errors+' errors · '+metadata.format+' · '+metadata.truncated_records+' truncated records';
            const p=await call(t,'GET','/panel');P.counter(p.panel_rev);panelRev=p.panel_rev;data=Object.assign(defaults(),p.body||{});
            // This controller never restores owner-only note/metadata modes.
            data.metric=null;data.note_target=null;data.query=null;data.cd=null;data.jump_active=false;data.focus_visible=false;
            selection(await call(t,'GET','/selection'));sync();await loadRules(t);
            if(data.check===null&&rules.length){data.check=rules[0].check;data.error_start='0';}
            await details(t);await errors(t);
            if(data.selected){const v=await read(t,{kind:'records',check:data.selected.check,errors:[data.selected.error]});const values=recordRows(v.rows);
                if(values.length!==1||values[0].check!==data.selected.check||values[0].local!==data.selected.error){throw Error('Invalid restored error');}selected=values[0];await geometry(t);}
            ready=true;
        }
        async function run(fn,initial){if(task||!context()||!initial&&!ready){return;}const t={turn:turn,cancelled:false,abort:null};task=t;render();
            try{await fn(t);check(t);note('Read-only result · private selection/panel; no review files changed.');}
            catch(e){if(valid(t)){if(e.viewChanged){liveDirty=ready&&data.in_view;if(data.in_view){rows=[];}note(liveDirty?'View changed; updating the current-view list…':'View changed; reload review or repeat the intended action.');}
                else{ready=false;rows=[];selected=points=null;complete=false;note('Review unavailable or update unconfirmed: '+e.message+' · Reload review; nothing was replayed.');}}}
            finally{if(valid(t)){task=null;render();const again=liveDirty&&ready&&data.in_view&&context().state_rev!==listRev;liveDirty=false;
                if(again){run(async function(next){data.error_start='0';errorBack.length=0;await errors(next);});}}}
        }
        function reload(){const c=o.context();reset();if(!c||!c.drc){return;}bound=key(c);data=defaults();controls();note('Restoring private review state…');run(restore,true);}
        function changed(){const c=o.context();if(key(c)!==bound){reload();return;}controls();
            if(c&&ready&&data.in_view&&listRev!==c.state_rev){if(task){liveDirty=true;}else{run(async function(t){data.error_start='0';errorBack.length=0;await errors(t);});}}
        }
        function focus(fit){const c=context();if(!selected||!c||c.mode!=='explore'||c.pending){return;}run(async function(t){const r=selected,rev=c.state_rev;
            const v=await read(t,{kind:'focus',check:r.check,error:r.local,fit:fit,isolate:false},true);
            if(v.check!==r.check||v.local!==r.local||!v.navigation||v.navigation.kind!=='goto'||!Array.isArray(v.navigation.center_um)||v.navigation.center_um.length!==2){throw Error('Invalid error navigation');}
            v.navigation.center_um.forEach(P.decimal);if(v.navigation.width_um!==undefined){if(!(Number(P.decimal(v.navigation.width_um))>0)){throw Error('Invalid error width');}}
            if(context().state_rev!==rev||!o.navigate(v.navigation,c)){throw Error('View input changed; select Go again.');}
        });}
        el('gd-reload').onclick=reload;
        el('gd-filter').onclick=function(){run(async function(t){data.search=el('gd-search').value;data.rule_start='0';data.error_start='0';data.check=null;data.selected=null;selected=points=null;complete=false;
            ruleBack.length=errorBack.length=0;await loadRules(t);if(rules.length){data.check=rules[0].check;}await details(t);await errors(t);await save(t);});};
        el('gd-waived').onchange=function(){run(async function(t){const v=el('gd-waived').value;data.waived=v==='all'?null:v==='yes';data.rule_start=data.error_start='0';ruleBack.length=errorBack.length=0;await loadRules(t);await errors(t);await save(t);});};
        ['in-view','selected-only','markers'].forEach(function(n){el('gd-'+n).onchange=function(){run(async function(t){data[n.replace('-','_')]=el('gd-'+n).checked;
            if(n!=='markers'){data.error_start='0';errorBack.length=0;await errors(t);}await save(t);});};});
        [['rule',ruleBack],['error',errorBack]].forEach(function(pair){const name=pair[0],back=pair[1];
            ['prev','next'].forEach(function(direction){el('gd-'+name+'-'+direction).onclick=function(){run(async function(t){const field=name+'_start',next=name==='rule'?ruleNext:errorNext;
                if(direction==='prev'){if(!back.length){return;}data[field]=back.pop();}else{if(next===null){return;}if(back.length===64){back.shift();}back.push(data[field]);data[field]=next;}
                if(name==='rule'){await loadRules(t);}else{await errors(t);}await save(t);});};});});
        el('gd-clear').onclick=function(){run(async function(t){selection(await call(t,'POST','/selection',{base_selection_rev:groups.revision,body:{kind:'clear_all'}}));if(data.selected_only){data.error_start='0';await errors(t);}});};
        el('gd-go').onclick=function(){focus(false);};el('gd-fit').onclick=function(){focus(true);};
        function paint(ctx,p,size){if(!p||!context()||!data||!data.markers||!data.shown){return;}ctx.save();
            rows.forEach(function(r){const b=bbox(r.bbox_um),xy=G.point(p,b[0]*.5+b[2]*.5,b[1]*.5+b[3]*.5);if(!xy.every(Number.isFinite)){return;}
                if(xy[0]<-4||xy[0]>size[0]+4||xy[1]<-4||xy[1]>size[1]+4){return;}
                ctx.fillStyle=contains(r)?'#f4cd64':r.status?'#70da9a':'#ff6969';ctx.fillRect(Math.round(xy[0])-3,Math.round(xy[1])-3,7,7);});
            if(selected){ctx.strokeStyle=selected.status?'#70da9a':'#ff6969';ctx.lineWidth=2;
                if(complete&&points){ctx.beginPath();for(let i=0;i<points.length;i+=2){const xy=G.point(p,points[i],points[i+1]);if(i===0||selected.kind==='e'&&i%4===0){ctx.moveTo(xy[0],xy[1]);}else{ctx.lineTo(xy[0],xy[1]);}}if(selected.kind==='p'){ctx.closePath();}ctx.stroke();}
                else{const b=bbox(selected.bbox_um),a=G.point(p,b[0],b[3]),z=G.point(p,b[2],b[1]);ctx.strokeRect(a[0],a[1],z[0]-a[0],z[1]-a[1]);}}
            ctx.restore();
        }
        controls();return {changed:changed,reset:reset,paint:paint};
    }
    const api={bind:bind};if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeGuestDRC=api;}
}(typeof window==='object'?window:this));
