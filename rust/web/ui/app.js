/* Local, ES2017 CAD client. The server owns world coordinates and view state. */
(function () {
    'use strict';
    const P = window.FloeProtocol;
    const bundle = document.querySelector('meta[name="floe-bundle"]').content;
    const el = function (id) { return document.getElementById(id); };
    const canvas = el('canvas'), viewport = el('viewport');
    const context = canvas.getContext('2d', {alpha: false});
    const sessionKey = 'floe-session:' + location.origin;
    let auth = null, stopped = false, socket = null, epoch = '', state = null;
    let seq = '0', queue = [], inflight = null, accepted = null, lastSend = 0;
    let socketSerial = 0, decode = null, reconnectTimer = null, reconnectDelay = 500;
    let catalog = [], currentId = '', currentSource = '', ownerBusy = false, submitting = false;
    let layerStart = 0, layerNext = null, layerLoad = 0, layerRev = '';
    let levelNext = null, levelSource = '', levelIds = new Set();
    let operationTimer = null, resizeTimer = null, displayed = false;
    const errors = {
        index_unavailable: 'A current index is required. Use “Index this source”; opening never indexes automatically.',
        busy: 'Resource or cache is busy. Close the current view before indexing or opening another source.',
        worker_version: 'The native renderer version does not match. Rebuild the matched binaries.',
        worker_failed: 'The renderer failed. Check the local service diagnostics; close and reopen to retry.',
        stale_state: 'The view changed in another connection. That edit was not replayed.',
        invalid_request: 'The requested value or selection is not supported.',
        incomplete: 'Operation finished with missing or unsupported sources.',
        cancelled: 'Operation cancelled.',
        operation_sequence: 'Another operation changed the session. Refresh its state and submit again.',
        io_error: 'A local file could not be read or written.',
        closed: 'This session is closed. Start a new local session.'
    };
    function notice(text) { el('notice').textContent = text || ''; el('notice').hidden = !text; }
    function message(error) { return errors[error] || String(error || 'Request failed'); }
    function report(error) { notice(message(error.message || error)); }
    function http(method, path, body, missing) {
        return new Promise(function (resolve, reject) {
            const xhr = new XMLHttpRequest();
            xhr.open(method, path); xhr.timeout = 8000;
            if (auth) { xhr.setRequestHeader('X-Floe-CSRF', auth.csrf); }
            if (body !== undefined) { xhr.setRequestHeader('Content-Type', 'application/json'); }
            xhr.onload = function () {
                if (missing && xhr.status === 404) { resolve(null); return; }
                let value = null;
                try {
                    if (xhr.responseText.length > 1024 * 1024) { throw new Error('Reply limit'); }
                    if (xhr.responseText) { value = JSON.parse(xhr.responseText); }
                } catch (e) { reject(e); return; }
                if (xhr.status < 200 || xhr.status >= 300) {
                    if (xhr.status === 401) { stopped = true; connection('Session expired', false); }
                    reject(new Error(message(value && value.error || ('HTTP ' + xhr.status))));
                } else { resolve(value); }
            };
            xhr.onerror = function () { reject(new Error('Local service is unavailable')); };
            xhr.ontimeout = function () { reject(new Error('Request timed out; its outcome may be pending. Check operation status before retrying.')); };
            xhr.send(body === undefined ? null : JSON.stringify(body));
        });
    }
    function connection(text, ready) { el('connection').textContent = text; el('connection').className = 'connection' + (ready ? ' ready' : ''); }
    function dims() {
        const r = viewport.getBoundingClientRect(), dpr = window.devicePixelRatio || 1;
        const left = Math.ceil(r.left * dpr), top = Math.ceil(r.top * dpr);
        const w = Math.floor(r.right * dpr) - left, h = Math.floor(r.bottom * dpr) - top;
        P.pixels(w, h);
        return {pixels: [w, h], dpr: dpr, left: left / dpr - r.left, top: top / dpr - r.top};
    }
    function positionCanvas(size) {
        canvas.style.width = (canvas.width / size.dpr) + 'px';
        canvas.style.height = (canvas.height / size.dpr) + 'px';
        canvas.style.left = (size.left + Math.round((size.pixels[0] - canvas.width) / 2) / size.dpr) + 'px';
        canvas.style.top = (size.top + Math.round((size.pixels[1] - canvas.height) / 2) / size.dpr) + 'px';
    }
    function live() { return state && !['closed', 'failed'].includes(state.status); }
    function controls() {
        const enabled = live() && socket && socket.readyState === WebSocket.OPEN && !!epoch;
        ['fit', 'zoom-in', 'zoom-out', 'goto', 'depth', 'detail', 'thin', 'frames', 'labels', 'mono', 'layers-all', 'layers-none'].forEach(function (id) { el(id).disabled = !enabled; });
        el('labels').disabled = !enabled || !state.capabilities.labels;
        el('open').disabled = submitting || ownerBusy || !!live();
        el('close').disabled = !currentId || submitting || ownerBusy;
        el('index').disabled = submitting || ownerBusy;
    }
    function send(value) {
        if (!socket || socket.readyState !== WebSocket.OPEN) { throw new Error('View is disconnected'); }
        seq = P.next(seq); value.seq = seq;
        const text = JSON.stringify(value);
        if (new TextEncoder().encode(text).length > 8192 || socket.bufferedAmount > 16384) {
            throw new Error('Input limit; wait for the local connection');
        }
        socket.send(text); return seq;
    }
    function pump() {
        if (!live() || !epoch || inflight || !queue.length || !socket || socket.readyState !== WebSocket.OPEN) { return; }
        const wait = 65 - (Date.now() - lastSend);
        if (wait > 0) { window.setTimeout(pump, wait); return; }
        try {
            const body = queue.shift();
            inflight = send({type: 'view.set', connection_epoch: epoch, view_id: currentId,
                base_state_rev: state.state_rev, body: body});
            lastSend = Date.now();
        } catch (e) { inflight = null; queue = []; report(e); }
    }
    function edit(body) {
        if (!live() || !epoch) { notice('Open a connected view first.'); return; }
        if (queue.length >= 64) { notice('Input queue is full. This input was not applied.'); return; }
        queue.push(body); pump();
    }
    function statusSnapshot(s) {
        if (s.view_id !== currentId || s.connection_epoch !== epoch) { return; }
        if (state && state.connection_epoch === epoch && P.compare(s.state_rev, state.state_rev) < 0) { return; }
        state = s; controls();
        el('rendering').hidden = !['opening', 'rendering', 'cancelling'].includes(s.status);
        el('rendering').textContent = s.status === 'opening' ? 'Opening index' : 'Rendering';
        if (!displayed) { el('empty-message').textContent = s.status === 'failed' ? message(s.failure) : 'Preparing the first frame…'; }
        if (s.failure) { notice(message(s.failure)); }
        if (document.activeElement !== el('depth')) { el('depth').value = s.depth; }
        el('max-depth').textContent = s.max_depth === null ? '' : '/ ' + s.max_depth;
        ['detail', 'thin'].forEach(function (id) { el(id).value = s[id]; });
        ['frames', 'labels', 'mono'].forEach(function (id) { el(id).checked = s[id]; });
        const b = s.bbox_dbu.map(Number), dbu = Number(s.dbu_um);
        el('viewport-info').textContent = ((b[2] - b[0]) * dbu).toPrecision(6) + ' × ' + ((b[3] - b[1]) * dbu).toPrecision(6) + ' µm';
        el('status').textContent = s.status + ' · depth ' + s.depth + ' · thin:' + s.effective_thin + (s.source_stale ? ' · SOURCE STALE' : '');
        if (accepted && P.compare(s.state_rev, accepted.rev) >= 0) { accepted = null; inflight = null; pump(); }
        if (s.state_rev !== layerRev) { loadLayers().catch(report); }
    }
    function finishDecode() { if (decode) { decode(); decode = null; } }
    function acknowledge(h, disposition, ws, serial) {
        if (ws !== socket || serial !== socketSerial || ws.readyState !== WebSocket.OPEN) { return; }
        try { send({type: 'frame.ack', connection_epoch: h.connection_epoch, frame_id: h.frame_id, disposition: disposition}); }
        catch (e) { report(e); ws.close(); }
    }
    function frame(buffer, ws, serial) {
        let packet;
        try { packet = P.packet(buffer); } catch (e) { report(e); ws.close(); return; }
        const h = packet.header;
        const valid = function () { return ws === socket && serial === socketSerial && P.matches(h, state) &&
            (!accepted || P.compare(h.render_rev, accepted.render) >= 0) && !document.hidden; };
        if (!valid()) { acknowledge(h, 'discarded', ws, serial); return; }
        if (decode) { notice('Frame credit violation'); ws.close(); return; }
        let done = false, image = null, url = null, timer = null;
        function finish(draw) {
            if (done) { return; } done = true;
            if (timer) { clearTimeout(timer); }
            let disposition = 'discarded';
            try {
                if (draw && valid()) {
                    if (canvas.width !== h.width || canvas.height !== h.height) { canvas.width = h.width; canvas.height = h.height; }
                    positionCanvas(dims()); context.imageSmoothingEnabled = false;
                    draw();
                    if (!displayed && document.activeElement === document.body) { viewport.focus(); }
                    displayed = true; el('empty').hidden = true; disposition = 'displayed';
                    canvas.dataset.frameId = h.frame_id; canvas.dataset.renderRev = h.render_rev;
                    canvas.dataset.bboxDbu = JSON.stringify(h.bbox_dbu);
                    el('status').textContent = (h.complete ? 'Live' : 'INCOMPLETE') + (h.approximate ? ' · summary/LOD' : '') +
                        (h.labels_truncated ? ' · labels partial' : '') + (h.deck_skipped !== '0' ? ' · skipped ' + h.deck_skipped : '') + ' · gen ' + h.generation;
                    const perf = h.perf || {}, ms = function (name) { return perf[name] ? (Number(perf[name]) / 1000).toFixed(1) : '0'; };
                    el('perf').textContent = 'Rust · plan ' + ms('plan_us') + ' ms · decode ' + ms('decode_us') + ' ms · draw ' + ms('raster_us') +
                        ' ms · ' + (perf.pages || '0') + ' pages · ' + h.width + ' × ' + h.height + ' px · ' + h.format + ' · refinement off';
                }
            } catch (e) { report(e); }
            if (image) { image.onload = null; image.onerror = null; image.src = ''; }
            if (url) { URL.revokeObjectURL(url); }
            decode = null; acknowledge(h, disposition, ws, serial);
        }
        decode = function () { finish(null); };
        if (h.format === 'raw') {
            finish(function () {
                const rgba = new Uint8ClampedArray(packet.data.buffer, packet.data.byteOffset + 16, h.width * h.height * 4);
                context.putImageData(new ImageData(rgba, h.width, h.height), 0, 0);
            });
        } else {
            image = new Image(); url = URL.createObjectURL(new Blob([packet.data], {type: 'image/png'}));
            image.onload = function () {
                if (image.naturalWidth !== h.width || image.naturalHeight !== h.height) { notice('Decoded PNG dimensions mismatch'); finish(null); return; }
                finish(function () { context.drawImage(image, 0, 0); });
            };
            image.onerror = function () { notice('PNG decode failed'); finish(null); };
            timer = setTimeout(function () { notice('PNG decode timeout'); finish(null); }, 5000);
            image.src = url;
        }
    }
    function disconnect() {
        ++socketSerial; finishDecode();
        if (socket) { socket.onclose = null; socket.close(); socket = null; }
        epoch = ''; inflight = null; accepted = null; queue = [];
        if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null; }
    }
    function connect() {
        disconnect(); if (stopped || !currentId || !live()) { return; }
        const serial = socketSerial;
        const ws = new WebSocket(location.origin.replace(/^http/, 'ws') + '/api/v1/events',
            ['floe.v1', 'bundle.' + bundle, 'csrf.' + auth.csrf]);
        socket = ws; ws.binaryType = 'arraybuffer'; seq = '0'; connection('Connecting', false);
        ws.onmessage = function (event) {
            if (socket !== ws || serial !== socketSerial) { return; }
            if (typeof event.data !== 'string') { frame(event.data, ws, serial); return; }
            try {
                if (event.data.length > 256 * 1024) { throw new Error('Control reply limit'); }
                const m = JSON.parse(event.data);
                if (m.type === 'hello') {
                    if (m.protocol !== 1 || m.bundle !== bundle || m.view_id !== currentId) { throw new Error('Client/server version mismatch; reload'); }
                    epoch = m.connection_epoch; reconnectDelay = 500; connection('Local · connected', true);
                } else if (m.type === 'snapshot') { statusSnapshot(m); }
                else if (m.type === 'accepted') {
                    if (m.seq !== inflight) { throw new Error('Unexpected edit acknowledgement'); }
                    accepted = {rev: m.state_rev, render: m.render_rev};
                } else if (m.type === 'error') {
                    if (m.seq === inflight) { queue = []; accepted = {rev: '0', render: state.render_rev}; }
                    notice(message(m.code));
                }
            } catch (e) { report(e); ws.close(); }
        };
        ws.onclose = function () {
            if (socket !== ws || serial !== socketSerial) { return; }
            const uncertain = !!inflight || queue.length > 0;
            epoch = ''; socket = null; finishDecode(); queue = []; inflight = null; accepted = null;
            controls(); connection('Disconnected', false);
            if (uncertain) { notice('Connection interrupted. Pending input was not replayed; the restored view is authoritative.'); }
            if (!stopped && live()) {
                reconnectTimer = setTimeout(function () { restore().catch(report); }, reconnectDelay);
                reconnectDelay = Math.min(5000, reconnectDelay * 2);
            }
        };
        ws.onerror = function () { connection('Connection error', false); };
    }
    async function restore() {
        const current = await http('GET', '/api/v1/view', undefined, true);
        if (!current) { currentId = ''; state = null; controls(); return; }
        const changed = currentId !== current.view.view_id;
        if (changed) { displayed = false; canvas.width = 1; canvas.height = 1; el('empty').hidden = false; layerStart = 0; layerRev = ''; }
        currentId = current.view.view_id; currentSource = current.source_id; state = current.view;
        el('document-title').textContent = current.title; document.title = current.title + ' · floe2';
        el('source').value = currentSource; el('mode').value = current.mode;
        sourceSelection();
        levelIds = new Set(current.levels || []); el('levels-all').checked = current.levels === null;
        Array.from(el('level-list').querySelectorAll('input')).forEach(function (box) {
            box.checked = levelIds.has(box.value); box.disabled = el('levels-all').checked;
        });
        controls(); connect();
    }
    async function loadLayers() {
        if (!state || !currentId) { return; }
        const id = currentId, rev = state.state_rev, start = layerStart, ticket = ++layerLoad;
        layerRev = rev;
        const page = await http('GET', '/api/v1/views/' + id + '/layers/' + start);
        if (ticket !== layerLoad || id !== currentId || start !== layerStart || !state || page.state_rev !== state.state_rev) { return; }
        layerNext = page.next; el('layers').textContent = '';
        page.rows.forEach(function (r) {
            const row = document.createElement('div'); row.className = 'layer-row' + (r.parent ? ' child' : '') + (r.head ? ' head' : '');
            const check = document.createElement('input'); check.type = 'checkbox'; check.checked = r.visible;
            check.setAttribute('aria-label', 'Show ' + r.name); check.onchange = function () { edit({layer_change: {pair: r.pair, visible: check.checked}}); };
            const color = document.createElement('input'); color.type = 'color'; color.value = r.color; color.setAttribute('aria-label', 'Color ' + r.name);
            color.onchange = function () { edit({styles: [{pair: r.pair, color: color.value, fill: r.fill, width: r.width}]}); };
            const name = document.createElement('span'); name.className = 'layer-name'; name.textContent = r.name || r.pair.join('/'); name.title = r.name + (r.aliases.length ? ' · ' + r.aliases.join(', ') : '');
            row.appendChild(check); row.appendChild(color); row.appendChild(name); el('layers').appendChild(row);
        });
        if (!page.rows.length) { el('layers').textContent = 'No visible layer rows.'; }
        el('layers-count').textContent = (page.total ? (start + 1) + '–' + (start + page.rows.length) + ' / ' : '') + page.total;
        el('layers-prev').disabled = start === 0; el('layers-next').disabled = page.next === null;
    }
    async function moreLevels() {
        const source = el('source').value, page = await http('GET', '/api/v1/catalog/' + source + '/levels/' + (levelNext || 0));
        if (source !== el('source').value) { return; }
        page.levels.forEach(function (r) {
            const label = document.createElement('label'); label.className = 'check';
            const box = document.createElement('input'); box.type = 'checkbox'; box.value = r.id; box.checked = levelIds.has(r.id); box.disabled = el('levels-all').checked;
            box.onchange = function () { if (box.checked) { levelIds.add(r.id); } else { levelIds.delete(r.id); } };
            label.appendChild(box); label.appendChild(document.createTextNode(r.title || ('Level ' + r.id))); el('level-list').appendChild(label);
        });
        levelNext = page.next; el('level-more').hidden = levelNext === null;
    }
    function sourceSelection() {
        const source = catalog.find(function (r) { return r.source_id === el('source').value; });
        if (!source) { return; }
        el('mode').disabled = !source.deck; el('level-options').hidden = !source.deck;
        if (!source.deck) { el('mode').value = 'level'; }
        if (levelSource !== source.source_id) {
            levelSource = source.source_id; levelNext = null; levelIds = new Set(); el('level-list').textContent = ''; el('levels-all').checked = true;
            if (source.deck) { moreLevels().catch(report); }
        }
        el('source-note').textContent = source.deck ? 'Jobdeck · select levels before opening.' : 'OASIS layout · current index required.';
    }
    function levels() {
        if (el('levels-all').checked) { return {mode: 'all'}; }
        if (!levelIds.size) { throw new Error('Select at least one level.'); }
        return {mode: 'only', ids: Array.from(levelIds)};
    }
    function operationLabel(op) {
        const p = op.native || {};
        return op.kind + ' · ' + op.phase + (p.phase ? ' · ' + p.phase : '') + (op.error ? ' · ' + message(op.error) : '');
    }
    async function operationState() {
        const all = await http('GET', '/api/v1/operations');
        ownerBusy = all.active !== null; el('cancel-job').disabled = !ownerBusy;
        el('cancel-job').dataset.seq = all.active || ''; controls();
        const recent = all.history || [], last = recent[recent.length - 1];
        if (last) { el('operation').textContent = operationLabel(last); }
        if (!ownerBusy && last && ['failed', 'incomplete', 'cancelled'].includes(last.phase)) {
            notice(message(last.error || last.phase));
            if (!currentId) { el('empty-message').textContent = message(last.error || last.phase); connection('Local · ready', true); }
        }
        if (ownerBusy) { operationTimer = setTimeout(function () { operationState().catch(report); }, 500); }
        else if (last && last.kind === 'open' && last.phase === 'succeeded' && last.view_id !== currentId) { await restore(); }
        return all;
    }
    async function submitOperation(request) {
        if (ownerBusy || submitting) { throw new Error('An operation is already running.'); }
        submitting = true; controls();
        if (operationTimer) { clearTimeout(operationTimer); operationTimer = null; }
        try {
            const all = await operationState();
            if (all.active !== null) { throw new Error('An operation is already running.'); }
            request.seq = P.next(all.last_seq); ownerBusy = true; controls(); notice('');
            try { await http('POST', '/api/v1/operations', request); }
            finally { await operationState(); }
        } finally { submitting = false; controls(); }
    }
    async function openSource(startup) {
        const request = startup || {kind: 'open', mode: el('mode').value, source_id: el('source').value, levels: levels(), body: {}};
        request.body.pixels = dims().pixels;
        await submitOperation(request);
    }
    async function start() {
        const fragment = location.hash;
        if (fragment) { history.replaceState(null, '', location.pathname); }
        if (/^#bootstrap=[0-9a-f]{64}$/.test(fragment)) {
            auth = await http('POST', '/api/v1/session/exchange', {bootstrap: fragment.slice(11), protocol: 1, bundle: bundle});
            try { sessionStorage.setItem(sessionKey, JSON.stringify(auth)); } catch (_) { notice('Session storage unavailable. Reloading requires a new local session.'); }
        } else {
            try { auth = JSON.parse(sessionStorage.getItem(sessionKey)); } catch (_) { auth = null; }
            if (!auth) { throw new Error('Launch with floe2-web view and use its private session link.'); }
        }
        const caps = await http('GET', '/api/v1/capabilities');
        if (caps.protocol !== 1 || caps.bundle !== bundle) { throw new Error('Client/server version mismatch. Reload the page.'); }
        catalog = (await http('GET', '/api/v1/catalog')).sources;
        el('source').textContent = '';
        catalog.forEach(function (s) { const option = document.createElement('option'); option.value = s.source_id; option.textContent = s.title; el('source').appendChild(option); });
        sourceSelection();
        const operations = await operationState();
        await restore();
        if (!currentId && operations.last_seq === '0') {
            const startup = (await http('GET', '/api/v1/startup')).request;
            if (startup) {
                const n = startup.body.navigation;
                if (n && n.kind === 'goto') { el('goto-x').value = n.center_um[0]; el('goto-y').value = n.center_um[1]; el('goto-width').value = n.width_um; }
                await openSource(startup);
            }
            else { el('empty-message').textContent = 'Choose a registered source and open its index.'; connection('Local · ready', true); }
        }
    }
    el('source').onchange = sourceSelection;
    el('open').onclick = function () { openSource(null).catch(report); };
    el('close').onclick = async function () {
        if (!currentId) { return; }
        try { await http('DELETE', '/api/v1/views/' + currentId); disconnect(); state = null; currentId = ''; controls(); displayed = false; canvas.width = 1; canvas.height = 1; el('empty').hidden = false; el('empty-message').textContent = 'View closed. Choose a source to reopen.'; el('rendering').hidden = true; el('layers').textContent = ''; }
        catch (e) { report(e); }
    };
    el('logout').onclick = async function () {
        stopped = true; disconnect(); if (operationTimer) { clearTimeout(operationTimer); }
        try { await http('DELETE', '/api/v1/session'); } catch (e) { report(e); }
        try { sessionStorage.removeItem(sessionKey); } catch (_) { /* storage may be disabled */ }
        state = null; currentId = ''; controls(); connection('Session ended', false); notice('Session ended. Close this tab.');
    };
    el('levels-all').onchange = function () { Array.from(el('level-list').querySelectorAll('input')).forEach(function (box) { box.disabled = el('levels-all').checked; }); };
    el('level-more').onclick = function () { moreLevels().catch(report); };
    el('layers-prev').onclick = function () { layerStart = Math.max(0, layerStart - 64); loadLayers().catch(report); };
    el('layers-next').onclick = function () { if (layerNext !== null) { layerStart = layerNext; loadLayers().catch(report); } };
    el('layers-all').onclick = function () { edit({layers: {mode: 'all'}}); };
    el('layers-none').onclick = function () { edit({layers: {mode: 'none'}}); };
    ['depth', 'detail', 'thin'].forEach(function (id) { el(id).onchange = function () { const body = {}; body[id] = el(id).value; edit(body); }; });
    ['frames', 'labels', 'mono'].forEach(function (id) { el(id).onchange = function () { const body = {}; body[id] = el(id).checked; edit(body); }; });
    const nav = function (n) { edit({navigation: n}); };
    const zoom = function (factor, anchor) { nav({kind: 'zoom', factor: factor, anchor: anchor || [0.5, 0.5]}); };
    el('fit').onclick = function () { nav({kind: 'fit'}); };
    el('zoom-in').onclick = function () { zoom(0.8); };
    el('zoom-out').onclick = function () { zoom(1.25); };
    el('goto-form').onsubmit = function (event) {
        event.preventDefault();
        try { nav({kind: 'goto', center_um: [P.decimal(el('goto-x').value), P.decimal(el('goto-y').value)], width_um: P.decimal(el('goto-width').value)}); }
        catch (e) { report(e); }
    };
    viewport.addEventListener('mousedown', function () { viewport.focus(); });
    viewport.addEventListener('keydown', function (event) {
        if (event.ctrlKey || event.metaKey || event.altKey) { return; }
        const amount = event.shiftKey ? 0.1 : 0.5, key = event.key;
        const directions = {ArrowLeft: [-amount, 0], ArrowRight: [amount, 0], ArrowUp: [0, amount], ArrowDown: [0, -amount]};
        if (directions[key]) { event.preventDefault(); nav({kind: 'pan', x: directions[key][0], y: directions[key][1], snap: true}); }
        else if (key === '+' || key === '=') { event.preventDefault(); zoom(0.8); }
        else if (key === '-') { event.preventDefault(); zoom(1.25); }
        else if (key.toLowerCase() === 'f') { event.preventDefault(); nav({kind: 'fit'}); }
    });
    viewport.addEventListener('wheel', function (event) {
        if (!live()) { return; } event.preventDefault();
        const rect = viewport.getBoundingClientRect();
        zoom(event.deltaY < 0 ? 0.8 : 1.25, [Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width)), Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height))]);
    }, {passive: false});
    el('index').onclick = function () {
        try { submitOperation({kind: 'index', source_id: el('source').value, levels: levels(), options: {jobs: Number(el('index-jobs').value), force: el('index-force').checked, lod: el('index-lod').checked, occupancy: el('index-occupancy').checked}}).catch(report); }
        catch (e) { report(e); }
    };
    el('cancel-job').onclick = function () { const id = el('cancel-job').dataset.seq; if (id) { http('POST', '/api/v1/operations/' + id + '/cancel', {}).then(operationState).catch(report); } };
    window.addEventListener('resize', function () {
        clearTimeout(resizeTimer); resizeTimer = setTimeout(function () {
            if (!live()) { return; }
            try { const size = dims(); positionCanvas(size); if (size.pixels[0] !== state.pixels[0] || size.pixels[1] !== state.pixels[1]) { edit({pixels: size.pixels}); } }
            catch (e) { report(e); }
        }, 120);
    });
    document.addEventListener('visibilitychange', function () { if (document.hidden) { finishDecode(); } else if (live() && !stopped) { connect(); } });
    setInterval(function () { if (socket && socket.readyState === WebSocket.OPEN && epoch) { try { send({type: 'ping'}); } catch (e) { report(e); } } }, 10000);
    window.addEventListener('pagehide', function () { disconnect(); clearTimeout(operationTimer); });
    window.addEventListener('pageshow', function (event) {
        if (event.persisted && auth && !stopped) { operationState().then(restore).catch(report); }
    });
    start().catch(function (e) { connection('Not connected', false); report(e); el('empty-message').textContent = e.message; });
}());
