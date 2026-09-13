/* Mouse pan is a temporary pixel translation; only release submits navigation. */
(function (root) {
    'use strict';
    function bind(port) {
        let drag = null, paint = null;
        function draw() {
            paint = null;
            if (drag && drag.moved) { port.preview([-drag.dx, -drag.dy], true); }
        }
        function cancel() {
            if (paint !== null) { port.cancelAnimationFrame(paint); paint = null; }
            const had = !!drag; drag = null;
            if (had) { port.preview(null, true); port.cursor(false); }
        }
        function update(event) {
            if (!drag) { return; }
            if (drag.stamp !== port.stamp() || !port.ready()) { cancel(); return; }
            const x = event.clientX - drag.x, y = event.clientY - drag.y;
            if (!drag.moved && Math.abs(x) <= 8 && Math.abs(y) <= 8) { return; }
            drag.moved = true;
            drag.dx = Math.max(-drag.width, Math.min(drag.width, Math.round(x * drag.dpr)));
            drag.dy = Math.max(-drag.height, Math.min(drag.height, Math.round(y * drag.dpr)));
        }
        port.viewport.addEventListener('mousedown', function (event) {
            if (drag) { drag.plain = false; return; }
            if ((event.button !== 0 && event.button !== 1) || event.ctrlKey || event.metaKey || event.altKey || !port.ready()) { return; }
            const d = port.dimensions();
            drag = {x: event.clientX, y: event.clientY, button: event.button, stamp: port.stamp(),
                width: d.pixels[0], height: d.pixels[1], dpr: d.dpr, dx: 0, dy: 0, moved: false,
                plain: !event.shiftKey && (event.buttons === undefined || event.buttons === 1)};
            event.preventDefault(); port.viewport.focus(); port.cursor(true);
        });
        port.window.addEventListener('mousemove', function (event) {
            if (!drag) { return; }
            const mask = drag.button === 0 ? 1 : 4;
            if (typeof event.buttons === 'number' && (event.buttons & mask) === 0) { cancel(); return; }
            if (typeof event.buttons === 'number' && event.buttons !== mask) { drag.plain = false; }
            update(event);
            if (drag && paint === null) { paint = port.requestAnimationFrame(draw); }
        });
        port.window.addEventListener('mouseup', function (event) {
            if (!drag || event.button !== drag.button) { return; }
            update(event);
            if (!drag) { return; }
            if (paint !== null) { port.cancelAnimationFrame(paint); paint = null; }
            const done = drag;
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
                if (!done.moved && done.button === 0 && done.plain && port.click &&
                    !event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey &&
                    (event.buttons === undefined || event.buttons === 0)) {
                    port.click(event.clientX, event.clientY, event.detail === 2);
                }
            }
        });
        port.window.addEventListener('blur', cancel);
        port.window.addEventListener('resize', cancel);
        port.window.addEventListener('pagehide', cancel);
        port.document.addEventListener('visibilitychange', function () { if (port.document.hidden) { cancel(); } });
        return Object.freeze({cancel: cancel, active: function () { return drag !== null; }});
    }
    if (typeof module !== 'undefined' && module.exports) { module.exports = {bind: bind}; }
    else { root.FloeGestures = {bind: bind}; }
}(typeof window === 'undefined' ? this : window));
