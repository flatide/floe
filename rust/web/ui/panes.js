/* Workspace chrome: the left cells/DRC/inspect notebook, the right
 * minimap/palette notebook, the two pane splitters and the tool dialogs that
 * host panels the GTK viewer shows as menu dialogs. Panels keep their own ids
 * and enable/disable logic; this module only decides where they are shown. */
(function (root) {
    'use strict';
    const LEFT_MIN = 156, LEFT_DEFAULT = 260, RIGHT_MIN = 210, RIGHT_DEFAULT = 250;
    function bind(port) {
        const el = port.el, doc = port.document, win = port.window;
        const notebooks = {};
        const dialogs = {};
        let active = null;
        function notebook(name, tabs, initial) {
            if (tabs.some(function (t) { return !el(t.tab) || !el(t.page); })) { return null; }
            const nb = {name: name, tabs: tabs, current: null};
            tabs.forEach(function (t) {
                el(t.tab).onclick = function () { select(nb, t.page); };
                el(t.tab).onkeydown = function (e) {
                    if (e.isComposing || e.keyCode === 229) { return; }
                    if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
                        e.preventDefault();
                        const i = tabs.indexOf(t), n = tabs[(i + (e.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length];
                        select(nb, n.page); el(n.tab).focus();
                    }
                };
            });
            notebooks[name] = nb; select(nb, initial);
            return nb;
        }
        function select(nb, page) {
            nb.current = page;
            nb.tabs.forEach(function (t) {
                const on = t.page === page;
                el(t.tab).setAttribute('aria-selected', String(on)); el(t.tab).tabIndex = on ? 0 : -1;
                el(t.tab).className = 'pane-tab' + (on ? ' active' : '');
                el(t.page).hidden = !on;
            });
            // The preset grids keep the <details> contract of the old panel:
            // they load only while "open", i.e. while the palette page shows.
            if (nb.name === 'right') {
                const presets = el('palette-presets'), want = page === 'palette-page';
                if (!!presets.open !== want) { presets.open = want; if (typeof presets.ontoggle === 'function') { presets.ontoggle(); } }
            }
            if (port.resized) { port.resized(); }
        }
        function raise(name, page) { const nb = notebooks[name]; if (nb && nb.current !== page) { select(nb, page); } }
        // Splitters: pointer drags resize a pane between its minimum and the
        // canvas' own minimum. Sizes are per-tab conveniences, not view state.
        function splitter(id, pane, side, min) {
            const handle = el(id), target = el(pane);
            if (!handle || !target) { return {widen: function () {}}; }
            let drag = null;
            function width() { const w = parseInt(target.style.width, 10); return Number.isFinite(w) ? w : (side === 'left' ? LEFT_DEFAULT : RIGHT_DEFAULT); }
            function apply(w) {
                const limit = Math.max(min, Math.min(w, (win.innerWidth || 1600) - 420));
                target.style.width = limit + 'px'; target.style.flexBasis = limit + 'px';
                try { win.localStorage.setItem('floe-pane:' + pane, String(limit)); } catch (_) { /* per-tab convenience only */ }
                if (port.resized) { port.resized(); }
            }
            handle.onmousedown = function (e) {
                if (e.button !== 0) { return; }
                e.preventDefault(); drag = {x: e.clientX, w: width()}; handle.setAttribute('data-dragging', 'true');
                function move(ev) { if (!drag) { return; } apply(drag.w + (side === 'left' ? 1 : -1) * (ev.clientX - drag.x)); }
                function up() { drag = null; handle.removeAttribute('data-dragging'); doc.removeEventListener('mousemove', move); doc.removeEventListener('mouseup', up); }
                doc.addEventListener('mousemove', move); doc.addEventListener('mouseup', up);
            };
            handle.ondblclick = function () { apply(side === 'left' ? LEFT_DEFAULT : RIGHT_DEFAULT); };
            handle.onkeydown = function (e) {
                if (e.isComposing || e.keyCode === 229) { return; }
                if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') { e.preventDefault(); apply(width() + (e.key === 'ArrowRight' ? 1 : -1) * (side === 'left' ? 16 : -16)); }
            };
            let saved = NaN;
            try { saved = parseInt(win.localStorage.getItem('floe-pane:' + pane), 10); } catch (_) { /* unavailable storage */ }
            if (Number.isFinite(saved) && saved >= min) { target.style.width = saved + 'px'; target.style.flexBasis = saved + 'px'; }
            return {widen: function (w) { if (width() < w) { apply(w); } }};
        }
        // Tool dialogs reuse the modal backdrop pattern of About/Browse: one
        // open at a time, Escape or Close returns focus to the canvas.
        function dialog(id, closeId) {
            const node = el(id);
            if (!node || !el(closeId)) { return null; }
            function hide(focusCanvas) {
                if (node.hidden) { return; }
                node.hidden = true; if (active === id) { active = null; }
                if (focusCanvas && port.focus) { port.focus(); }
                if (port.changed) { port.changed(); }
            }
            el(closeId).onclick = function () { hide(true); };
            node.onkeydown = function (e) { if (e.key === 'Escape' && !e.isComposing && e.keyCode !== 229) { e.preventDefault(); hide(true); } };
            node.onmousedown = function (e) { if (e.target === node) { hide(true); } };
            dialogs[id] = {show: function () {
                Object.keys(dialogs).forEach(function (k) { if (k !== id) { dialogs[k].hide(false); } });
                if (port.blocked && port.blocked()) { return; }
                node.hidden = false; active = id;
                const focusable = node.querySelectorAll('button');
                const first = Array.prototype.find.call(focusable, function (b) { return !b.hidden && !b.disabled && b.id !== closeId; }) || el(closeId);
                if (first) { first.focus(); }
                if (port.changed) { port.changed(); }
            }, hide: hide, node: node};
            return dialogs[id];
        }
        notebook('left', [
            {tab: 'tab-cells', page: 'cells-page'}, {tab: 'tab-drc', page: 'drc-page'}, {tab: 'tab-inspect', page: 'inspect-page'}
        ], 'cells-page');
        notebook('right', [{tab: 'tab-minimap', page: 'minimap-page'}, {tab: 'tab-palette', page: 'palette-page'}], 'minimap-page');
        const leftSplit = splitter('left-splitter', 'left-pane', 'left', LEFT_MIN);
        splitter('right-splitter', 'right-pane', 'right', RIGHT_MIN);
        ['source', 'display', 'clip', 'settings', 'index'].forEach(function (n) { dialog(n + '-dialog', n + '-close'); });
        let drcWasShown = false, launchWasShown = false, indexWasShown = false;
        function changed() {
            // Loading a DRC database raises the DRC page and widens the pane
            // once, as the GTK viewer does; hiding the panel restores the note.
            if (el('drc-panel') && el('drc-empty')) {
                const drcShown = !el('drc-panel').hidden;
                el('drc-empty').hidden = drcShown;
                if (drcShown && !drcWasShown) { raise('left', 'drc-page'); leftSplit.widen(420); }
                drcWasShown = drcShown;
            }
            // A launcher proposal or a pending index approval lives in the
            // source dialog; surface it when it appears so it is not missed.
            if (el('launch-panel') && el('index-open') && dialogs['source-dialog'] && dialogs['index-dialog']) {
                const launchShown = !el('launch-panel').hidden, indexShown = !el('index-open').hidden;
                if (launchShown && !launchWasShown && !active) { dialogs['source-dialog'].show(); }
                else if (indexShown && !indexWasShown && !active) { dialogs['index-dialog'].show(); }
                launchWasShown = launchShown; indexWasShown = indexShown;
            }
        }
        return Object.freeze({
            changed: changed,
            raise: raise,
            show: function (name, prepare) { const d = dialogs[name + '-dialog']; if (d) { if (prepare) { prepare(); } d.show(); } },
            hide: function (name) { const d = dialogs[name + '-dialog']; if (d) { d.hide(true); } },
            active: function () { return active; },
            page: function (name) { return notebooks[name] ? notebooks[name].current : null; },
            widenLeft: leftSplit.widen
        });
    }
    const api = {bind: bind};
    if (typeof module !== 'undefined' && module.exports) { module.exports = api; } else { root.FloePanes = api; }
}(typeof window === 'undefined' ? this : window));
