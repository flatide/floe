/* GTK-style menu bar over the existing controls. The bar owns no view state:
 * each time a menu opens, its check/radio items re-read the control they
 * proxy, and activating an item clicks or sets that same control. Hidden or
 * disabled proxies hide or disable their items, so the menu can never grant
 * an action the panels themselves refuse. */
(function (root) {
    'use strict';
    function bind(port) {
        const el = port.el, doc = port.document, bar = el('menubar');
        const menus = [];
        let open = null;
        function fire(control, type) {
            if (typeof control.dispatchEvent === 'function' && port.window && typeof port.window.Event === 'function') {
                control.dispatchEvent(new port.window.Event(type, {bubbles: true}));
            } else if (typeof control['on' + type] === 'function') { control['on' + type](); }
        }
        function control(item) {
            const id = item.proxy || (item.set && item.set.id) || item.toggle || (item.check && item.check.id) || (item.radio && item.radio.id);
            return id ? el(id) : null;
        }
        function hiddenItem(item) {
            if (typeof item.hidden === 'function') { return !!item.hidden(); }
            const c = item.proxy ? el(item.proxy) : null;
            return !!(c && c.hidden);
        }
        function disabledItem(item) {
            if (typeof item.enabled === 'function' && !item.enabled()) { return true; }
            const c = control(item);
            return !!(c && c.disabled);
        }
        function checkedItem(item) {
            if (typeof item.check === 'function') { return !!item.check(); }
            if (item.toggle) { return !!el(item.toggle).checked; }
            if (item.check) { const c = el(item.check.id); return item.check.value !== undefined ? c.value === item.check.value : !!c.checked; }
            if (item.radio) { return el(item.radio.id).value === item.radio.value; }
            return null;
        }
        function activate(item) {
            if (item.sub) { return; }
            close(true);
            try {
                if (typeof item.action === 'function') { item.action(); return; }
                if (item.toggle) { const c = el(item.toggle); c.checked = !c.checked; fire(c, 'change'); return; }
                if (item.set || item.radio) {
                    const s = item.set || item.radio, c = el(s.id);
                    if (c.value !== s.value) { c.value = s.value; fire(c, 'change'); }
                    return;
                }
                if (item.proxy) { const c = el(item.proxy); if (typeof c.click === 'function') { c.click(); } else if (c.onclick) { c.onclick(); } }
            } catch (e) { if (port.report) { port.report(e); } }
        }
        function refresh(menu) {
            menu.entries.forEach(function (entry) {
                const item = entry.item, node = entry.node;
                if (item.sep) { return; }
                node.hidden = hiddenItem(item);
                if (node.hidden) { return; }
                node.disabled = disabledItem(item);
                const checked = checkedItem(item);
                if (checked !== null) {
                    node.setAttribute('aria-checked', String(checked));
                    entry.mark.textContent = checked ? (item.radio ? '●' : '✓') : '';
                }
                if (entry.sub) { refresh(entry.sub); }
            });
        }
        function close(focusBar) {
            if (!open) { return; }
            const was = open; open = null;
            was.panel.hidden = true; was.title.setAttribute('aria-expanded', 'false');
            menus.forEach(function (m) { m.entries.forEach(function (e) { if (e.sub) { e.sub.panel.hidden = true; } }); });
            if (focusBar && port.focus) { port.focus(); }
        }
        function show(menu) {
            if (open && open !== menu) { close(false); }
            refresh(menu);
            open = menu; menu.panel.hidden = false; menu.title.setAttribute('aria-expanded', 'true');
            const first = items(menu)[0];
            if (first) { first.node.focus(); }
        }
        function toggle(menu) { if (open === menu) { close(true); } else { show(menu); } }
        // A submenu is a menu of its own (its entries live in entry.sub);
        // parent/owner link it back for Escape, ArrowLeft and refresh.
        function items(menu) {
            return menu.entries.filter(function (e) { return !e.item.sep && !e.node.hidden && !e.node.disabled; });
        }
        function rootOf(menu) { while (menu.parent) { menu = menu.parent; } return menu; }
        function move(menu, entry, delta) {
            const visible = items(menu);
            const i = visible.findIndex(function (e) { return e.node === entry.node; });
            const next = visible[(i + delta + visible.length) % visible.length];
            if (next) { next.node.focus(); }
        }
        function enterSub(entry) {
            openSub(entry);
            const first = items(entry.sub)[0];
            if (first) { first.node.focus(); }
        }
        function leaveSub(menu) {
            closeSubs(menu.parent); menu.owner.node.focus();
        }
        function switchTop(menu, delta) {
            const i = menus.indexOf(rootOf(menu));
            show(menus[(i + delta + menus.length) % menus.length]);
        }
        function buildPanel(menu, list, parentPanel) {
            const panel = doc.createElement('div'); panel.className = parentPanel ? 'menu-panel menu-sub' : 'menu-panel';
            panel.setAttribute('role', 'menu'); panel.hidden = true;
            list.forEach(function (item) {
                if (item.sep) {
                    const sep = doc.createElement('div'); sep.className = 'menu-sep'; sep.setAttribute('role', 'separator');
                    panel.appendChild(sep); menu.entries.push({item: item, node: sep, panel: panel});
                    return;
                }
                const node = doc.createElement('button'); node.type = 'button'; node.className = 'menu-item';
                node.setAttribute('role', item.sub ? 'menuitem' : (item.radio ? 'menuitemradio' : (item.toggle || item.check ? 'menuitemcheckbox' : 'menuitem')));
                const mark = doc.createElement('span'); mark.className = 'menu-mark'; mark.setAttribute('aria-hidden', 'true');
                const label = doc.createElement('span'); label.className = 'menu-label'; label.textContent = item.label;
                const key = doc.createElement('span'); key.className = 'menu-key'; key.textContent = item.sub ? '▸' : (item.key || '');
                node.appendChild(mark); node.appendChild(label); node.appendChild(key);
                if (item.title) { node.title = item.title; }
                if (item.id) { node.id = item.id; }
                const entry = {item: item, node: node, mark: mark, panel: panel, sub: null, menu: menu};
                if (item.sub) {
                    entry.sub = {entries: [], panel: null, title: node, parent: menu, owner: entry};
                    entry.sub.panel = buildPanel(entry.sub, item.sub, panel);
                    node.setAttribute('aria-haspopup', 'menu'); node.setAttribute('aria-expanded', 'false');
                    const wrap = doc.createElement('div'); wrap.className = 'menu-subwrap';
                    wrap.appendChild(node); wrap.appendChild(entry.sub.panel); panel.appendChild(wrap);
                    node.onclick = function (e) { if (e && e.preventDefault) { e.preventDefault(); } openSub(entry); };
                    node.onmouseenter = function () { openSub(entry); };
                } else {
                    panel.appendChild(node);
                    node.onclick = function (e) { if (e && e.preventDefault) { e.preventDefault(); } activate(item); };
                    node.onmouseenter = function () { closeSubs(menu); };
                }
                node.onkeydown = function (e) { keyInPanel(menu, entry, e); };
                menu.entries.push(entry);
            });
            return panel;
        }
        function closeSubs(menu) {
            menu.entries.forEach(function (e) { if (e.sub) { e.sub.panel.hidden = true; e.node.setAttribute('aria-expanded', 'false'); } });
        }
        function openSub(entry) {
            if (!open) { return; }
            closeSubs(rootOf(entry.menu));
            refresh(entry.sub);
            entry.sub.panel.hidden = false; entry.node.setAttribute('aria-expanded', 'true');
        }
        function keyInPanel(menu, entry, e) {
            if (!open || e.isComposing || e.keyCode === 229) { return; }
            const k = e.key, inSub = !!menu.parent;
            if (k === 'Escape') { e.preventDefault(); if (inSub) { leaveSub(menu); } else { close(true); } return; }
            if (k === 'ArrowDown' || k === 'ArrowUp') { e.preventDefault(); move(menu, entry, k === 'ArrowDown' ? 1 : -1); return; }
            if (k === 'ArrowRight') { e.preventDefault(); if (entry.sub) { enterSub(entry); } else { switchTop(menu, 1); } return; }
            if (k === 'ArrowLeft') { e.preventDefault(); if (inSub) { leaveSub(menu); } else { switchTop(menu, -1); } return; }
            if (k === 'Home' || k === 'End') { e.preventDefault(); const v = items(menu); const t = v[k === 'Home' ? 0 : v.length - 1]; if (t) { t.node.focus(); } return; }
            if (k === 'Enter' || k === ' ') { e.preventDefault(); if (entry.sub) { enterSub(entry); } else { activate(entry.item); } }
        }
        function build(model) {
            bar.textContent = '';
            model.forEach(function (m) {
                const menu = {name: m.name, entries: [], title: null, panel: null};
                const wrap = doc.createElement('div'); wrap.className = 'menu';
                const title = doc.createElement('button'); title.type = 'button'; title.className = 'menu-title'; title.textContent = m.name;
                title.setAttribute('aria-haspopup', 'menu'); title.setAttribute('aria-expanded', 'false');
                if (m.id) { title.id = m.id; }
                menu.title = title; menu.panel = buildPanel(menu, m.items, null);
                title.onclick = function (e) { if (e && e.preventDefault) { e.preventDefault(); } toggle(menu); };
                title.onmouseenter = function () { if (open && open !== menu) { show(menu); } };
                title.onkeydown = function (e) {
                    if (e.isComposing || e.keyCode === 229) { return; }
                    if (e.key === 'ArrowDown' || e.key === 'Enter' || e.key === ' ') { e.preventDefault(); show(menu); }
                    else if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
                        e.preventDefault(); const i = menus.indexOf(menu), n = menus[(i + (e.key === 'ArrowRight' ? 1 : -1) + menus.length) % menus.length];
                        if (open) { show(n); } else { n.title.focus(); }
                    } else if (e.key === 'Escape') { close(true); }
                };
                wrap.appendChild(title); wrap.appendChild(menu.panel); bar.appendChild(wrap);
                menus.push(menu);
            });
            if (m0Hidden(model)) { bar.hidden = true; }
        }
        function m0Hidden(model) { return !model.length; }
        doc.addEventListener('mousedown', function (e) { if (open && !bar.contains(e.target)) { close(false); } });
        doc.addEventListener('focusin', function (e) { if (open && !bar.contains(e.target)) { close(false); } });
        doc.addEventListener('keydown', function (e) { if (open && e.key === 'Escape' && !e.isComposing && e.keyCode !== 229 && !bar.contains(e.target)) { close(true); } });
        build(port.model || []);
        return Object.freeze({
            refresh: function () { if (open) { refresh(open); } },
            close: close,
            open: function (name) { const m = menus.find(function (x) { return x.name === name; }); if (m) { show(m); } },
            isOpen: function () { return !!open; }
        });
    }
    const api = {bind: bind};
    if (typeof module !== 'undefined' && module.exports) { module.exports = api; } else { root.FloeMenubar = api; }
}(typeof window === 'undefined' ? this : window));
