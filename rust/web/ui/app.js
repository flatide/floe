/* Local, ES2017 CAD client. The server owns world coordinates and view state. */
(function () {
    'use strict';
    const P = window.FloeProtocol;
    const bundle = document.querySelector('meta[name="floe-bundle"]').content;
    const el = function (id) { return document.getElementById(id); };
    const canvas = el('canvas'), viewport = el('viewport');
    const context = canvas.getContext('2d', {alpha: false});
    const marginCanvas = el('margin-canvas'), marginContext = marginCanvas.getContext('2d', {alpha: false});
    marginCanvas.hidden = true;
    let foregroundFrame = null, marginFrame = null, inflightBody = null, foregroundPerf = '';
    let gesture = null, dragShift = null, lastPlacement = null;
    let drcPanel = null, displayProjection = null, frozenProjection = null;
    const sessionKey = 'floe-session:' + location.origin;
    let auth = null, stopped = false, socket = null, epoch = '', state = null;
    let seq = '0', queue = [], inflight = null, accepted = null, lastSend = 0;
    let socketSerial = 0, decode = null, reconnectTimer = null, reconnectDelay = 500;
    let catalog = [], currentId = '', currentSource = '', ownerBusy = false, submitting = false;
    let layerStart = 0, layerNext = null, layerLoad = 0, layerKey = '', selectedStyle = null;
    let levelNext = null, levelSource = '', levelIds = new Set(), levelLoad = 0, levelBusy = false;
    let lastDigit = '', lastDigitAt = 0;
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
        closed: 'This session is closed. Start a new local session.',
        drc_changed_or_corrupt: 'The DRC pack or review sidecar changed or is corrupt. Restart with a valid pack.',
        drc_read_error: 'The registered DRC file cannot be read.',
        drc_read_limit: 'This DRC item exceeds the read/response limit; no partial geometry was accepted.',
        drc_context_changed: 'The view changed while reading DRC. Select the error again.',
        drc_busy: 'The DRC read queue is busy. Retry this page.',
        drc_closed: 'The DRC reader is closed.',
        invalid_drc_request: 'Invalid DRC index, cursor, coordinate or page limit.'
    };
    function notice(text) { el('notice').textContent = text || ''; el('notice').hidden = !text; }
    function message(error) { return errors[error] || String(error || 'Request failed'); }
    function report(error) { notice(message(error.message || error)); }
    function http(method, path, body, missing, token) {
        return new Promise(function (resolve, reject) {
            const xhr = new XMLHttpRequest();
            if (token && token.cancelled) { reject(new Error('Request cancelled')); return; }
            if (token) { token.abort = function () { xhr.abort(); }; }
            function done() { if (token) { token.abort = null; } }
            xhr.open(method, path); xhr.timeout = 8000;
            if (auth) { xhr.setRequestHeader('X-Floe-CSRF', auth.csrf); }
            if (body !== undefined) { xhr.setRequestHeader('Content-Type', 'application/json'); }
            xhr.onload = function () {
                done();
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
            xhr.onerror = function () { done(); reject(new Error('Local service is unavailable')); };
            xhr.onabort = function () { done(); reject(new Error('Request cancelled')); };
            xhr.ontimeout = function () { done(); reject(new Error('Request timed out; its outcome may be pending. Check operation status before retrying.')); };
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
    function positionBuffer(target, size, p) {
        target.style.width = (target.width / size.dpr) + 'px'; target.style.height = (target.height / size.dpr) + 'px';
        target.style.left = (size.left - p[0] / size.dpr) + 'px'; target.style.top = (size.top - p[1] / size.dpr) + 'px';
    }
    function pendingPan() {
        const items = (inflightBody ? [inflightBody] : []).concat(queue), delta = [0, 0];
        for (let i = 0; i < items.length; ++i) {
            const n = items[i].navigation;
            if (Object.keys(items[i]).length !== 1 || !n || n.kind !== 'pan') { return null; }
            const period = n.snap ? 16 : 1;
            delta[0] += P.roundEven(n.x * state.pixels[0] / period) * period;
            delta[1] -= P.roundEven(n.y * state.pixels[1] / period) * period;
        }
        if (dragShift) { delta[0] += dragShift[0]; delta[1] += dragShift[1]; }
        return delta;
    }
    function present() {
        const size = dims(), delta = state && pendingPan();
        const sameSize = state && size.pixels[0] === state.pixels[0] && size.pixels[1] === state.pixels[1];
        const at = function (h) { const p = sameSize && delta && P.placement(h, state); return p && [p[0] + delta[0], p[1] + delta[1]]; };
        const mp = at(marginFrame), fp = at(foregroundFrame);
        marginCanvas.hidden = !mp;
        if (mp) { positionBuffer(marginCanvas, size, mp); }
        const full = mp && marginFrame.complete && mp[0] >= 0 && mp[1] >= 0 &&
            mp[0] + size.pixels[0] <= marginCanvas.width && mp[1] + size.pixels[1] <= marginCanvas.height;
        canvas.hidden = !!full;
        if (fp) { positionBuffer(canvas, size, fp); } else { positionCanvas(size); }
        lastPlacement = {pixels: size.pixels, margin: mp, full: !!full,
            foreground: fp || [-Math.round((size.pixels[0] - canvas.width) / 2), -Math.round((size.pixels[1] - canvas.height) / 2)]};
        if (full) { displayProjection = window.FloeDRC.projection(marginFrame, mp, state.dbu_um); }
        else if (foregroundFrame) { displayProjection = window.FloeDRC.projection(foregroundFrame, lastPlacement.foreground, state.dbu_um); }
        else { displayProjection = frozenProjection && window.FloeDRC.shifted(frozenProjection.projection,
            [-Math.round((size.pixels[0] - frozenProjection.pixels[0]) / 2), -Math.round((size.pixels[1] - frozenProjection.pixels[1]) / 2)]); }
        if (drcPanel) { drcPanel.paint(displayProjection, size); }
        if (full) {
            const pending = !!inflightBody || queue.length > 0 || !!dragShift;
            el('status').textContent = (pending ? 'Pan preview' : 'Live') + ' · margin crop · gen ' + marginFrame.generation;
        }
        el('perf').textContent = foregroundPerf + (full ? ' · crop (no foreground render)' : '');
        el('margin-info').textContent = state && state.capabilities.margin ?
            (state.margin_working ? 'Prefetching' : (mp ? 'Margin ready' : 'Margin pending')) +
            (marginFrame ? ' · ' + (Number((marginFrame.perf || {}).raster_us || 0) / 1000).toFixed(1) + ' ms bg' : '') +
            (marginFrame && marginFrame.labels_truncated ? ' · labels partial' : '') + (state.margin_failure ? ' · prefetch failed' : '') : '';
    }
    function freezeMargin() {
        // Freeze the actual composite, including a truncated margin's incoming
        // strip. A non-period mouse release must not recenter the previous image.
        const p = lastPlacement;
        if (displayed && p) {
            frozenProjection = {projection: displayProjection, pixels: p.pixels.slice()};
            const [w, h] = p.pixels, mp = p.margin, fp = p.foreground;
            const copy = !!mp || fp[0] !== 0 || fp[1] !== 0 || canvas.width !== w || canvas.height !== h;
            if (copy) {
                if (p.full && mp) {
                    canvas.width = w; canvas.height = h; context.imageSmoothingEnabled = false;
                    context.drawImage(marginCanvas, mp[0], mp[1], w, h, 0, 0, w, h);
                } else {
                    const temp = document.createElement('canvas'); temp.width = w; temp.height = h;
                    const ctx = temp.getContext('2d', {alpha: false}); ctx.imageSmoothingEnabled = false;
                    if (mp) { ctx.drawImage(marginCanvas, -mp[0], -mp[1]); }
                    if (!canvas.hidden) { ctx.drawImage(canvas, -fp[0], -fp[1]); }
                    canvas.width = w; canvas.height = h; context.imageSmoothingEnabled = false;
                    context.drawImage(temp, 0, 0); temp.width = 1; temp.height = 1;
                }
            }
            foregroundFrame = null; canvas.hidden = false; positionCanvas(dims());
            lastPlacement = {pixels: p.pixels, margin: null, full: false, foreground: [0, 0]};
        }
        marginCanvas.hidden = true;
    }
    function clearBuffers() {
        foregroundFrame = null; marginFrame = null; inflightBody = null; foregroundPerf = '';
        lastPlacement = null; dragShift = null;
        displayProjection = null; frozenProjection = null;
        canvas.hidden = false; canvas.width = 1; canvas.height = 1;
        marginCanvas.hidden = true; marginCanvas.width = 1; marginCanvas.height = 1;
    }
    function live() { return state && !['closed', 'failed'].includes(state.status); }
    function controls() {
        const enabled = live() && socket && socket.readyState === WebSocket.OPEN && !!epoch;
        ['fit', 'zoom-in', 'zoom-out', 'goto', 'depth', 'detail', 'thin', 'frames', 'labels', 'mono', 'layers-all', 'layers-none'].forEach(function (id) { el(id).disabled = !enabled; });
        el('labels').disabled = !enabled || !state.capabilities.labels;
        el('font-px').disabled = !enabled || !state.capabilities.labels;
        el('open').disabled = submitting || ownerBusy || !!live();
        el('close').disabled = !currentId || submitting || ownerBusy;
        el('index').disabled = submitting || ownerBusy;
        if (drcPanel) { drcPanel.contextChanged(); }
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
            inflightBody = body;
            inflight = send({type: 'view.set', connection_epoch: epoch, view_id: currentId,
                base_state_rev: state.state_rev, body: body});
            lastSend = Date.now();
        } catch (e) { inflight = null; inflightBody = null; queue = []; report(e); }
    }
    function edit(body) {
        if (!live() || !epoch) { notice('Open a connected view first.'); return; }
        if (queue.length >= 64) { notice('Input queue is full. This input was not applied.'); return; }
        if (!body.navigation || body.navigation.kind !== 'pan' || !body.navigation.snap) { freezeMargin(); }
        queue.push(body); pump(); present();
    }
    function statusSnapshot(s) {
        if (s.view_id !== currentId || s.connection_epoch !== epoch) { return; }
        if (state && state.connection_epoch === epoch && P.compare(s.state_rev, state.state_rev) < 0) { return; }
        if (gesture && gesture.active() && state && (s.state_rev !== state.state_rev || s.connection_epoch !== state.connection_epoch)) { gesture.cancel(); }
        if (marginFrame && !P.placement(marginFrame, s)) {
            freezeMargin(); marginFrame = null; marginCanvas.width = 1; marginCanvas.height = 1;
        }
        state = s; controls();
        el('rendering').hidden = !['opening', 'rendering', 'cancelling'].includes(s.status);
        el('rendering').textContent = s.status === 'opening' ? 'Opening index' : 'Rendering';
        if (!displayed) { el('empty-message').textContent = s.status === 'failed' ? message(s.failure) : 'Preparing the first frame…'; }
        if (s.failure) { notice(message(s.failure)); }
        if (document.activeElement !== el('depth')) { el('depth').value = s.depth; }
        el('max-depth').textContent = s.max_depth === null ? '' : '/ ' + s.max_depth;
        ['detail', 'thin'].forEach(function (id) { el(id).value = s[id]; });
        ['frames', 'labels', 'mono'].forEach(function (id) { el(id).checked = s[id]; });
        if (document.activeElement !== el('font-px')) { el('font-px').value = s.font_px; }
        const b = s.bbox_dbu.map(Number), dbu = Number(s.dbu_um);
        el('viewport-info').textContent = ((b[2] - b[0]) * dbu).toPrecision(6) + ' × ' + ((b[3] - b[1]) * dbu).toPrecision(6) + ' µm';
        el('status').textContent = s.status + ' · depth ' + s.depth + ' · thin:' + s.effective_thin + (s.source_stale ? ' · SOURCE STALE' : '');
        if (accepted && P.compare(s.state_rev, accepted.rev) >= 0) { accepted = null; inflight = null; inflightBody = null; pump(); }
        present();
        if (s.render_key !== layerKey) { loadLayers().catch(report); }
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
            (!accepted || (h.purpose === 'foreground' && P.compare(h.render_rev, accepted.render) >= 0)) && !document.hidden; };
        if (!valid()) { acknowledge(h, 'discarded', ws, serial); return; }
        if (decode) { notice('Frame credit violation'); ws.close(); return; }
        let done = false, image = null, url = null, timer = null;
        function finish(draw) {
            if (done) { return; } done = true;
            if (timer) { clearTimeout(timer); }
            let disposition = 'discarded';
            try {
                if (draw && valid()) {
                    const target = h.purpose === 'margin' ? marginCanvas : canvas;
                    const ctx = h.purpose === 'margin' ? marginContext : context;
                    if (target.width !== h.width || target.height !== h.height) { target.width = h.width; target.height = h.height; }
                    ctx.imageSmoothingEnabled = false; draw(ctx);
                    if (h.purpose === 'margin') { marginFrame = h; } else { foregroundFrame = h; frozenProjection = null; }
                    if (!displayed && document.activeElement === document.body) { viewport.focus(); }
                    displayed = true; el('empty').hidden = true; disposition = 'displayed';
                    target.dataset.frameId = h.frame_id; target.dataset.renderRev = h.render_rev;
                    target.dataset.bboxDbu = JSON.stringify(h.bbox_dbu);
                    el('status').textContent = (h.complete ? 'Live' : 'INCOMPLETE') + (h.approximate ? ' · summary/LOD' : '') +
                        (h.labels_truncated ? ' · labels partial' : '') + (h.deck_skipped !== '0' ? ' · skipped ' + h.deck_skipped : '') + ' · gen ' + h.generation;
                    const perf = h.perf || {}, ms = function (name) { return perf[name] ? (Number(perf[name]) / 1000).toFixed(1) : '0'; };
                    if (h.purpose === 'foreground') {
                        foregroundPerf = 'Rust foreground · plan ' + ms('plan_us') + ' ms · decode ' + ms('decode_us') + ' ms · draw ' + ms('raster_us') +
                            ' ms · ' + (perf.pages || '0') + ' pages · ' + h.width + ' × ' + h.height + ' px · ' + h.format + ' · refinement off';
                    }
                    present();
                }
            } catch (e) { report(e); }
            if (image) { image.onload = null; image.onerror = null; image.src = ''; }
            if (url) { URL.revokeObjectURL(url); }
            decode = null; acknowledge(h, disposition, ws, serial);
        }
        decode = function () { finish(null); };
        if (h.format === 'raw') {
            finish(function (ctx) {
                const rgba = new Uint8ClampedArray(packet.data.buffer, packet.data.byteOffset + 16, h.width * h.height * 4);
                ctx.putImageData(new ImageData(rgba, h.width, h.height), 0, 0);
            });
        } else {
            image = new Image(); url = URL.createObjectURL(new Blob([packet.data], {type: 'image/png'}));
            image.onload = function () {
                if (image.naturalWidth !== h.width || image.naturalHeight !== h.height) { notice('Decoded PNG dimensions mismatch'); finish(null); return; }
                finish(function (ctx) { ctx.drawImage(image, 0, 0); });
            };
            image.onerror = function () { notice('PNG decode failed'); finish(null); };
            timer = setTimeout(function () { notice('PNG decode timeout'); finish(null); }, 5000);
            image.src = url;
        }
    }
    function disconnect() {
        if (gesture) { gesture.cancel(); }
        freezeMargin();
        ++socketSerial; finishDecode();
        if (socket) { socket.onclose = null; socket.close(); socket = null; }
        epoch = ''; inflight = null; inflightBody = null; accepted = null; queue = [];
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
            if (gesture) { gesture.cancel(); }
            const uncertain = !!inflight || queue.length > 0;
            freezeMargin(); epoch = ''; socket = null; finishDecode(); queue = []; inflight = null; inflightBody = null; accepted = null;
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
        if (changed) { displayed = false; clearBuffers(); el('empty').hidden = false; layerStart = 0; layerKey = ''; selectedStyle = null; el('style-editor').hidden = true; }
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
        const id = currentId, key = state.render_key, start = layerStart, ticket = ++layerLoad;
        layerKey = key;
        let page;
        try { page = await http('GET', '/api/v1/views/' + id + '/layers/' + start); }
        catch (e) { if (ticket === layerLoad) { layerKey = ''; } throw e; }
        if (ticket !== layerLoad || id !== currentId || start !== layerStart || !state || page.render_key !== state.render_key) { return; }
        layerNext = page.next; el('layers').textContent = '';
        page.rows.forEach(function (r) {
            const row = document.createElement('div'); row.className = 'layer-row' + (r.parent ? ' child' : '') + (r.head ? ' head' : '');
            const check = document.createElement('input'); check.type = 'checkbox'; check.checked = r.visible;
            check.setAttribute('aria-label', 'Show ' + r.name); check.onchange = function () { edit({layer_change: {pair: r.pair, visible: check.checked}}); };
            const color = document.createElement('input'); color.type = 'color'; color.value = r.color; color.setAttribute('aria-label', 'Color ' + r.name);
            color.onchange = function () {
                if (!state || state.render_key !== page.render_key) { notice('Layer styles changed. Select the layer again.'); return; }
                edit({styles: [{pair: r.pair, color: color.value, fill: r.fill, width: r.width}]});
            };
            const name = document.createElement('span'); name.className = 'layer-name'; name.textContent = r.name || r.pair.join('/'); name.title = r.name + (r.aliases.length ? ' · ' + r.aliases.join(', ') : '');
            const style = document.createElement('button'); style.className = 'layer-edit'; style.textContent = '⋯';
            style.setAttribute('aria-label', 'Edit style ' + r.name);
            style.onclick = function () {
                selectedStyle = {row: r, key: page.render_key, view: id};
                el('style-title').textContent = r.name; el('style-fill').value = r.fill.kind; el('style-width').value = r.width;
                el('style-pattern').value = (r.fill.rows || new Array(16).fill(0xaaaa)).map(function (n) { return n.toString(16).padStart(4, '0'); }).join(' ');
                el('style-editor').hidden = false; patternControls(); el('style-fill').focus();
            };
            row.appendChild(check); row.appendChild(color); row.appendChild(name); row.appendChild(style); el('layers').appendChild(row);
        });
        if (!page.rows.length) { el('layers').textContent = 'No visible layer rows.'; }
        el('layers-count').textContent = (page.total ? (start + 1) + '–' + (start + page.rows.length) + ' / ' : '') + page.total;
        el('layers-prev').disabled = start === 0; el('layers-next').disabled = page.next === null;
    }
    async function moreLevels() {
        if (levelBusy) { return; }
        const source = el('source').value, ticket = ++levelLoad;
        levelBusy = true; el('level-more').disabled = true;
        let page;
        try { page = await http('GET', '/api/v1/catalog/' + source + '/levels/' + (levelNext || 0)); }
        finally { if (ticket === levelLoad) { levelBusy = false; el('level-more').disabled = false; } }
        if (source !== el('source').value || ticket !== levelLoad) { return; }
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
            ++levelLoad; levelBusy = false;
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
        if (!context || !marginContext || typeof WebSocket !== 'function' || typeof TextEncoder !== 'function' ||
            typeof TextDecoder !== 'function' || typeof ImageData !== 'function' || typeof URL.createObjectURL !== 'function' ||
            typeof window.requestAnimationFrame !== 'function' || typeof window.cancelAnimationFrame !== 'function') {
            throw new Error('This browser lacks Canvas 2D/binary WebSocket/UTF-8 image APIs. Use a supported Firefox and restart the local session.');
        }
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
        if (caps.drc) { await drcPanel.init(); }
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
        try { await http('DELETE', '/api/v1/views/' + currentId); disconnect(); state = null; currentId = ''; controls(); displayed = false; clearBuffers(); el('empty').hidden = false; el('empty-message').textContent = 'View closed. Choose a source to reopen.'; el('rendering').hidden = true; el('layers').textContent = ''; }
        catch (e) { report(e); }
    };
    el('logout').onclick = async function () {
        if (drcPanel) { drcPanel.stop(); }
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
    el('font-px').onchange = function () {
        const n = Number(el('font-px').value);
        if (!Number.isInteger(n) || n < 6 || n > 96) { notice('Label size must be 6–96 device pixels.'); return; }
        edit({font_px: n});
    };
    function patternControls() { el('style-pattern').hidden = el('pattern-label').hidden = el('style-fill').value !== 'pattern'; }
    el('style-fill').onchange = patternControls;
    el('style-cancel').onclick = function () { selectedStyle = null; el('style-editor').hidden = true; };
    el('style-editor').onsubmit = function (event) {
        event.preventDefault();
        if (!selectedStyle || !state || selectedStyle.view !== currentId || selectedStyle.key !== state.render_key) { notice('Layer styles changed. Select the layer again before applying.'); return; }
        const fill = {kind: el('style-fill').value}, width = Number(el('style-width').value);
        if (!Number.isInteger(width) || width < 1 || width > 8) { notice('Line width must be 1–8 device pixels.'); return; }
        if (fill.kind === 'pattern') {
            const rows = el('style-pattern').value.trim().split(/\s+/);
            if (rows.length !== 16 || !rows.every(function (s) { return /^[0-9a-f]{4}$/i.test(s); })) { notice('A pattern needs exactly 16 four-digit hex rows.'); return; }
            fill.rows = rows.map(function (s) { return parseInt(s, 16); });
        }
        edit({styles: [{pair: selectedStyle.row.pair, color: selectedStyle.row.color, fill: fill, width: width}]});
        selectedStyle = null; el('style-editor').hidden = true;
    };
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
        if (!live()) { return; }
        if (gesture && gesture.active()) { if (event.key === 'Escape') { event.preventDefault(); gesture.cancel(); } return; }
        if (event.metaKey || event.altKey || event.isComposing) { return; }
        let key = event.key;
        if (key.length === 1 && key.charCodeAt(0) > 127 && /^Key[A-Z]$/.test(event.code || '')) { key = event.shiftKey ? event.code.slice(3) : event.code.slice(3).toLowerCase(); }
        if (event.ctrlKey) {
            if (key.toLowerCase() === 'a') { event.preventDefault(); nav({kind: 'fit'}); }
            else if (key.toLowerCase() === 'z') { event.preventDefault(); zoom(0.5); }
            else if (key === '.') { event.preventDefault(); el('goto-x').focus(); el('goto-x').select(); }
            return;
        }
        if (drcPanel && drcPanel.key(key)) { event.preventDefault(); return; }
        const amount = event.shiftKey ? 0.1 : 0.5;
        const directions = {ArrowLeft: [-amount, 0], ArrowRight: [amount, 0], ArrowUp: [0, amount], ArrowDown: [0, -amount]};
        if (directions[key]) { event.preventDefault(); nav({kind: 'pan', x: directions[key][0], y: directions[key][1], snap: true}); }
        else if (key === '+' || key === '=') { event.preventDefault(); zoom(0.8); }
        else if (key === '-') { event.preventDefault(); zoom(1.25); }
        else if (key === 'f' || key === 'C') { event.preventDefault(); edit({frames: !state.frames}); }
        else if (key === 'b') { event.preventDefault(); edit({mono: !state.mono}); }
        else if (key === 'Z') { event.preventDefault(); zoom(2); }
        else if (key === 'g') { event.preventDefault(); el('goto-x').focus(); el('goto-x').select(); }
        else if (key === 'd') { event.preventDefault(); el('detail').focus(); }
        else if (/^[0-9]$/.test(key)) {
            event.preventDefault(); const depth = key === '9' && lastDigit === '9' && Date.now() - lastDigitAt < 1000 ? 'full' : key;
            lastDigit = depth === 'full' ? '' : key; lastDigitAt = Date.now(); edit({depth: depth});
        }
    });
    viewport.addEventListener('wheel', function (event) {
        if (!live()) { return; } event.preventDefault();
        if ((gesture && gesture.active()) || event.buttons) { return; }
        const rect = viewport.getBoundingClientRect();
        zoom(event.deltaY < 0 ? 0.8 : 1.25, [Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width)), Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height))]);
    }, {passive: false});
    gesture = window.FloeGestures.bind({viewport: viewport, window: window, document: document,
        dimensions: dims, ready: function () { return live() && displayed && !!epoch && !inflight && queue.length === 0; },
        stamp: function () { return currentId + ':' + epoch + ':' + (state ? state.state_rev : ''); },
        requestAnimationFrame: function (fn) { return window.requestAnimationFrame(fn); },
        cancelAnimationFrame: function (id) { window.cancelAnimationFrame(id); },
        preview: function (p, paint) { dragShift = p; if (paint) { present(); } },
        cursor: function (active) { viewport.style.cursor = active ? 'grabbing' : ''; }, pan: nav,
        // DRC owns display-space marker hits. Native geometry queries remain
        // disabled until the expected/actual query-scene contract is wired.
        click: function (x, y, twice) { if (drcPanel) { drcPanel.click(x, y, twice); } }});
    el('index').onclick = function () {
        try { submitOperation({kind: 'index', source_id: el('source').value, levels: levels(), options: {jobs: Number(el('index-jobs').value), force: el('index-force').checked, lod: el('index-lod').checked, occupancy: el('index-occupancy').checked}}).catch(report); }
        catch (e) { report(e); }
    };
    el('cancel-job').onclick = function () { const id = el('cancel-job').dataset.seq; if (id) { http('POST', '/api/v1/operations/' + id + '/cancel', {}).then(operationState).catch(report); } };
    function resized() {
        clearTimeout(resizeTimer); resizeTimer = setTimeout(function () {
            if (!live() || stopped || document.hidden) { return; }
            try {
                const size = dims();
                const target = (inflightBody ? [inflightBody] : []).concat(queue).filter(function (b) { return b.pixels; });
                const pendingSize = target.length ? target[target.length - 1].pixels : state.pixels;
                if (size.pixels[0] !== pendingSize[0] || size.pixels[1] !== pendingSize[1]) { edit({pixels: size.pixels}); }
                present();
            }
            catch (e) { report(e); }
        }, 120);
    }
    window.addEventListener('resize', resized);
    // Optional on older Firefox. Explicit panel/window resize paths remain;
    // notices are overlays and never change the viewport's layout size.
    const sizeObserver = typeof window.ResizeObserver === 'function' ? new window.ResizeObserver(resized) : null;
    if (sizeObserver) { sizeObserver.observe(viewport); }
    drcPanel = window.FloeDRC.bind({document: document, window: window, protocol: P, http: http,
        stateStore: window.FloePanelState,
        context: function () { return !stopped && state && currentId ? {id: currentId, source: currentSource, state: state,
            connected: !!epoch && !!socket && socket.readyState === WebSocket.OPEN, pending: !!inflight || queue.length > 0 || !!dragShift} : null; },
        navigate: nav, resize: resized});
    document.addEventListener('visibilitychange', function () { if (document.hidden) { finishDecode(); } else if (live() && !stopped) { connect(); } });
    setInterval(function () { if (socket && socket.readyState === WebSocket.OPEN && epoch) { try { send({type: 'ping'}); } catch (e) { report(e); } } }, 10000);
    window.addEventListener('pagehide', function () { disconnect(); clearTimeout(operationTimer); clearTimeout(resizeTimer); if (sizeObserver) { sizeObserver.disconnect(); } drcPanel.stop(); });
    window.addEventListener('pageshow', function (event) {
        if (event.persisted && auth && !stopped) { if (sizeObserver) { sizeObserver.observe(viewport); } drcPanel.resume().then(operationState).then(restore).then(resized).catch(report); }
    });
    start().catch(function (e) { connection('Not connected', false); report(e); el('empty-message').textContent = e.message; });
}());
