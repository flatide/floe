/* Palette UI state only. Rust owns group order, visibility and styles. */
(function (root) {
    'use strict';
    const LIMIT = 4096, PAGE = 64;
    function key(pair) {
        if (!Array.isArray(pair) || pair.length !== 2 || !pair.every(function (n) { return Number.isInteger(n) && n >= 0 && n <= 4294967295; })) { throw Error('Invalid layer pair'); }
        return pair.join('/');
    }
    function pair(k) { return k.split('/').map(Number); }
    function order(a, b) { const x = pair(a), y = pair(b); return x[0] - y[0] || x[1] - y[1]; }
    function keys(values) {
        if (!Array.isArray(values) || values.length > LIMIT) { throw Error('Select at most 4096 layer rows. The previous selection was preserved.'); }
        const result = values.map(key);
        if (new Set(result).size !== result.length) { throw Error('Invalid duplicate layer rows'); }
        return result;
    }
    function checkRow(r) {
        key(r.pair); if (r.parent !== null) { key(r.parent); }
        if (typeof r.name !== 'string' || !Array.isArray(r.aliases) || r.aliases.length > 4 || !r.aliases.every(function (s) { return typeof s === 'string'; }) ||
            typeof r.head !== 'boolean' || typeof r.visible !== 'boolean' || !/^#[0-9a-f]{6}$/i.test(r.color) ||
            !r.fill || !['clear','solid','speckle','pattern'].includes(r.fill.kind) || !Number.isInteger(r.width) || r.width < 1 || r.width > 8) { throw Error('Invalid layer style'); }
        if (r.fill.kind === 'pattern' && (!Array.isArray(r.fill.rows) || r.fill.rows.length !== 16 || !r.fill.rows.every(function (n) { return Number.isInteger(n) && n >= 0 && n <= 65535; }))) { throw Error('Invalid layer pattern'); }
    }
    // The caller supplies a server-ordered inclusive range, or null when the
    // anchor is hidden/missing. This mirrors GTK's list-selection rules.
    function choose(before, anchor, row, event, range) {
        if (event.button === 2) { return {selected:before.slice().sort(order),anchor:anchor}; }
        let selected = new Set(before), next = anchor;
        if (event.shiftKey && range !== null) {
            selected = new Set(event.ctrlKey || event.metaKey ? before.concat(range) : range);
        } else if (event.ctrlKey || event.metaKey) {
            if (selected.has(row)) { selected.delete(row); } else { selected.add(row); }
            next = row;
        } else if (!(event.detail >= 2) && selected.size === 1 && selected.has(row)) {
            selected.clear();
        } else { selected = new Set([row]); next = row; }
        if (selected.size > LIMIT) { throw Error('Select at most 4096 layer rows. The previous selection was preserved.'); }
        return {selected:Array.from(selected).sort(order), anchor:next};
    }
    function bind(port) {
        const el = port.el, list = el('layers'), menu = el('layer-menu');
        let identity = '', loadedKey = '', start = 0, page = null, widgets = [];
        let allClosed = false, exceptions = new Set(), foldRevision = 0;
        let selected = new Map(), anchor = null, flight = null, writing = null, failed = '';
        let stopped = false, suspended = false, menuTarget = null, inputRevision = 0;
        let styleScope = null;
        function context() { return stopped || suspended ? null : port.context(); }
        function available() { const s = context(); return !!s && s.id === identity && s.connected; }
        function closed(k) { return allClosed !== exceptions.has(k); }
        function fold() { return {closed:allClosed, exceptions:Array.from(exceptions).sort(order).map(pair)}; }
        function cancelRead() { if (flight) { const f = flight; flight = null; ++inputRevision; f.token.cancelled = true; if (f.token.abort) { f.token.abort(); } } }
        function hideMenu(focus) {
            const target = menuTarget; menuTarget = null; menu.hidden = true;
            if (focus && target && list.contains(target) && !target.disabled) { target.focus(); }
        }
        function usable() { const s = context(); return available() && page && page.value.start === start && s.key === loadedKey && page.fold === foldRevision && (!flight || flight.kind === 'range') && !writing; }
        function editable() { const s = context(); return usable() && !flight && s.editable; }
        function notify(text) { el('layers-note').textContent = text || ''; }
        function update() {
            const use = usable(), edit = editable(), pending = !!flight || !!writing;
            if (styleScope && !styleCurrent()) { closeStyle(); }
            el('palette-style-apply').disabled = !edit || !styleScope;
            list.setAttribute('aria-busy', String(pending));
            widgets.forEach(function (w) {
                const on = selected.has(w.key);
                w.row.dataset.selected = String(on); w.name.setAttribute('aria-pressed', String(on));
                w.name.disabled = !use; if (w.fold) { w.fold.disabled = !use; }
                w.check.disabled = !edit; w.color.disabled = !edit; w.style.disabled = !edit;
            });
            el('layers-selected').textContent = selected.size + ' selected';
            ['show','hide','toggle','style'].forEach(function (action) {
                el('layers-' + action).disabled = !edit || !selected.size;
                el('layer-menu-' + action).disabled = !edit || !selected.size;
            });
            ['all','none'].forEach(function (action) {
                const s = context(), disabled = !available() || !s.editable || !!writing;
                el('layers-' + action).disabled = el('layer-menu-' + action).disabled = disabled;
            });
            el('layers-clear').disabled = !available() || !selected.size || !!writing;
            ['collapse','expand'].forEach(function (v) { el('layers-' + v).disabled = !available() || !!writing; });
            el('layers-prev').disabled = !use || !!flight || start === 0;
            el('layers-next').disabled = !use || !!flight || page.value.next === null;
            el('layers-retry').hidden = !failed;
            el('layers-retry').disabled = !available() || pending;
            presets.changed();
        }
        function valid(p) { const s = context(); return available() && p === page && p.value.start === start && s.key === loadedKey && p.fold === foldRevision; }
        function applySelection(result, groups, row) {
            const next = new Map();
            result.selected.forEach(function (k) { next.set(k, groups.has(k) || selected.get(k) === true); });
            selected = next;
            if (result.anchor !== (anchor && anchor.key)) { anchor = row ? {key:result.anchor, parent:row.parent && key(row.parent)} : null; }
            notify(''); update();
        }
        async function select(r, event, p) {
            if (!usable() || !valid(p) || (event.button !== undefined && event.button !== 0)) { return false; }
            cancelRead(); hideMenu(false); const revision = ++inputRevision;
            const before = Array.from(selected.keys()), oldAnchor = anchor && anchor.key, k = key(r.pair);
            const groups = new Set(p.value.rows.filter(function (row) { return row.children > 0; }).map(function (row) { return key(row.pair); }));
            let range = null;
            try {
                if (event.shiftKey && anchor && !(anchor.parent && closed(anchor.parent))) {
                    const order = p.value.rows.map(function (row) { return key(row.pair); }), a = order.indexOf(oldAnchor), b = order.indexOf(k);
                    if (a >= 0) { range = order.slice(Math.min(a,b), Math.max(a,b) + 1); }
                    else {
                        const f = {kind:'range', token:{}, key:loadedKey, revision:inputRevision}; flight = f; update();
                        let result;
                        try { result = await port.http('POST', '/api/v1/views/' + identity + '/palette', {kind:'range', first:pair(oldAnchor), last:r.pair, fold:fold()}, false, f.token); }
                        finally { if (flight === f) { flight = null; } }
                        if (f.token.cancelled || !valid(p) || inputRevision !== f.revision) { return false; }
                        if (result.render_key !== f.key) { throw Error('Layer state changed. Select the range again.'); }
                        range = keys(result.pairs);
                        const heads = keys(result.groups);
                        const ends = [oldAnchor,k].sort(orderCompare);
                        if (range[0] !== ends[0] || range[range.length-1] !== ends[1] || heads.some(function (h) { return !range.includes(h); }) || range.some(function (v,i) { return i && orderCompare(range[i-1], v) >= 0; })) { throw Error('Invalid layer range'); }
                        heads.forEach(function (h) { groups.add(h); });
                    }
                }
                applySelection(choose(before, oldAnchor, k, event, range), groups, r);
                return true;
            } catch (e) { if (revision === inputRevision && valid(p)) { notify(e.message); } return false; }
            finally { update(); }
        }
        function orderCompare(a,b) { return order(a,b); }
        function change(action, entries) {
            if (!editable() || !['show','hide','toggle'].includes(action)) { return; }
            const chosen = entries || selected;
            if (!chosen.size) { return; }
            const ids = Array.from(chosen.keys()).sort(order);
            const body = {layer_batch:{action:action, pairs:ids.map(pair), collapsed:ids.filter(function (k) { return chosen.get(k) && closed(k); }).map(pair)}};
            write(body);
        }
        function styleCurrent() {
            const s = context();
            return available() && !!s && styleScope && s.id === styleScope.id && s.key === styleScope.key &&
                styleScope.revision === inputRevision && styleScope.fold === foldRevision;
        }
        function closeStyle() { styleScope = null; el('palette-style').hidden = true; }
        function openStyle() {
            if (!editable() || !selected.size) { return; }
            if (port.closeRowStyle) { port.closeRowStyle(); }
            hideMenu(false);
            styleScope = {id:identity,key:loadedKey,revision:inputRevision,fold:foldRevision,selected:new Map(selected)};
            el('palette-style-title').textContent = 'Style ' + selected.size + ' selected rows';
            el('palette-color-on').checked = false; el('palette-color').value = '#00ffff';
            el('palette-fill').value = ''; el('palette-width').value = '';
            el('palette-pattern').value = new Array(16).fill('aaaa').join(' ');
            patternFields(); el('palette-style').hidden = false; update(); el('palette-fill').focus();
        }
        function patternFields() { el('palette-pattern').hidden = el('palette-pattern-label').hidden = el('palette-fill').value !== 'pattern'; }
        function applyStyle(event) {
            event.preventDefault();
            if (!editable() || !styleCurrent()) { closeStyle(); notify('Selection or view changed. Open Style selected again.'); return; }
            try {
                const ids = Array.from(styleScope.selected.keys()).sort(order);
                const batch = {pairs:ids.map(pair),collapsed:ids.filter(function(k){return styleScope.selected.get(k) && closed(k);}).map(pair)};
                if (el('palette-color-on').checked) {
                    const color = el('palette-color').value;
                    if (!/^#[0-9a-f]{6}$/i.test(color)) { throw Error('Choose a six-digit RGB color.'); }
                    batch.color = color;
                }
                const kind = el('palette-fill').value, width = el('palette-width').value;
                if (kind) {
                    if (!['clear','solid','speckle','pattern'].includes(kind)) { throw Error('Invalid fill.'); }
                    batch.fill = {kind:kind};
                    if (kind === 'pattern') {
                        const rows = el('palette-pattern').value.trim().split(/\s+/);
                        if (rows.length !== 16 || !rows.every(function(s){return /^[0-9a-f]{4}$/i.test(s);})) { throw Error('A pattern needs exactly 16 four-digit hex rows.'); }
                        batch.fill.rows = rows.map(function(s){return parseInt(s,16);});
                    }
                }
                if (width === '+1' || width === '-1') { batch.width_step = Number(width); }
                else if (width) { if (!/^[1-8]$/.test(width)) { throw Error('Line width must be 1–8 pixels.'); } batch.width = Number(width); }
                if (!batch.color && !batch.fill && batch.width === undefined && batch.width_step === undefined) { throw Error('Choose at least one style field.'); }
                closeStyle(); write({style_batch:batch});
            } catch(e) { notify(e.message); }
        }
        function write(body) {
            if (!available() || !context().editable || writing) { return; }
            const mark = {}; writing = mark; hideMenu(false); notify(''); update();
            try {
                port.edit(body, function (error) { if (writing === mark) { writing = null; if (error) { notify(String(error)); } changed(); } });
            } catch (e) { writing = null; notify(e.message); update(); }
        }
        function showMenu(event, target) {
            event.preventDefault();
            if (!usable()) { return; }
            menuTarget = target; menu.hidden = false; update();
            const bounds = target.getBoundingClientRect();
            const x = Number.isFinite(event.clientX) ? event.clientX : bounds.left;
            const y = Number.isFinite(event.clientY) ? event.clientY : bounds.bottom;
            menu.style.left = Math.max(4, Math.min(x, (port.window.innerWidth || 1000) - 190)) + 'px';
            menu.style.top = Math.max(4, Math.min(y, (port.window.innerHeight || 800) - 244)) + 'px';
            const first = ['show','hide','toggle','all','none'].map(function (v) { return el('layer-menu-' + v); }).find(function (b) { return !b.disabled; });
            (first || el('layer-menu-close')).focus();
        }
        function paint(value) {
            const focused = widgets.find(function (w) { return w.name === port.document.activeElement; });
            page = {value:value, fold:foldRevision}; const p = page;
            list.textContent = ''; widgets = [];
            value.rows.forEach(function (r) {
                const k = key(r.pair), row = port.document.createElement('div');
                row.className = 'layer-row' + (r.parent ? ' child' : '') + (r.head ? ' head' : '');
                const check = port.document.createElement('input'); check.type = 'checkbox'; check.checked = r.visible;
                check.setAttribute('aria-label', 'Show ' + r.name);
                check.onchange = function () { const on = check.checked; check.checked = r.visible; if (valid(p)) { change(on ? 'show' : 'hide', new Map([[k,r.children > 0]])); } };
                const extras = port.styles(r, {id:identity,key:loadedKey}, function () { return valid(p) && editable(); });
                const expander = port.document.createElement(r.children ? 'button' : 'span'); expander.className = 'layer-fold';
                if (r.children) { expander.type = 'button'; expander.textContent = r.closed ? '+' : '−'; expander.setAttribute('aria-expanded', String(!r.closed)); expander.setAttribute('aria-label', (r.closed ? 'Expand ' : 'Collapse ') + r.name);
                    expander.onclick = function () { if (usable() && valid(p)) { toggleFold(k); } }; }
                const name = port.document.createElement('button'); name.type = 'button'; name.className = 'layer-select';
                const title = port.document.createElement('span'); title.className = 'layer-name'; title.textContent = r.name || k; title.dataset.pair = k;
                name.title = k + ' · ' + r.name + (r.aliases.length ? ' · ' + r.aliases.join(', ') : '');
                name.setAttribute('aria-label', 'Select ' + k + ' ' + r.name); name.setAttribute('aria-haspopup','menu'); name.appendChild(title);
                let lastClick = null, lastRevision = 0;
                name.onclick = function (e) { lastClick = select(r,e,p); lastRevision = inputRevision; };
                name.ondblclick = function (e) { e.preventDefault(); const revision = lastRevision; Promise.resolve(lastClick).then(function (ok) { if (ok && revision === inputRevision && valid(p)) { change('toggle', new Map([[k,r.children > 0]])); } }); };
                name.onkeydown = function (e) { if (!e.isComposing && (e.key === 'ContextMenu' || e.key === 'F10' && e.shiftKey)) { showMenu(e,name); } };
                row.oncontextmenu = function (e) { showMenu(e,name); };
                row.onclick = function (e) { if (e.target === row) { select(r,e,p); } };
                row.appendChild(check); row.appendChild(extras.color); row.appendChild(expander); row.appendChild(name); row.appendChild(extras.style); list.appendChild(row);
                widgets.push({row:row, key:k, name:name, check:check, color:extras.color, style:extras.style, fold:r.children ? expander : null});
            });
            if (!value.rows.length) { list.textContent = 'No layer rows.'; }
            el('layers-count').textContent = (value.total ? (start + 1) + '–' + (start + value.rows.length) + ' / ' : '') + value.total;
            port.painted(); update();
            const target = focused && widgets.find(function (w) { return w.key === focused.key; }); if (target && !target.name.disabled) { target.name.focus(); }
        }
        async function load() {
            const s = context(); if (!available()) { update(); return; }
            cancelRead(); hideMenu(false);
            const f = {kind:'page', token:{}, id:identity, key:s.key, start:start, fold:foldRevision}; flight = f; failed = ''; update();
            try {
                const value = await port.http('POST', '/api/v1/views/' + f.id + '/palette', {kind:'page', start:start, fold:fold()}, false, f.token);
                const now = context();
                if (f.token.cancelled || flight !== f || !now || now.id !== f.id || now.key !== f.key || f.fold !== foldRevision || f.start !== start) { return; }
                if (value.render_key !== f.key) { failed = f.key; throw Error('Layer state changed. Reload the layer page.'); }
                if (!Number.isSafeInteger(value.total) || value.total < 0 || value.start !== start || !Array.isArray(value.rows) || value.rows.length > PAGE || value.rows.length !== Math.min(PAGE,value.total-start) || value.next !== (start + value.rows.length < value.total ? start + value.rows.length : null)) { throw Error('Invalid layer page'); }
                const rowKeys = keys(value.rows.map(function (r) { return r.pair; }));
                if (rowKeys.some(function (k,i) { return i && order(rowKeys[i-1],k) >= 0; })) { throw Error('Invalid layer order'); }
                value.rows.forEach(function (r) { checkRow(r); if (!Number.isSafeInteger(r.children) || r.children < 0 || typeof r.closed !== 'boolean' || r.closed !== (r.children > 0 && closed(key(r.pair)))) { throw Error('Invalid layer group'); } });
                loadedKey = f.key; flight = null; notify(''); paint(value);
            } catch (e) {
                if (!f.token.cancelled && identity === f.id && context() && context().key === f.key) { failed = f.key; notify(e.message); }
            } finally { if (flight === f) { flight = null; } update(); }
        }
        function toggleFold(k) {
            const next = new Set(exceptions); if (next.has(k)) { next.delete(k); } else { next.add(k); }
            if (next.size > LIMIT) { notify('Too many individual group settings. Use Expand all or Collapse all first.'); return; }
            exceptions = next; folding();
        }
        function folding() { ++foldRevision; ++inputRevision; start = 0; failed = ''; cancelRead(); load(); }
        function changed() {
            const s = port.context();
            if (!s || s.id !== identity) {
                cancelRead(); hideMenu(false); ++inputRevision; identity = s ? s.id : ''; loadedKey = ''; start = 0; page = null;
                selected.clear(); anchor = null; allClosed = false; exceptions.clear(); ++foldRevision; writing = null; failed = ''; widgets = []; list.textContent = ''; el('layers-count').textContent = '0 layers'; notify('');
            }
            if (!available()) { cancelRead(); hideMenu(false); update(); return; }
            if (flight && flight.key !== s.key) { cancelRead(); }
            if (page && loadedKey !== s.key) { hideMenu(false); }
            if (!flight && failed !== s.key && (!page || loadedKey !== s.key || page.fold !== foldRevision || page.value.start !== start)) { load(); }
            update();
        }
        ['show','hide','toggle'].forEach(function (v) { el('layers-' + v).onclick = el('layer-menu-' + v).onclick = function () { change(v); }; });
        el('layers-style').onclick = el('layer-menu-style').onclick = openStyle;
        el('palette-fill').onchange = patternFields;
        el('palette-style').onsubmit = applyStyle;
        el('palette-style-cancel').onclick = closeStyle;
        ['all','none'].forEach(function (v) { el('layers-' + v).onclick = el('layer-menu-' + v).onclick = function () { write({layers:{mode:v}}); }; });
        el('layers-clear').onclick = function () { if (available() && !writing) { if (flight && flight.kind === 'range') { cancelRead(); } ++inputRevision; selected.clear(); notify(''); update(); } };
        el('layers-collapse').onclick = function () { if (available() && !writing) { allClosed = true; exceptions.clear(); folding(); } };
        el('layers-expand').onclick = function () { if (available() && !writing) { allClosed = false; exceptions.clear(); folding(); } };
        el('layers-prev').onclick = function () { if (usable() && !flight && start) { start = Math.max(0,start - PAGE); ++inputRevision; load(); } };
        el('layers-next').onclick = function () { if (usable() && !flight && page.value.next !== null) { start = page.value.next; ++inputRevision; load(); } };
        el('layers-retry').onclick = function () { if (available() && !writing) { failed = ''; load(); } };
        el('layer-menu-close').onclick = function () { hideMenu(true); };
        list.oncontextmenu = function (e) { if (e.target === list) { showMenu(e,widgets.length ? widgets[0].name : list); } };
        menu.onkeydown = function (e) {
            if (e.isComposing) { return; }
            if (e.key === 'Escape') { e.preventDefault(); hideMenu(true); return; }
            if (!['ArrowDown','ArrowUp','Home','End'].includes(e.key)) { return; }
            const buttons = ['show','hide','toggle','style','all','none','close'].map(function (k) { return el('layer-menu-' + k); }).filter(function (b) { return !b.disabled; });
            const i = buttons.indexOf(port.document.activeElement); e.preventDefault();
            buttons[e.key === 'Home' ? 0 : e.key === 'End' ? buttons.length - 1 : (i + (e.key === 'ArrowUp' ? -1 : 1) + buttons.length) % buttons.length].focus();
        };
        port.document.addEventListener('mousedown', function (e) { if (!menu.hidden && !menu.contains(e.target)) { hideMenu(false); } });
        port.document.addEventListener('focusin', function (e) { if (!menu.hidden && !menu.contains(e.target)) { hideMenu(false); } });
        function suspend() { suspended = true; cancelRead(); hideMenu(false); update(); }
        function resume() { suspended = false; changed(); }
        function stop() { stopped = true; suspend(); }
        function applyPreset(fields) {
            if (!editable() || !selected.size) { return; }
            const ids=Array.from(selected.keys()).sort(order);
            const batch={pairs:ids.map(pair),collapsed:ids.filter(function(k){return selected.get(k)&&closed(k);}).map(pair)};
            if (fields.color) { batch.color=fields.color; }
            if (fields.fill) { batch.fill=fields.fill; }
            if (fields.fill_slot) { batch.fill_slot=fields.fill_slot; }
            closeStyle(); if (port.closeRowStyle) { port.closeRowStyle(); } write({style_batch:batch});
        }
        const presets=port.presets.bind({el:el,document:port.document,window:port.window,http:port.http,available:available,
            slotEditor:port.slotEditor,editSlot:port.editSlot,
            context:function(){const s=context();return s?{id:s.id,epoch:s.epoch,rev:s.rev,slotKey:s.slotKey,
                ready:available(),idle:s.editable&&!writing,fillEdit:s.fillEdit}:null;},
            enabled:function(){return editable()&&selected.size>0;},apply:applyPreset});
        menu.hidden = true; closeStyle(); changed();
        return Object.freeze({changed:changed, stop:function(){stop();presets.stop();}, suspend:suspend, resume:resume, closeStyle:closeStyle});
    }
    const api = {bind:bind, choose:choose};
    if (typeof module !== 'undefined' && module.exports) { module.exports = api; } else { root.FloePalette = api; }
}(typeof window === 'undefined' ? this : window));
