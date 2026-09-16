/* Read-only guest review. All navigation/selection state is private to the grant.
 * One HTTP operation at a time; uncertain mutations require an explicit reload. */
(function(root){
    'use strict';
    function bind(o){
        const P=o.protocol,G=o.geometry,el=o.el,doc=o.document;
        let bound='',turn=0,task=null,ready=false,metadata=null,panelRev='1',groups=null;
        let data=null,rules=[],rows=[],selected=null,points=null,complete=false;
        let ruleNext=null,errorNext=null,listRev=null,liveDirty=false;
        let continuation=null,continuationStamp='',boxMode=false,boxStart=null,boxEnd=null,painted=null,errorNodes='';
        const ruleBack=[],errorBack=[];
        function defaults(){return {search:'',metric:null,rule_start:'0',check:null,error_start:'0',query:null,in_view:false,selected_only:false,waived:null,selected:null,
            markers:true,shown:true,jump_scale:null,zoom_lock:false,jump_active:false,focus_visible:false,cd:null};}
        function key(c){return c&&c.drc?c.view_id+':'+c.epoch+':'+c.drc.id+':'+c.drc.revision:'';}
        function context(){const c=o.context();return c&&key(c)===bound?c:null;}
        function note(s){el('gd-status').textContent=s;}
        function stamp(){const c=context();return c?bound+':'+c.state_rev:'';}
        function filterStamp(){return data?[bound,data.check,data.waived,data.in_view?stamp():'',data.selected_only?groups.revision:''].join(':'):'';}
        function clearStep(){continuation=null;continuationStamp='';el('gd-step-continue').hidden=true;el('gd-step-status').textContent='';}
        function boxReset(off){boxStart=boxEnd=null;if(off){boxMode=false;}
            el('gd-box').setAttribute('aria-pressed',String(boxMode));el('gd-box-status').textContent=boxMode?'Click the first corner · drag still pans.':'e: two corners · current rule/page only · Shift adds · Ctrl/Cmd toggles.';
            if(o.modeChanged){o.modeChanged(boxMode);}o.repaint();}
        function controls(){const c=context(),enabled=!!c&&ready&&!task;
            ['search','filter','waived','in-view','selected-only','markers','rule-prev','rule-next','error-prev','error-next','clear','go','fit','step-prev','step-next','step-continue','box'].forEach(function(n){el('gd-'+n).disabled=!enabled;});
            ['step-prev','step-next','step-continue'].forEach(function(n){el('gd-'+n).disabled=!enabled||data.check===null||data.in_view&&c.pending;});
            el('gd-step-continue').hidden=!continuation;
            el('gd-box').disabled=!enabled||data.check===null||!data.markers||c.pending;
            el('gd-rule-prev').disabled=!enabled||!ruleBack.length;el('gd-rule-next').disabled=!enabled||ruleNext===null;
            el('gd-error-prev').disabled=!enabled||!errorBack.length;el('gd-error-next').disabled=!enabled||errorNext===null;
            ['go','fit'].forEach(function(n){el('gd-'+n).disabled=!enabled||!selected||c.mode!=='explore'||c.pending;});
            el('gd-reload').disabled=!c;el('gd-panel').hidden=!bound;
        }
        function sync(){if(!data){return;}el('gd-search').value=data.search;el('gd-waived').value=data.waived===null?'all':data.waived?'yes':'no';
            ['in-view','selected-only','markers'].forEach(function(n){el('gd-'+n).checked=data[n.replace('-','_')];});}
        function cancel(){turn++;if(task){task.cancelled=true;if(task.abort){task.abort();}}task=null;}
        function reset(){cancel();bound='';ready=false;metadata=groups=data=null;rules=rows=[];selected=points=null;complete=false;listRev=null;liveDirty=false;
            painted=null;errorNodes='';clearStep();boxReset(true);
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
                if(view&&path==='/read'&&valid(t)&&context().state_rev!==c.state_rev){e.viewChanged=true;}throw e;
            }check(t);
            if(!v||v.view_id!==c.view_id||v.revision!==c.drc.revision||!v.data){throw Error('Wrong guest DRC response');}
            if(view&&context().state_rev!==c.state_rev){const e=Error('View changed during review operation');e.viewChanged=path==='/read';throw e;}
            return v.data;
        }
        async function read(t,body,view){return call(t,'POST','/read',{body:body},view);}
        function selection(v){const c=context();groups=o.selection.decode({view_id:c.view_id,revision:c.drc.revision,state:v},{view:c.view_id,revision:c.drc.revision},P);}
        function contains(r){return groups&&groups.rules.has(r.check)&&groups.rules.get(r.check).has(r.local);}
        function bbox(b){if(!Array.isArray(b)||b.length!==4){throw Error('Invalid DRC bounds');}b.forEach(P.decimal);const n=b.map(Number);if(n[0]>n[2]||n[1]>n[3]){throw Error('Invalid DRC bounds');}return n;}
        function record(r){if(!r){throw Error('Missing DRC record');}['check','local','global','points'].forEach(function(k){P.counter(r[k],true);});
            if(!['p','e'].includes(r.kind)||![0,1].includes(r.status)){throw Error('Invalid DRC record');}bbox(r.bbox_um);return r;}
        function recordRows(v){if(!Array.isArray(v)||v.length>64){throw Error('Invalid DRC page');}return v.map(record);}
        function same(a,b){return !!a&&!!b&&a.check===b.check&&a.local===b.local;}
        function render(){
            if(!selected){el('gd-selected').textContent='';}
            el('gd-rules').textContent='';rules.forEach(function(r){const b=doc.createElement('button');b.textContent=r.name+' · '+r.errors+' errors';b.disabled=!ready||!!task;
                b.setAttribute('aria-pressed',String(data.check===r.check));b.onclick=function(){run(async function(t){clearStep();boxReset(true);data.check=r.check;data.error_start='0';data.selected=null;errorBack.length=0;selected=points=null;complete=false;await details(t);await errors(t);await save(t);});};el('gd-rules').appendChild(b);});
            // Preserve the node between first click and dblclick. Only the
            // in-flight target can accept a second, explicit focus intent.
            const signature=JSON.stringify(rows);if(signature!==errorNodes){el('gd-errors').textContent='';errorNodes=signature;
                rows.forEach(function(r){const b=doc.createElement('button');b.onclick=function(e){if(!e||e.detail!==2){choose(r,e,false);}};
                    b.ondblclick=function(e){choose(r,e,true);};el('gd-errors').appendChild(b);});}
            rows.forEach(function(r,i){const b=el('gd-errors').children[i];b.textContent=(contains(r)?'✓ ':'')+'#'+r.global+' · '+(r.status?'waived':'active');
                b.disabled=!ready||!!task&&!same(task.choose,r);
                b.setAttribute('aria-pressed',String(!!selected&&selected.check===r.check&&selected.local===r.local));
            });
            el('gd-selection').textContent=groups?groups.total+' selected · private to this guest':'';controls();o.repaint();
        }
        function mode(e){return e&&(e.ctrlKey||e.metaKey)?'toggle':e&&e.shiftKey?'add':'replace';}
        async function selectRecord(t,r){selected=r;data.selected={check:r.check,error:r.local};points=null;complete=false;await save(t);await geometry(t);}
        function choose(r,e,twice){const c=context();if(!ready||!c){return false;}
            if(task){if(twice&&same(task.choose,r)&&task.view===stamp()){task.focus=true;return true;}return false;}
            if(twice&&same(selected,r)){run(function(t){return focusRead(t,r,true);});return true;}
            clearStep();boxReset(true);run(async function(t){t.choose=r;t.view=stamp();t.focus=!!twice;render();
                selection(await call(t,'POST','/selection',{base_selection_rev:groups.revision,body:{kind:'apply',check:r.check,errors:[r.local],mode:mode(e)}}));
                await selectRecord(t,r);if(data.selected_only){await errors(t);}
                if(t.focus&&t.view===stamp()&&!context().pending){await focusRead(t,r,true);}
            });return true;
        }
        function step(backwards,resume){if(!ready||task||!context()||data.check===null||data.in_view&&context().pending){return false;}
            if(resume&&(!continuation||continuationStamp!==filterStamp())){clearStep();return false;}
            const body=resume?continuation:{kind:'filtered_step',check:data.check,backwards:backwards,
                after:selected&&selected.check===data.check&&(data.waived===null||(selected.status===1)===data.waived)?selected.local:null,
                cursor:null,waived:data.waived,in_view:data.in_view,selection_rev:data.selected_only?groups.revision:null};
            clearStep();boxReset(true);const expected=filterStamp();
            run(async function(t){t.stepRead=true;const v=await read(t,body,body.in_view);t.stepRead=false;if(filterStamp()!==expected){return;}
                const result=o.steps.decode(v,body,P,record,contains);continuation=result.continuation;continuationStamp=expected;
                if(continuation){el('gd-step-status').textContent='Search incomplete · '+v.scanned+' slots checked. Continue search explicitly; no background scan.';return;}
                if(!result.hit){el('gd-step-status').textContent='No matching errors in the current rule.';return;}
                const r=result.hit;if(!rows.some(function(v){return v.check===r.check&&v.local===r.local;})){data.error_start=r.local;errorBack.length=0;await errors(t);}
                // Traversal changes the focused record, not the selected set:
                // selected-only traversal must not destroy its own filter.
                await selectRecord(t,r);el('gd-step-status').textContent='Error #'+r.global+' · camera unchanged; use Go or Frame error.';
            });return true;
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
            if(continuation&&continuationStamp!==filterStamp()){clearStep();}
            if(painted&&(painted.stamp!==stamp()||c.pending)){painted=null;boxReset(true);}
            if(c&&ready&&data.in_view&&listRev!==c.state_rev){if(task){liveDirty=true;}else{run(async function(t){data.error_start='0';errorBack.length=0;await errors(t);});}}
        }
        async function focusRead(t,r,fit){const c=context();if(!c||c.mode!=='explore'||c.pending){return;}const rev=c.state_rev;
            const v=await read(t,{kind:'focus',check:r.check,error:r.local,fit:fit,isolate:false},true);
            if(v.check!==r.check||v.local!==r.local||!v.navigation||v.navigation.kind!=='goto'||!Array.isArray(v.navigation.center_um)||v.navigation.center_um.length!==2){throw Error('Invalid error navigation');}
            v.navigation.center_um.forEach(P.decimal);if(v.navigation.width_um!==undefined){if(!(Number(P.decimal(v.navigation.width_um))>0)){throw Error('Invalid error width');}}
            if(context().state_rev!==rev||!o.navigate(v.navigation,c)){throw Error('View input changed; select Go again.');}
        }
        function focus(fit){if(selected){run(function(t){return focusRead(t,selected,fit);});}}
        el('gd-reload').onclick=reload;
        el('gd-filter').onclick=function(){run(async function(t){clearStep();boxReset(true);data.search=el('gd-search').value;data.rule_start='0';data.error_start='0';data.check=null;data.selected=null;selected=points=null;complete=false;
            ruleBack.length=errorBack.length=0;await loadRules(t);if(rules.length){data.check=rules[0].check;}await details(t);await errors(t);await save(t);});};
        el('gd-waived').onchange=function(){run(async function(t){clearStep();boxReset(true);const v=el('gd-waived').value;data.waived=v==='all'?null:v==='yes';data.rule_start=data.error_start='0';ruleBack.length=errorBack.length=0;await loadRules(t);await errors(t);await save(t);});};
        ['in-view','selected-only','markers'].forEach(function(n){el('gd-'+n).onchange=function(){run(async function(t){data[n.replace('-','_')]=el('gd-'+n).checked;
            clearStep();boxReset(true);if(n!=='markers'){data.error_start='0';errorBack.length=0;await errors(t);}await save(t);});};});
        [['rule',ruleBack],['error',errorBack]].forEach(function(pair){const name=pair[0],back=pair[1];
            ['prev','next'].forEach(function(direction){el('gd-'+name+'-'+direction).onclick=function(){run(async function(t){const field=name+'_start',next=name==='rule'?ruleNext:errorNext;
                clearStep();boxReset(true);if(direction==='prev'){if(!back.length){return;}data[field]=back.pop();}else{if(next===null){return;}if(back.length===64){back.shift();}back.push(data[field]);data[field]=next;}
                if(name==='rule'){await loadRules(t);}else{await errors(t);}await save(t);});};});});
        el('gd-clear').onclick=function(){run(async function(t){clearStep();boxReset(true);selection(await call(t,'POST','/selection',{base_selection_rev:groups.revision,body:{kind:'clear_all'}}));if(data.selected_only){data.error_start='0';await errors(t);}});};
        el('gd-go').onclick=function(){focus(false);};el('gd-fit').onclick=function(){focus(true);};
        el('gd-step-prev').onclick=function(){step(true,false);};el('gd-step-next').onclick=function(){step(false,false);};el('gd-step-continue').onclick=function(){step(false,true);};
        function toggleBox(){if(boxMode){boxReset(true);return true;}const c=context();if(!ready||task||!c||c.pending||data.check===null||!data.markers){return false;}boxMode=true;boxReset(false);return true;}
        el('gd-box').onclick=toggleBox;
        function hitContext(x,y){const c=context(),v=painted,r=o.rect?o.rect():v&&v.rect;if(!ready||!c||c.pending||!data.markers||!v||v.stamp!==stamp()||!r||
            !Number.isFinite(x)||!Number.isFinite(y)||!(r.width>0&&r.height>0)||x<r.left||y<r.top||x>=r.left+r.width||y>=r.top+r.height){return null;}return Object.assign({},v,{rect:r});}
        function world(v,x,y){const p=v.p,r=v.rect,px=(x-r.left)/r.width*v.size[0]+p.origin[0],py=(y-r.top)/r.height*v.size[1]+p.origin[1];
            const xy=[(p.bbox[0]+px*p.step[0])*p.dbu,(p.bbox[3]-py*p.step[1])*p.dbu];return xy.every(Number.isFinite)?xy:null;}
        function click(x,y,twice,e){const v=hitContext(x,y);if(!v){return false;}
            if(boxMode){if(task){return true;}const xy=world(v,x,y);if(!xy){return true;}
                if(!boxStart){boxStart=boxEnd=xy;el('gd-box-status').textContent='Click the opposite corner · Esc cancels corner.';o.repaint();return true;}
                const a=boxStart,b=xy,area=[Math.min(a[0],b[0]),Math.min(a[1],b[1]),Math.max(a[0],b[0]),Math.max(a[1],b[1])];
                boxReset(false);clearStep();const body={kind:'apply',check:data.check,errors:rows.map(function(r){return r.local;}),mode:mode(e),bbox_um:area.map(String),waived:data.waived};
                run(async function(t){selection(await call(t,'POST','/selection',{base_selection_rev:groups.revision,body:body},true));if(data.selected_only){data.error_start='0';await errors(t);}});return true;
            }
            let best=null,distance=37;v.hits.forEach(function(h){const dx=v.rect.left+h.xy[0]/v.size[0]*v.rect.width-x,dy=v.rect.top+h.xy[1]/v.size[1]*v.rect.height-y,d=dx*dx+dy*dy;
                if(d<=36&&d<distance){best=h.row;distance=d;}});if(!best){return false;}choose(best,e,twice);return true;
        }
        function move(x,y){if(!boxMode||!boxStart){return;}const v=hitContext(x,y),xy=v&&world(v,x,y);if(xy){boxEnd=xy;o.repaint();}}
        function keyInput(k,shift){if(!ready||!context()){return false;}
            if(k==='Escape'&&boxMode){boxReset(!boxStart);return true;}if(k==='e'){return toggleBox();}
            if(k==='Escape'&&(continuation||task&&task.stepRead)){if(task){cancel();}clearStep();render();note('Error search stopped; no selection edit was replayed.');return true;}
            if(k===','||k==='.'||k==='Tab'){return step(k===','||k==='Tab'&&!!shift,false);}return false;
        }
        el('gd-errors').onkeydown=function(e){if(!e.ctrlKey&&!e.metaKey&&!e.altKey&&!e.isComposing&&(e.key==='ArrowUp'||e.key==='ArrowDown')){if(step(e.key==='ArrowUp',false)){e.preventDefault();}}};
        function paint(ctx,p,size,rect){painted=null;if(!p||!size||!context()||!data||!data.markers||!data.shown){return;}const hits=[];ctx.save();
            const markers=rows.slice();if(selected&&!markers.some(function(r){return r.check===selected.check&&r.local===selected.local;})){markers.push(selected);}
            markers.forEach(function(r){const b=bbox(r.bbox_um),xy=G.point(p,b[0]*.5+b[2]*.5,b[1]*.5+b[3]*.5);if(!xy.every(Number.isFinite)){return;}
                if(xy[0]<-4||xy[0]>size[0]+4||xy[1]<-4||xy[1]>size[1]+4){return;}
                const center=xy.map(Math.round);hits.push({row:r,xy:center});ctx.fillStyle=contains(r)?'#f4cd64':r.status?'#70da9a':'#ff6969';ctx.fillRect(center[0]-3,center[1]-3,7,7);});
            if(selected){ctx.strokeStyle=selected.status?'#70da9a':'#ff6969';ctx.lineWidth=2;
                if(complete&&points){ctx.beginPath();for(let i=0;i<points.length;i+=2){const xy=G.point(p,points[i],points[i+1]);if(i===0||selected.kind==='e'&&i%4===0){ctx.moveTo(xy[0],xy[1]);}else{ctx.lineTo(xy[0],xy[1]);}}if(selected.kind==='p'){ctx.closePath();}ctx.stroke();}
                else{const b=bbox(selected.bbox_um),a=G.point(p,b[0],b[3]),z=G.point(p,b[2],b[1]);ctx.strokeRect(a[0],a[1],z[0]-a[0],z[1]-a[1]);}}
            if(boxMode&&boxStart&&boxEnd){const a=G.point(p,boxStart[0],boxStart[1]),b=G.point(p,boxEnd[0],boxEnd[1]);ctx.strokeStyle='#f4cd64';ctx.lineWidth=rect&&rect.width>0?size[0]/rect.width:1;
                ctx.setLineDash([5*ctx.lineWidth,3*ctx.lineWidth]);ctx.strokeRect(a[0],a[1],b[0]-a[0],b[1]-a[1]);ctx.setLineDash([]);}
            ctx.restore();painted={p:p,size:size.slice(),rect:rect,stamp:stamp(),hits:hits};
        }
        controls();return {changed:changed,reset:reset,paint:paint,click:click,move:move,key:keyInput,
            active:function(){return boxMode;},leave:function(){boxReset(true);}};
    }
    const api={bind:bind};if(typeof module==='object'&&module.exports){module.exports=api;}else{root.FloeGuestDRC=api;}
}(typeof window==='object'?window:this));
