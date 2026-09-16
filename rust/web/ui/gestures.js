/* Mouse pan/band previews are screen-only; only release submits navigation. */
(function (root) {
    'use strict';
    function wheelNavigation(event, size, rect) {
        const mode = event.deltaMode === undefined ? 0 : event.deltaMode;
        if (![0, 1, 2].includes(mode) || !Number.isFinite(event.deltaY) || !event.deltaY ||
            event.buttons || !Number.isFinite(event.clientX) || !Number.isFinite(event.clientY)) { return null; }
        // GTK caps each event, including accumulated smooth deltas, at one
        // 0.96 step. Keep fractions of the reported unit; there is no universal
        // DOM pixel/line/page -> GDK physical-wheel conversion. Never multiply
        // a large pixel delta into a many-step zoom. Field sensitivity is separate.
        const factor = Math.pow(0.96, -Math.max(-1, Math.min(1, event.deltaY)));
        if (factor === 1) { return null; }
        const anchor = [(event.clientX - rect.left - (size.left || 0)) * size.dpr / size.pixels[0],
            (event.clientY - rect.top - (size.top || 0)) * size.dpr / size.pixels[1]];
        if (!anchor.every(Number.isFinite)) { return null; }
        return {kind: 'zoom', factor: factor, anchor: anchor.map(v => Math.max(0, Math.min(1, v)))};
    }
    function bind(port) {
        let drag = null, paint = null;
        function draw() {
            paint = null;
            if (drag && drag.band) { port.bandPreview(bandPreview(drag)); }
            else if (drag && drag.moved) { port.preview([-drag.dx, -drag.dy], true); }
        }
        function bandPreview(d) {
            return {start:d.start,end:d.end,outward:d.x-d.minX>d.maxX-d.x,dimensions:d.dimensions};
        }
        function cancel() {
            if (paint !== null) { port.cancelAnimationFrame(paint); paint = null; }
            const had = drag; drag = null;
            if (had) { if(had.band){port.bandPreview(null);}else{port.preview(null, true);} port.cursor(false); }
        }
        function update(event) {
            if (!drag) { return; }
            if (drag.stamp !== port.stamp() || !port.ready()) { cancel(); return; }
            if (drag.band) {
                if (port.bandReady && !port.bandReady()) { cancel(); return; }
                const dx=Math.max(-drag.width/drag.dpr,Math.min(drag.width/drag.dpr,event.clientX-drag.x));
                const dy=Math.max(-drag.height/drag.dpr,Math.min(drag.height/drag.dpr,event.clientY-drag.y));
                if(!Number.isFinite(dx)||!Number.isFinite(dy)){cancel();return;}
                drag.minX=Math.min(drag.minX,drag.x+dx);drag.maxX=Math.max(drag.maxX,drag.x+dx);
                drag.dx=dx;drag.dy=dy;
                drag.end=[drag.start[0]+dx*drag.dpr/drag.width,drag.start[1]+dy*drag.dpr/drag.height];return;
            }
            const x = event.clientX - drag.x, y = event.clientY - drag.y;
            if (!drag.moved && Math.abs(x) <= 8 && Math.abs(y) <= 8) { return; }
            // Ctrl/Cmd was not a pan gesture before object picking. Opting in
            // to modifier clicks must not introduce modifier-drag navigation.
            if (drag.objectOnly) { cancel(); return; }
            drag.moved = true;
            drag.dx = Math.max(-drag.width, Math.min(drag.width, Math.round(x * drag.dpr)));
            drag.dy = Math.max(-drag.height, Math.min(drag.height, Math.round(y * drag.dpr)));
        }
        port.viewport.addEventListener('mousedown', function (event) {
            if (drag) { if(drag.band){cancel();}else{drag.plain = drag.unchorded = false;} return; }
            const box = !!(port.selectionMode && port.selectionMode());
            const band=event.button===2&&!!port.band;
            if ((!band && event.button !== 0 && event.button !== 1) || ((event.ctrlKey || event.metaKey) && !box && !port.objectClicks) || event.altKey || !port.ready()) { return; }
            const d = port.dimensions();
            if(band){
                if((event.buttons!==undefined&&event.buttons!==2)||(port.bandReady&&!port.bandReady())){return;}
                const r=port.viewport.getBoundingClientRect();
                const start=[(event.clientX-r.left-(d.left||0))*d.dpr/d.pixels[0],(event.clientY-r.top-(d.top||0))*d.dpr/d.pixels[1]].map(v=>Math.max(0,Math.min(1,v)));
                drag={band:true,button:2,stamp:port.stamp(),x:event.clientX,y:event.clientY,minX:event.clientX,maxX:event.clientX,
                    dx:0,dy:0,width:d.pixels[0],height:d.pixels[1],dpr:d.dpr,start:start,end:start.slice(),dimensions:d};
                event.preventDefault();port.viewport.focus();port.cursor(true);return;
            }
            drag = {x: event.clientX, y: event.clientY, button: event.button, stamp: port.stamp(),
                width: d.pixels[0], height: d.pixels[1], dpr: d.dpr, dx: 0, dy: 0, moved: false,
                box: box, modifiers: [!!event.ctrlKey, !!event.metaKey, !!event.shiftKey],
                objectOnly: !!port.objectClicks && !box && !!(event.ctrlKey || event.metaKey),
                unchorded: event.buttons === undefined || event.buttons === (event.button === 0 ? 1 : 4),
                plain: !event.shiftKey && (event.buttons === undefined || event.buttons === 1)};
            event.preventDefault(); port.viewport.focus(); port.cursor(true);
        });
        port.window.addEventListener('mousemove', function (event) {
            if (!drag) { return; }
            const mask = drag.button === 0 ? 1 : drag.button===2 ? 2 : 4;
            if (typeof event.buttons === 'number' && (event.buttons & mask) === 0) { cancel(); return; }
            if(drag.band && typeof event.buttons==='number' && event.buttons!==mask){cancel();return;}
            if (typeof event.buttons === 'number' && event.buttons !== mask) { drag.plain = drag.unchorded = false; }
            update(event);
            if (drag && paint === null) { paint = port.requestAnimationFrame(draw); }
        });
        port.window.addEventListener('mouseup', function (event) {
            if (!drag || event.button !== drag.button) { return; }
            update(event);
            if (!drag) { return; }
            if (paint !== null) { port.cancelAnimationFrame(paint); paint = null; }
            const done = drag;
            if(done.band){
                drag=null;port.bandPreview(null);port.cursor(false);
                if(event.buttons!==undefined&&event.buttons!==0){return;}
                const outward=done.x-done.minX>done.maxX-done.x;
                const axes=[(outward?-done.dx:done.dx)>=5,Math.abs(done.dy)>=5];
                if(axes.some(Boolean)){port.band({kind:'band',start:done.start,end:done.end,axes:axes,outward:outward});}
                else if(Math.max(done.maxX-done.x,done.x-done.minX)>=5 && port.notice){port.notice('Zoom band cancelled');}
                return;
            }
            if (done.moved) { port.preview([-done.dx, -done.dy], true); }
            drag = null; port.preview(null, false); port.cursor(false);
            if (done.moved && (done.dx || done.dy)) {
                // No 16px snap for a mouse gesture: a sub-period release must
                // render its correct native phase rather than silently shift.
                port.pan({kind: 'pan', x: -done.dx / done.width, y: done.dy / done.height, snap: false});
            } else {
                port.preview(null, true);
                // Only an unmodified, unchorded left-button release is a
                // marker click. Never turn a cancelled or returned drag into
                // a pick. MouseEvent.detail supplies the browser's double-
                // click interval; no independent timer races navigation.
                if (!done.moved && done.button === 0 && done.unchorded && port.click &&
                    done.box === !!(port.selectionMode && port.selectionMode()) && !event.altKey &&
                    (event.buttons === undefined || event.buttons === 0)) {
                    if (done.box) { port.click(event.clientX, event.clientY, event.detail === 2, {ctrlKey: !!event.ctrlKey, metaKey: !!event.metaKey, shiftKey: !!event.shiftKey}); }
                    else if (port.objectClicks) {
                        if (done.modifiers.every(function (v, i) { return v === [!!event.ctrlKey, !!event.metaKey, !!event.shiftKey][i]; })) {
                            if (done.modifiers.some(Boolean)) { port.click(event.clientX, event.clientY, event.detail === 2, {ctrlKey: !!event.ctrlKey, metaKey: !!event.metaKey, shiftKey: !!event.shiftKey}); }
                            else { port.click(event.clientX, event.clientY, event.detail === 2); }
                        }
                    }
                    else if (done.plain && !event.ctrlKey && !event.metaKey && !event.shiftKey) { port.click(event.clientX, event.clientY, event.detail === 2); }
                }
            }
        });
        port.window.addEventListener('blur', cancel);
        port.viewport.addEventListener('contextmenu', function(event){if(port.band){event.preventDefault();}});
        port.window.addEventListener('resize', cancel);
        port.window.addEventListener('pagehide', cancel);
        port.document.addEventListener('visibilitychange', function () { if (port.document.hidden) { cancel(); } });
        return Object.freeze({cancel: cancel, active: function () { return drag !== null; },
            moving:function(){return !!drag&&(!!drag.band||!!drag.moved);}, bandActive:function(){return !!drag&&!!drag.band;}});
    }
    if (typeof module !== 'undefined' && module.exports) { module.exports = {bind: bind, wheelNavigation: wheelNavigation}; }
    else { root.FloeGestures = {bind: bind, wheelNavigation: wheelNavigation}; }
}(typeof window === 'undefined' ? this : window));
