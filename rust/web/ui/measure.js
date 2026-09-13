/* Ruler input and presentation. World coordinates, dominant axis and
 * measurements are computed in Rust. No measurements are inferred from PNGs. */
(function (root) {
    'use strict';
    function keys(v, fields) {
        if (!v || typeof v !== 'object' || Array.isArray(v) || Object.keys(v).sort().join(',') !== fields.slice().sort().join(',')) { throw new Error('Invalid measurement'); }
    }
    function point(v, P, Q) {
        if (!Array.isArray(v) || v.length !== 2) { throw new Error('Invalid measurement point'); }
        return v.map(function (s) {
            if (typeof s !== 'string' || s.length > 64) { throw new Error('Invalid measurement coordinate'); }
            if (/^(0|-?[1-9][0-9]*)$/.test(s)) { Q.i64(s, P); }
            else if (/^-?(0|[1-9][0-9]*)\.(25|5|75)$/.test(s)) {
                const whole=s.split('.')[0];Q.i64(whole==='-0'?'0':whole,P);
                if (whole==='9223372036854775807' || whole==='-9223372036854775808') { throw new Error('Invalid midpoint'); }
            }
            else { const n=Number(P.decimal(s)); if (Math.abs(n)>4611686018427387904 || n===Math.trunc(n)) { throw new Error('Invalid measurement fraction'); } }
            return s;
        });
    }
    function decode(m, t, P, Q) {
        keys(m, ['type','seq','view_id','connection_epoch','anchor','point_dbu','snap','segment']);
        keys(m.anchor, Object.keys(t.anchor));
        if (m.view_id !== t.view || m.connection_epoch !== t.epoch || Object.keys(t.anchor).some(function (k) { return m.anchor[k] !== t.anchor[k]; })) { throw new Error('Measurement identity mismatch'); }
        const p = point(m.point_dbu, P, Q);
        if (m.snap !== null && !['vertex','edge'].includes(m.snap)) { throw new Error('Invalid snap kind'); }
        if (t.snap === null && m.snap !== null) { throw new Error('Unexpected snap'); }
        if (m.segment === null) { if (t.start !== null) { throw new Error('Missing measurement'); } return {point:p,snap:m.snap,segment:null}; }
        keys(m.segment, ['endpoints_dbu','delta_um','distance_um']);
        const s = m.segment;
        if (!t.start || !Array.isArray(s.endpoints_dbu) || s.endpoints_dbu.length !== 2 || !Array.isArray(s.delta_um) || s.delta_um.length !== 2) { throw new Error('Invalid segment'); }
        s.endpoints_dbu.forEach(function (v) { point(v, P, Q); });
        if (JSON.stringify(s.endpoints_dbu[0]) !== JSON.stringify(t.start)) { throw new Error('Measurement start changed'); }
        s.delta_um.forEach(function (n) { P.decimal(n); });
        if (Number(P.decimal(s.distance_um)) < 0) { throw new Error('Invalid distance'); }
        return {point:p,snap:m.snap,segment:s};
    }
    function bind(o) {
        const P=o.protocol, Q=o.query, el=function (id) { return o.document.getElementById(id); };
        const canvas=el('ruler-canvas'), ctx=canvas.getContext('2d');
        const book=o.history || o.rulers.history();
        let enabled=false, start=null, preview=null, bound='', stamp='', turn=0, locked=false, stopped=false, auto=null;
        let pending=null, timer=null, last=-Infinity, painting=null, projection=null, size=null, snapPreference=true;
        const snap=Q.bind({protocol:P,context:o.context,send:o.send,now:o.now,setTimeout:o.setTimeout,clearTimeout:o.clearTimeout});
        function scope() { try { return Q.scope(o.context(),P,true); } catch (e) { return null; } }
        function status(s) { el('ruler-status').textContent=s; }
        function format(s) { const n=Number(s); return n!==0 && (Math.abs(n)<.0001 || Math.abs(n)>=1e9) ? n.toExponential(4) : n.toFixed(4); }
        function paintLater() { if (!stopped && painting===null) { painting=o.window.requestAnimationFrame(function () { painting=null; draw(); }); } }
        function refresh() {
            const entries=book.entries(), segments=entries.filter(function (e) { return e.kind!=='cd' && e.value; }).map(function (e) { return e.value; });
            el('ruler-mode').setAttribute('aria-pressed',String(enabled));
            el('ruler-mode').textContent=enabled?'Ruler on (r)':'Ruler (r)';
            el('ruler-pop').disabled=!entries.length; el('ruler-clear').disabled=!entries.length&&!start&&!locked;
            const s=preview&&preview.segment || (segments.length?segments[segments.length-1]:null);
            el('ruler-details').textContent=s?'Length '+format(s.distance_um)+' µm · Δx '+format(s.delta_um[0])+' · Δy '+format(s.delta_um[1])+' µm\n'+
                'DBU: '+s.endpoints_dbu[0].join(', ')+' → '+s.endpoints_dbu[1].join(', '):start?'First point (DBU): '+start.join(', '):'';
            el('ruler-count').textContent=entries.length+' rulers'+(auto?' · gap pending':'');
            el('ruler-auto').textContent=entries.filter(function (e) { return e.kind==='auto' && e.value; }).map(function (e,i) {
                return 'BBox gap '+(i+1)+': '+format(e.value.distance_um)+' µm';
            }).join('\n');paintLater();
        }
        function retire(keepPreview) {
            ++turn; locked=false; if (!keepPreview) { preview=null; }
            if (pending && pending.timeout!==null) { o.clearTimeout(pending.timeout); }
            pending=null; if (timer!==null) { o.clearTimeout(timer); timer=null; }
        }
        function cancelAuto() { if (auto) { o.clearTimeout(auto.timeout);auto=null;book.clear('auto'); } }
        function interrupt() { retire(); cancelAuto(); snap.cancel('snap'); refresh(); }
        function leave() { enabled=false; start=null; interrupt(); status('Ruler off.'); o.modeChanged(); }
        function changed() {
            const c=o.context(), key=c?[c.id,c.state.dataset_revision,c.state.worker_epoch].join(':'):'';
            const s=scope(), next=s?s.key:'';
            if (key!==bound) { enabled=false; start=null; bound=key;book.clear('manual');book.clear('auto');interrupt();status('Click two points to measure.');o.modeChanged(); }
            if (next!==stamp) { if (locked || auto) { status('View changed; pending measurement was discarded.'); } stamp=next; interrupt(); }
            snap.changed();
            if (auto && JSON.stringify(selection())!==auto.selection) { cancelAuto();status('Selection changed; gap request was discarded.'); }
            el('ruler-mode').disabled=!s;
            el('ruler-snap').disabled=!c || !c.state.capabilities.query;
            el('ruler-snap').checked=snapPreference && !el('ruler-snap').disabled;
            el('ruler-availability').textContent=!s?'Waiting for a displayed frame.':!c.state.capabilities.query?'Geometry snap unavailable in jobdeck; cursor measurement is available.':
                !Q.scope(c,P)?'Geometry incomplete; disable snap for cursor-only measurements.':c.frame.query_scene.summary_layers!=='0'?'Summary layers cannot snap; zoom in, hide them or disable snap.':'';
            return s;
        }
        function finish(t, v) {
            if (pending!==t) { return; }
            if (t.timeout!==null) { o.clearTimeout(t.timeout); } pending=null; locked=false;
            if (t.click) {
                if (!start) { start=v.point; status('Click the second point. Shift: free angle.'); }
                else if (Number(v.segment.distance_um)===0) { start=null; status('Zero-length ruler was not added.'); }
                else if (book.entries().filter(function (e) { return e.kind==='manual'; }).length>=256) { status('256 rulers: delete a ruler before adding more.'); }
                else { book.push('manual',v.segment); start=null; status('Ruler added. Click a new first point.'); }
                preview=t.start===null?v:null;
            } else { preview=v; status(v.snap?'Snapped to '+v.snap+'.':start?'Second point preview.':'Click the first point.'); }
            refresh();
        }
        function fail(t, message) { if (pending!==t) { return; } retire(); status(message); refresh(); }
        function pump() {
            timer=null; const t=pending, s=scope();
            if (!t || t.seq || !s || s.key!==t.stamp || stopped) { return; }
            const wait=80-(o.now()-last); if (wait>0) { timer=o.setTimeout(pump,wait); return; }
            try {
                t.seq=o.send({type:'view.measure',view_id:t.view,connection_epoch:t.epoch,body:{anchor:t.anchor,position:t.position,start_dbu:t.start,free_angle:t.free,snap_query:t.snap}});
                last=o.now();t.timeout=o.setTimeout(function () { fail(t,'Measurement timed out; select the point again.'); },8000);
            } catch (e) { fail(t,'Measurement could not be sent.'); }
        }
        function sample(x,y,free,click) {
            const c=o.context(), s=changed(); if (!enabled || stopped || locked || !s) { return false; }
            const p=Q.position(c,x,y); if (!p) { interrupt(); return false; }
            // Keep the last accepted preview while the cursor moves. The
            // pending point is always resolved anew; it never commits this
            // older preview or flashes the annotation off on every mousemove.
            retire(true); const n=turn; locked=click;
            function submit(snapId) {
                if (n!==turn) { return; }
                const now=scope(); if (!now || now.key!==s.key) { interrupt(); return; }
                pending={view:c.id,epoch:c.state.connection_epoch,anchor:s.anchor,stamp:s.key,position:p,start:start?start.slice():null,
                    free:!!free,click:click,snap:snapId,seq:null,timeout:null};pump();
            }
            if (el('ruler-snap').checked) {
                const ok=snap.request('snap',p,Math.min(64,10*c.size.dpr),'0',function (v, receipt) {
                    if (n!==turn) { return; }
                    if (v.status!=='ok') { locked=false;preview=null;status(v.message);refresh();return; }
                    submit(receipt.query_id);
                });
                if (!ok) { locked=false;status('No exact snap scene. Disable snap to measure cursor coordinates.');refresh();return false; }
            } else { submit(null); }
            status(click?'Resolving ruler point…':'Measuring…');refresh();return true;
        }
        function toggle() {
            if (enabled) { leave();return true; }
            if (!changed()) { return false; }
            enabled=true;status('Click the first point.');refresh();o.modeChanged();measureSelection();return true;
        }
        function selection() { return o.selection?o.selection():[]; }
        function measureSelection() {
            const boxes=selection(), s=scope(), c=o.context();
            if (!s || boxes.length<2) { return; } // GTK keeps the previous auto set with <2 selections.
            const t={view:c.id,epoch:c.state.connection_epoch,anchor:s.anchor,selection:JSON.stringify(boxes),seq:null,timeout:null};
            cancelAuto();book.set('auto',[null],true);auto=t;
            try {
                t.seq=o.send({type:'view.measure_selection',view_id:t.view,connection_epoch:t.epoch,body:{anchor:t.anchor,boxes_dbu:boxes}});
                t.timeout=o.setTimeout(function () { if (auto===t) { cancelAuto();status('Gap measurement timed out; enter ruler mode again.');refresh(); } },8000);
                status('Measuring selected bbox gaps…');refresh();
            } catch (e) { cancelAuto();status('Gap measurement could not be sent.');refresh(); }
        }
        function autoReply(m,t) {
            keys(m,['type','seq','view_id','connection_epoch','anchor','segments']);keys(m.anchor,Object.keys(t.anchor));
            if (m.view_id!==t.view || m.connection_epoch!==t.epoch || Object.keys(t.anchor).some(function (k) { return m.anchor[k]!==t.anchor[k]; }) ||
                !Array.isArray(m.segments) || m.segments.length>128) { throw new Error('Invalid gap response'); }
            m.segments.forEach(function (s) {
                keys(s,['endpoints_dbu','delta_um','distance_um']);
                if (!Array.isArray(s.endpoints_dbu) || s.endpoints_dbu.length!==2 || !Array.isArray(s.delta_um) || s.delta_um.length!==2) { throw new Error('Invalid gap segment'); }
                s.endpoints_dbu.forEach(function (p) { point(p,P,Q); });s.delta_um.forEach(function (n) { P.decimal(n); });
                if (!(Number(P.decimal(s.distance_um))>0)) { throw new Error('Invalid gap distance'); }
            });
            o.clearTimeout(t.timeout);auto=null;book.set('auto',m.segments);
            status(m.segments.length+' selected bbox gaps · not contour distances. Click to add a manual ruler.');refresh();
        }
        function pop(all) {
            const entries=book.entries(), lastEntry=entries[entries.length-1];
            const had=entries.length>0 || (all && (start!==null || locked));
            if (!had) { return false; }
            if (o.cdBusy && o.cdBusy() && entries.some(function (e) { return e.kind==='cd'; }) && (all || lastEntry.kind==='cd')) {
                status('CD restoration is in progress; try deleting again when it finishes.');return true;
            }
            retire();snap.cancel('snap');
            if (all) {
                cancelAuto();book.clear('manual');book.clear('auto');start=null;
                if (o.popCD) { o.popCD(true); }
            } else if (lastEntry.kind==='cd') { if (o.popCD) { o.popCD(false); } }
            else if (lastEntry.kind==='auto' && auto) { cancelAuto(); }
            else { book.pop(); }
            status(all?'Rulers cleared.':'Last ruler deleted.');refresh();return true;
        }
        let overlayVisible=true;
        function draw() {
            const entries=book.entries();
            if (!overlayVisible || !projection || !size || !bound || (!entries.length && !preview)) { canvas.hidden=true;return; }
            const w=size.pixels[0],h=size.pixels[1],dpr=size.dpr,p=projection;
            if (canvas.width!==w || canvas.height!==h) { canvas.width=w;canvas.height=h; }
            canvas.style.width=w/dpr+'px';canvas.style.height=h/dpr+'px';canvas.style.left=size.left+'px';canvas.style.top=size.top+'px';canvas.hidden=false;
            ctx.clearRect(0,0,w,h);ctx.save();ctx.beginPath();ctx.rect(0,0,w,h);ctx.clip();
            function xy(x,y) { return [(x-p.bbox[0])/p.step[0]-p.origin[0],(p.bbox[3]-y)/p.step[1]-p.origin[1]]; }
            const dbu=p.dbu;
            const list=entries.filter(function (e) { return e.value && (e.kind!=='cd' || (el('drc-markers').checked && dbu>0)); }).map(function (e) {
                const s=e.value;
                return e.kind==='cd'?{ends:s.ends.map(function (p) { return p.map(function (n) { return n/dbu; }); }),offset:s.offset,label:s.label}:
                    {ends:s.endpoints_dbu.map(function (v) { return v.map(Number); }),offset:false,label:format(s.distance_um)+' µm'};
            });
            if (preview && preview.segment) { list.push({ends:preview.segment.endpoints_dbu.map(function (v) { return v.map(Number); }),offset:false,label:format(preview.segment.distance_um)+' µm'}); }
            o.rulers.paint(ctx,list,xy,size);
            if (preview) {
                const v=xy(Number(preview.point[0]),Number(preview.point[1])),r=5*dpr;
                ctx.setLineDash([]);ctx.strokeStyle=preview.snap?'#72e8da':'#ffffff';ctx.lineWidth=dpr;ctx.beginPath();
                ctx.moveTo(v[0]-r,v[1]);ctx.lineTo(v[0]+r,v[1]);ctx.moveTo(v[0],v[1]-r);ctx.lineTo(v[0],v[1]+r);ctx.stroke();
            }
            ctx.restore();
        }
        el('ruler-mode').onclick=toggle;
        el('ruler-pop').onclick=function () { pop(false); };el('ruler-clear').onclick=function () { pop(true); };
        el('ruler-snap').checked=true;el('ruler-snap').onchange=function () { snapPreference=el('ruler-snap').checked;interrupt();status('Snap '+(snapPreference?'on.':'off.')); };
        book.watch(refresh);
        return {active:function () { return enabled; },changed:changed,interrupt:interrupt,leave:leave,
            showOverlay:function (show) { overlayVisible=!!show;draw(); },
            flush:function () { if(painting!==null){o.window.cancelAnimationFrame(painting);painting=null;}draw(); },
            click:function (x,y,m) { return sample(x,y,m&&m.shiftKey,true); },
            move:function (x,y,m) { if (enabled && !locked) { sample(x,y,m&&m.shiftKey,false); } },
            key:function (key) {
                if (key==='r') { return toggle(); }
                if (key==='m' && enabled && !el('ruler-snap').disabled) { el('ruler-snap').checked=!el('ruler-snap').checked;el('ruler-snap').onchange();return true; }
                if (key==='k' || key==='K') { return pop(key==='K'); }
                if (key==='Escape') {
                    if (start || locked) { start=null;interrupt();status('Pending ruler cancelled.');return true; }
                    if (enabled) { leave();return true; }return pop(true);
                } return false;
            },
            receive:function (m) {
                if (stopped) { return false; }
                changed(); const t=pending, a=auto;
                if (a && m.seq===a.seq && (m.type==='measure_selection.result' || m.type==='error')) {
                    try { if (m.type==='error') { throw new Error('refused'); } autoReply(m,a); }
                    catch (e) { cancelAuto();status('Gap measurement was refused or invalid; enter ruler mode again.');refresh(); } return true;
                }
                if (m.type==='measure_selection.result') { return true; }
                if (t && t.seq && m.seq===t.seq && (m.type==='measure.result' || m.type==='error')) {
                    if (m.type==='error') { fail(t,'Measurement was refused; select the point again.'); }
                    else { try { finish(t,decode(m,t,P,Q)); } catch (e) { fail(t,'Invalid measurement response; no ruler was added.'); } } return true;
                }
                return enabled?snap.receive(m):m.type==='measure.result';
            },
            paint:function (p,s) { projection=p;size=s;paintLater(); },
            stop:function () { stopped=true;enabled=false;start=null;cancelAuto();book.clear();retire();snap.stop();canvas.hidden=true;canvas.width=1;canvas.height=1;
                if (painting!==null) { o.window.cancelAnimationFrame(painting);painting=null; } },
            resume:function () { stopped=false;snap.resume();changed();refresh(); }};
    }
    const api={bind:bind,decode:decode};
    if (typeof module==='object' && module.exports) { module.exports=api; } else { root.FloeMeasure=api; }
}(typeof window==='object'?window:this));
