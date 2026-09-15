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
    el('zoom-band').hidden = el('zoom-band-hint').hidden = true;
    let foregroundFrame = null, marginFrame = null, inflightBody = null, foregroundPerf = '';
    let gesture = null, dragShift = null, lastPlacement = null;
    let drcPanel = null, displayProjection = null, frozenProjection = null;
    let inspector = null, measurement = null, clipper = null, snapshots = null, overlayMode = 'all', pickedPairs = [];
    let settings = null, defaults = null, about = null, sessionExit = null;
    let minimap = null, launcher = null, picker = null, indexOpen = null, palette = null;
    const rulerHistory = window.FloeRulers.history();
    let ackedFrames = {foreground: null, margin: null};
    const sessionKey = 'floe-session:' + location.origin;
    let auth = null, stopped = false, socket = null, epoch = '', state = null;
    let seq = '0', queue = [], inflight = null, accepted = null, lastSend = 0;
    const editCallbacks = new WeakMap();
    let socketSerial = 0, decode = null, reconnectTimer = null, reconnectDelay = 500;
    let catalog = [], currentId = '', currentSource = '', currentMode = 'level', ownerBusy = false, submitting = false;
    let modeReceipt = '', modeSupported = false;
    let pendingStartup = null, startupWaiting = false;
    let gotoDirty = false, gotoRevision = 0, gotoView = '';
    const gotoFields = ['goto-x','goto-y','goto-width'];
    let selectedStyle = null;
    let levelNext = null, levelSource = '', levelIds = new Set(), levelLoad = 0, levelBusy = false;
    let lastDigit = '', lastDigitAt = 0;
    let operationTimer = null, resizeTimer = null, displayed = false;
    const errors = {
        index_unavailable: 'A current index is required. Review “Index and open…” or use “Index this source”; opening never indexes automatically.',
        busy: 'Resources or cache are busy. Wait, choose fewer index jobs, or explicitly close a reader before rebuilding its cache.',
        worker_version: 'The native renderer version does not match. Rebuild the matched binaries.',
        worker_failed: 'The renderer failed. Check the local service diagnostics; close and reopen to retry.',
        stale_state: 'The view changed in another connection. That edit was not replayed.',
        prepared_edit_expired: 'This prepared move is no longer current. Select the error again; it was not replayed.',
        prepared_edit_unavailable: 'The move could not be prepared. Try again.',
        prepared_edit_limit: 'This view cannot prepare another move. Close and reopen it.',
        invalid_request: 'The requested value or selection is not supported.',
        invalid_palette: 'The layer page or group is no longer valid. Reload the layer list.',
        palette_anchor_hidden: 'The range anchor is hidden by a folded group. Select its visible parent first.',
        palette_range_too_large: 'Select at most 4096 layer rows. The previous selection was preserved.',
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
        drc_selection_conflict: 'Selection changed in another request. Server state will be reloaded; no command is retried.',
        drc_selection_limit: 'Selection limit reached. The previous selection was preserved.',
        invalid_drc_request: 'Invalid DRC index, cursor, coordinate or page limit.'
    };
    function notice(text) { el('notice').textContent = text || ''; el('notice').hidden = !text; }
    function message(error) { return errors[error] || String(error || 'Request failed'); }
    function report(error) { notice(message(error.message || error)); }
    function http(method, path, body, missing, token, upload) {
        return new Promise(function (resolve, reject) {
            const xhr = new XMLHttpRequest();
            if (token && token.cancelled) { reject(new Error('Request cancelled')); return; }
            if (token) { token.abort = function () { xhr.abort(); }; }
            function done() { if (token) { token.abort = null; } }
            xhr.open(method, path); xhr.timeout = 8000;
            if (auth) { xhr.setRequestHeader('X-Floe-CSRF', auth.csrf); }
            if (upload) {Object.keys(upload.headers).forEach(function (k) {xhr.setRequestHeader(k, upload.headers[k]);});}
            else if (body !== undefined) { xhr.setRequestHeader('Content-Type', 'application/json'); }
            xhr.onload = function () {
                done();
                if (missing && xhr.status === 404) { resolve(null); return; }
                let value = null;
                try {
                    if (xhr.responseText.length > 1024 * 1024) { throw new Error('Reply limit'); }
                    if (xhr.responseText) { value = JSON.parse(xhr.responseText); }
                } catch (e) { reject(e); return; }
                if (xhr.status < 200 || xhr.status >= 300) {
                    if (xhr.status === 401) { stopped = true; if (indexOpen) { indexOpen.stop(); } if (picker) { picker.stop(); } if (launcher) { launcher.stop(); } connection('Session expired', false); }
                    const failure = new Error(message(value && value.error || ('HTTP ' + xhr.status)));
                    failure.status = xhr.status; failure.code = value && value.error;
                    reject(failure);
                } else { resolve(value); }
            };
            xhr.onerror = function () { done(); reject(new Error('Local service is unavailable')); };
            xhr.onabort = function () { done(); reject(new Error('Request cancelled')); };
            xhr.ontimeout = function () { done(); reject(new Error('Request timed out; its outcome may be pending. Check operation status before retrying.')); };
            xhr.send(upload ? upload.blob : body === undefined ? null : JSON.stringify(body));
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
        if (inspector) { inspector.changed(); inspector.paint(displayProjection, size); }
        if (measurement) { measurement.changed(); measurement.paint(displayProjection, size); }
        if (clipper) { clipper.changed(); }
        if (snapshots) { snapshots.changed(); }
        if (minimap) { minimap.changed(); }
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
            // A no-op/rejected edit produces no replacement frame. If the
            // pixels were not copied/recomposed, their original receipt is
            // still valid; pending/revision checks already prevent queries
            // during a real state change.
            if (copy) { foregroundFrame = null; }
            canvas.hidden = false; positionCanvas(dims());
            lastPlacement = {pixels: p.pixels, margin: null, full: false, foreground: [0, 0]};
        }
        marginCanvas.hidden = true;
    }
    function clearBuffers() {
        foregroundFrame = null; marginFrame = null; foregroundPerf = '';
        lastPlacement = null; dragShift = null;
        displayProjection = null; frozenProjection = null;
        canvas.hidden = false; canvas.width = 1; canvas.height = 1;
        marginCanvas.hidden = true; marginCanvas.width = 1; marginCanvas.height = 1;
        ackedFrames = {foreground: null, margin: null};
    }
    function live() { return state && !['closed', 'failed'].includes(state.status); }
    function indexBlocked() { return !!indexOpen && indexOpen.blocked(); }
    function deckModeReady() {
        return modeSupported && live() && state.capabilities.mode &&
            ['idle','rendering'].includes(state.status) && socket && socket.readyState === WebSocket.OPEN && epoch &&
            !indexBlocked() && !submitting && !ownerBusy && !inflight && !accepted && !queue.length && !(gesture && gesture.active());
    }
    function controls() {
        const enabled = live() && socket && socket.readyState === WebSocket.OPEN && !!epoch && !submitting && !ownerBusy && !indexBlocked();
        ['fit', 'zoom-in', 'zoom-out', 'goto', 'depth', 'detail', 'thin', 'frames', 'labels', 'mono', 'layers-all', 'layers-none'].forEach(function (id) { el(id).disabled = !enabled; });
        el('overlays').disabled=!live();
        el('labels').disabled = !enabled || !state.capabilities.labels;
        el('font-px').disabled = !enabled || !state.capabilities.labels;
        const launchPending = launcher && launcher.blocked(), source = catalog.find(function (s) { return s.source_id === el('source').value; });
        el('open').disabled = submitting || ownerBusy || !!live() || !source || launchPending || indexBlocked();
        el('source').disabled = !catalog.length || launchPending || ownerBusy || submitting || indexBlocked();
        el('mode').disabled = !source || !source.deck || launchPending || ownerBusy || submitting || indexBlocked();
        el('close').disabled = !currentId || submitting || ownerBusy || indexBlocked();
        el('index').disabled = submitting || ownerBusy || !source || indexBlocked();
        el('cancel-job').disabled = !ownerBusy || indexBlocked();
        el('levels-all').disabled = indexBlocked();
        el('level-more').disabled = indexBlocked() || levelBusy || levelNext === null;
        Array.from(el('level-list').querySelectorAll('input')).forEach(function (box) { box.disabled = indexBlocked() || el('levels-all').checked; });
        el('live-mode-row').hidden = !modeSupported || !state || !state.capabilities.mode;
        el('live-mode').disabled = !deckModeReady();
        el('live-mode').value = currentMode;
        if (settings) { settings.changed(); }
        if (defaults) { defaults.changed(); }
        if (drcPanel) { drcPanel.contextChanged(); }
        if (inspector) { inspector.changed(); }
        if (measurement) { measurement.changed(); }
        if (clipper) { clipper.changed(); }
        if (snapshots) { snapshots.changed(); }
        if (launcher) { launcher.changed(); }
        if (picker) { picker.changed(); }
        if (indexOpen) { indexOpen.changed(); }
        if (palette) { palette.changed(); }
    }
    function queryContext() {
        if (!state || !currentId || !lastPlacement) { return null; }
        let h = lastPlacement.full ? marginFrame : foregroundFrame;
        let origin = lastPlacement.full ? lastPlacement.margin : lastPlacement.foreground;
        // A label-truncated margin still supplies complete geometry beneath
        // the old label frame, including the newly exposed strip.
        if ((!h || !P.matches(h, state)) && marginFrame && P.matches(marginFrame, state)) { h = marginFrame; origin = lastPlacement.margin; }
        let size; try { size = dims(); } catch (e) { return null; }
        return {id: currentId, state: state, frame: h, origin: origin, size: size, rect: viewport.getBoundingClientRect(),
            acked: !!h && ackedFrames[h.purpose] === h.frame_id,
            connected: !stopped && !!epoch && !!socket && socket.readyState === WebSocket.OPEN,
            hidden: document.hidden, pending: indexBlocked() || !!inflight || queue.length > 0 || !!dragShift || !!accepted ||
                !!(gesture && gesture.active()) || !!(drcPanel && drcPanel.boxActive())};
    }
    function highlightPicked(pairs) {
        pickedPairs = pairs;
        Array.from(el('layers').querySelectorAll('span')).forEach(function (name) {
            if (name.dataset.pair) { name.dataset.picked = String(pairs.some(function (p) { return p.join('/') === name.dataset.pair; })); }
        });
    }
    function send(value) {
        if (!socket || socket.readyState !== WebSocket.OPEN) { throw new Error('View is disconnected'); }
        seq = P.next(seq); value.seq = seq;
        const text = JSON.stringify(value);
        if (new TextEncoder().encode(text).length > 8192) {
            throw new Error('Input is too large. Select fewer layers or use a smaller edit; nothing was applied.');
        }
        if (socket.bufferedAmount > 16384) {
            throw new Error('Input limit; wait for the local connection');
        }
        socket.send(text); return seq;
    }
    function settleEdit(body, error) {
        if (!body) { return; }
        const done = editCallbacks.get(body); editCallbacks.delete(body);
        if (done) { try { done(error || null); } catch (e) { report(e); } }
    }
    function rejectQueued(error) {
        const old = queue; queue = []; old.forEach(function (body) { settleEdit(body, error); });
    }
    function rejectEdits(error) {
        const body = inflightBody; inflight = null; inflightBody = null; accepted = null;
        rejectQueued(error); settleEdit(body, error);
    }
    function pump() {
        if (!live() || !epoch || inflight || !queue.length || !socket || socket.readyState !== WebSocket.OPEN || ownerBusy || submitting || indexBlocked()) { return; }
        const wait = 65 - (Date.now() - lastSend);
        if (wait > 0) { window.setTimeout(pump, wait); return; }
        try {
            const body = queue.shift();
            inflightBody = body;
            const wire = {type: body.prepared_token ? 'view.apply' : 'view.set', connection_epoch: epoch, view_id: currentId, base_state_rev: state.state_rev};
            if (body.prepared_token) { wire.token = body.prepared_token; } else { wire.body = body; }
            inflight = send(wire);
            lastSend = Date.now();
        } catch (e) { rejectEdits(e.message); report(e); }
    }
    function edit(body, done) {
        if (done) { editCallbacks.set(body, done); }
        const error = ownerBusy || submitting || indexBlocked() ? 'An owner operation or approval is pending. This input was not applied.' : !live() || !epoch ? 'Open a connected view first.' : queue.length >= 64 ? 'Input queue is full. This input was not applied.' : null;
        if (error) { notice(error); settleEdit(body, error); return null; }
        if (!body.navigation || body.navigation.kind !== 'pan' || !body.navigation.snap) { freezeMargin(); }
        queue.push(body); pump(); present();
        return function () {
            const i = queue.indexOf(body);
            if (i < 0) { return false; } // A sent edit may already be committed.
            queue.splice(i, 1); settleEdit(body, 'Request cancelled'); controls(); present(); return true;
        };
    }
    function syncGoto(force) {
        if (!state || !currentId || (launcher && launcher.blocked()) ||
            (pendingStartup && pendingStartup.source_id !== currentSource)) { return; }
        if (gotoView !== currentId) { gotoView = currentId; gotoDirty = false; ++gotoRevision; force = true; }
        if (!force && (gotoDirty || gotoFields.some(function(k){return document.activeElement === el(k);}))) { return; }
        const camera = state.camera_um;
        if (camera !== null && (!Array.isArray(camera) || camera.length !== 3 || Number(P.decimal(camera[2])) <= 0)) { throw Error('Invalid camera coordinates'); }
        const values = camera === null ? ['','',''] : camera.map(P.decimal);
        gotoFields.forEach(function(k,i){el(k).value = values[i];});
    }
    function statusSnapshot(s) {
        if (s.view_id !== currentId || s.connection_epoch !== epoch) { return; }
        if (state && state.connection_epoch === epoch && P.compare(s.state_rev, state.state_rev) < 0) { return; }
        if (gesture && gesture.active() && state && (s.state_rev !== state.state_rev || s.connection_epoch !== state.connection_epoch)) { gesture.cancel(); }
        if (marginFrame && !P.placement(marginFrame, s)) {
            freezeMargin(); marginFrame = null; marginCanvas.width = 1; marginCanvas.height = 1;
        }
        state = s;
        if (accepted && P.compare(s.state_rev, accepted.rev) >= 0) {
            const body = inflightBody, error = accepted.error || (s.state_rev !== accepted.rev ?
                'The view changed again after that edit; dependent review effects were not applied. The current view is authoritative.' : null);
            accepted = null; inflight = null; inflightBody = null;
            // Apply dependent UI only after the authoritative snapshot, but
            // before live viewport filters observe the new location.
            settleEdit(body, error);
        }
        controls();
        el('rendering').hidden = !['opening', 'rendering', 'cancelling'].includes(s.status);
        el('rendering').textContent = s.status === 'opening' ? 'Opening index' : 'Rendering';
        if (!displayed) { el('empty-message').textContent = s.status === 'failed' ? message(s.failure) : 'Preparing the first frame…'; }
        if (s.failure) { notice(message(s.failure)); }
        if (document.activeElement !== el('depth')) { el('depth').value = s.depth; }
        el('max-depth').textContent = s.max_depth === null ? '' : '/ ' + s.max_depth;
        ['detail', 'thin'].forEach(function (id) { el(id).value = s[id]; });
        ['frames', 'labels', 'mono'].forEach(function (id) { el(id).checked = s[id]; });
        if (document.activeElement !== el('font-px')) { el('font-px').value = s.font_px; }
        syncGoto(false);
        const b = s.bbox_dbu.map(Number), dbu = Number(s.dbu_um);
        el('viewport-info').textContent = ((b[2] - b[0]) * dbu).toPrecision(6) + ' × ' + ((b[3] - b[1]) * dbu).toPrecision(6) + ' µm';
        el('status').textContent = s.status + ' · depth ' + s.depth + ' · thin:' + s.effective_thin + (s.source_stale ? ' · SOURCE STALE' : '');
        pump();
        present();
    }
    function finishDecode() { if (decode) { decode(); decode = null; } }
    function acknowledge(h, disposition, ws, serial) {
        if (ws !== socket || serial !== socketSerial || ws.readyState !== WebSocket.OPEN) { return; }
        try {
            send({type: 'frame.ack', connection_epoch: h.connection_epoch, frame_id: h.frame_id, disposition: disposition});
            if (disposition === 'displayed') { ackedFrames[h.purpose] = h.frame_id; }
            if (inspector) { inspector.changed(); }
            if (clipper) { clipper.changed(); }
        }
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
        if (inspector) { inspector.interrupt(); }
        ackedFrames = {foreground: null, margin: null};
        if (gesture) { gesture.cancel(); }
        freezeMargin();
        ++socketSerial; finishDecode();
        if (socket) { socket.onclose = null; socket.close(); socket = null; }
        epoch = ''; rejectEdits('Connection interrupted; pending input was not replayed.');
        if (settings) { settings.changed(); }
        if (inspector) { inspector.changed(); }
        if (clipper) { clipper.changed(); }
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
                else if (clipper && clipper.receive(m)) { /* current clip preparation owns this response */ }
                else if (measurement && measurement.receive(m)) { /* ruler owns its current response */ }
                else if (inspector && inspector.receive(m)) { /* latest query owns its response */ }
                else if (m.type === 'accepted') {
                    if (m.seq !== inflight) { throw new Error('Unexpected edit acknowledgement'); }
                    accepted = {rev: m.state_rev, render: m.render_rev};
                } else if (m.type === 'error') {
                    if (m.seq === inflight) {
                        rejectQueued(message(m.code)); accepted = {rev: '0', render: state.render_rev, error: message(m.code)};
                        notice(message(m.code));
                    } // An old query's refusal cannot replace current UI state.
                }
            } catch (e) { report(e); ws.close(); }
        };
        ws.onclose = function () {
            if (socket !== ws || serial !== socketSerial) { return; }
            if (gesture) { gesture.cancel(); }
            const uncertain = !!inflight || queue.length > 0;
            freezeMargin(); epoch = ''; socket = null; finishDecode(); rejectEdits('Connection interrupted; pending input was not replayed.');
            ackedFrames = {foreground: null, margin: null};
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
        if (changed) { displayed = false; clearBuffers(); el('empty').hidden = false; selectedStyle = null; el('style-editor').hidden = true; }
        currentId = current.view.view_id; currentSource = current.source_id; currentMode = current.mode; state = current.view;
        el('document-title').textContent = current.title; document.title = current.title + ' · floe2';
        // Reconnection restores the live view, not a different pending CLI
        // proposal's source/level form. Its explicit selection must survive.
        if (!launcher || !launcher.blocked()) {
            if (pendingStartup && pendingStartup.source_id === currentSource) { pendingStartup = null; }
            el('source').value = currentSource; el('mode').value = current.mode;
            sourceSelection();
            levelIds = new Set(current.levels || []); el('levels-all').checked = current.levels === null;
            Array.from(el('level-list').querySelectorAll('input')).forEach(function (box) {
                box.checked = levelIds.has(box.value); box.disabled = el('levels-all').checked;
            });
        }
        syncGoto(false); controls(); connect();
    }
    function paletteStyle(r, scope, valid) {
        const color = document.createElement('input'); color.type = 'color'; color.value = r.color; color.setAttribute('aria-label', 'Color ' + r.name);
        color.onchange = function () {
            const value=color.value; color.value=r.color;
            if (!valid()) { notice('Layer styles changed or an input is pending. Select the layer again.'); return; }
            edit({style_batch: {pairs:[r.pair],collapsed:r.closed?[r.pair]:[],color:value}});
        };
        const style = document.createElement('button'); style.className = 'layer-edit'; style.textContent = '⋯';
        style.setAttribute('aria-label', 'Edit style ' + r.name);
        style.onclick = function () {
            if (!valid()) { return; }
            palette.closeStyle();
            selectedStyle = {row: r, key: scope.key, view: scope.id, valid:valid};
            el('style-title').textContent = r.name; el('style-fill').value = r.fill.kind; el('style-width').value = r.width;
            el('style-pattern').value = (r.fill.rows || new Array(16).fill(0xaaaa)).map(function (n) { return n.toString(16).padStart(4, '0'); }).join(' ');
            el('style-editor').hidden = false; patternControls(); el('style-fill').focus();
        };
        return {color:color, style:style};
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
        if (!source) { el('source-note').textContent = 'No source registered in this workspace.'; el('level-options').hidden = true; controls(); return; }
        if (pendingStartup && pendingStartup.source_id !== source.source_id) { pendingStartup = null; startupWaiting = false; }
        el('mode').disabled = !source.deck; el('level-options').hidden = !source.deck;
        if (!source.deck) { el('mode').value = 'level'; }
        if (levelSource !== source.source_id) {
            ++levelLoad; levelBusy = false;
            levelSource = source.source_id; levelNext = null; levelIds = new Set(); el('level-list').textContent = ''; el('levels-all').checked = true;
            if (source.deck) { moreLevels().catch(report); }
        }
        el('source-note').textContent = source.deck ? 'Jobdeck · select levels before opening.' : 'OASIS layout · current index required.';
        controls();
    }
    async function refreshCatalog() {
        const selected = el('source').value;
        catalog = (await http('GET', '/api/v1/catalog')).sources;
        el('source').textContent = '';
        catalog.forEach(function (s) { const option = document.createElement('option'); option.value = s.source_id; option.textContent = s.title; el('source').appendChild(option); });
        el('source').value = catalog.some(function (s) { return s.source_id === selected; }) ? selected : catalog.length ? catalog[0].source_id : '';
    }
    function levels() {
        if (el('levels-all').checked) { return {mode: 'all'}; }
        if (!levelIds.size) { throw new Error('Select at least one level.'); }
        return {mode: 'only', ids: Array.from(levelIds)};
    }
    function operationLabel(op) {
        if (op.kind === 'index_open') { return window.FloeIndexOpen.resultText(op,message); }
        const p = op.native || {};
        return op.kind + ' · ' + op.phase + (p.phase ? ' · ' + p.phase : '') + (op.error ? ' · ' + message(op.error) : '');
    }
    async function operationState() {
        try { return await readOperationState(); }
        catch (e) {
            // Read-only reconciliation after an uncertain mutation response;
            // never re-submit the mutation automatically.
            if (!stopped && !document.hidden) {
                clearTimeout(operationTimer);
                operationTimer = setTimeout(function () { operationState().catch(report); }, 1000);
            }
            throw e;
        }
    }
    async function readOperationState() {
        const all = await http('GET', '/api/v1/operations');
        if (indexOpen) { indexOpen.observe(all); }
        ownerBusy = all.active !== null; el('cancel-job').disabled = !ownerBusy;
        el('cancel-job').dataset.seq = all.active || ''; controls();
        const recent = all.history || [], last = recent[recent.length - 1];
        if (last) { el('operation').textContent = operationLabel(last); }
        el('live-mode-note').textContent = ownerBusy && last && last.kind === 'mode' ? 'Changing jobdeck mode…' : 'Ctrl+, toggles level/chip · same camera and loaded levels. Mode defaults reload.';
        if (!ownerBusy && last && ['failed', 'incomplete', 'cancelled'].includes(last.phase)) {
            notice(message(last.error || last.phase));
            if (!currentId) { el('empty-message').textContent = message(last.error || last.phase); connection('Local · ready', true); }
        }
        if (ownerBusy) { operationTimer = setTimeout(function () { operationState().catch(report); }, 500); }
        else if (last && (last.kind === 'open' || last.kind === 'index_open' && !indexOpen.pending()) && last.phase === 'succeeded') {
            if (last.view_id !== currentId) { await restore(); }
            else if (pendingStartup && pendingStartup.source_id === currentSource) { pendingStartup = null; }
        }
        else if (last && last.kind === 'mode' && last.seq !== modeReceipt) {
            await restore(); modeReceipt = last.seq;
            if (last.phase === 'succeeded') { notice(''); }
        }
        if (!ownerBusy && !submitting) { pump(); }
        return all;
    }
    async function submitOperation(request) {
        if (ownerBusy || submitting || indexBlocked()) { throw new Error('An operation or approval is already pending.'); }
        submitting = true; controls();
        if (operationTimer) { clearTimeout(operationTimer); operationTimer = null; }
        try {
            const all = await operationState();
            if (all.active !== null) { throw new Error('An operation is already running.'); }
            request.seq = P.next(all.last_seq); ownerBusy = true; controls(); notice('');
            try { await http('POST', '/api/v1/operations', request); }
            finally { await operationState(); }
        } finally { submitting = false; controls(); pump(); }
    }
    async function changeDeckMode(mode) {
        if (!['level','chip','layer'].includes(mode)) { throw new Error('Invalid jobdeck mode.'); }
        if (!deckModeReady()) { el('live-mode').value = currentMode; throw new Error('Wait for the current view and pending inputs before changing jobdeck mode.'); }
        if (mode === currentMode) { return; }
        await submitOperation({kind:'mode', view_id:currentId, base_state_rev:state.state_rev, mode:mode});
    }
    el('live-mode').onchange = function () { changeDeckMode(el('live-mode').value).catch(report); };
    async function openSource(startup) {
        const remembered = pendingStartup && pendingStartup.source_id === el('source').value ? pendingStartup : null;
        const request = Object.assign({}, startup || {kind: 'open', mode: el('mode').value, source_id: el('source').value, levels: levels(),
            display_policy: remembered ? remembered.display_policy || 'explicit' : 'window', body: remembered ? remembered.body : {}});
        if (!startup && remembered && remembered.label_preference !== undefined) { request.label_preference = remembered.label_preference; }
        request.body = Object.assign({}, request.body, {pixels:dims().pixels});
        await submitOperation(request);
        startupWaiting = false;
    }
    function prepareStartup(request) {
        el('source').value = request.source_id; sourceSelection();
        pendingStartup = request;
        el('mode').value = request.mode;
        el('levels-all').checked = !request.levels || request.levels.mode === 'all';
        levelIds = new Set(request.levels && request.levels.ids || []);
        Array.from(el('level-list').querySelectorAll('input')).forEach(function (box) {
            box.checked = levelIds.has(box.value); box.disabled = el('levels-all').checked;
        });
        const n = request.body.navigation;
        if (n && n.kind === 'goto') {
            el('goto-x').value = n.center_um[0]; el('goto-y').value = n.center_um[1];
            if (n.width_um !== undefined) { el('goto-width').value = n.width_um; }
        }
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
        about.init(); sessionExit.init();
        modeSupported = caps.jobdeck_modes === true;
        await refreshCatalog();
        if (caps.drc) { await drcPanel.init(); }
        await clipper.init(caps.exports);
        snapshots.init(caps.snapshot_png);
        settings.capabilities(caps.layer_settings);
        await defaults.init(caps.design_defaults);
        sourceSelection();
        await indexOpen.init(caps.index_open);
        const operations = await operationState();
        await restore();
        if (!currentId && operations.last_seq === '0') {
            const preferences = await http('GET', '/api/v1/startup'), startup = preferences.request;
            if (startup) {
                prepareStartup(startup);
                if (preferences.confirm_levels) {
                    startupWaiting = true;
                    el('level-options').open = true;
                    el('empty-message').textContent = 'Choose jobdeck levels, then Open layout. No indexing or rendering has started.';
                    connection('Local · choose levels', true); el('open').focus();
                } else { await openSource(startup); }
            }
            else { el('empty-message').textContent = catalog.length ? 'Choose a registered source and open its index.' : caps.file_picker ? 'Choose a server file with Browse server files. Indexing requires separate approval.' : caps.launcher ? 'Workspace ready. Run floe2-web view FILE to open a layout here.' : 'No registered sources. Restart this independent workspace with FILE.'; connection('Local · ready', true); }
        }
        await picker.init(caps.file_picker, !catalog.length);
        await launcher.init(caps.launcher || caps.file_picker);
    }
    el('source').onchange = sourceSelection;
    el('open').onclick = function () { openSource(null).catch(report); };
    el('close').onclick = async function () {
        if (!currentId) { return; }
        try { await http('DELETE', '/api/v1/views/' + currentId); disconnect(); state = null; currentId = ''; controls(); displayed = false; clearBuffers(); el('empty').hidden = false; el('empty-message').textContent = 'View closed. Choose a source to reopen.'; el('rendering').hidden = true; el('layers').textContent = ''; }
        catch (e) { report(e); }
    };
    async function endSession() {
        palette.stop();
        if (indexOpen) { indexOpen.stop(); }
        if (launcher) { launcher.stop(); }
        if (picker) { picker.stop(); }
        if (minimap) { minimap.stop(); }
        about.stop();
        if (drcPanel) { drcPanel.stop(); }
        if (clipper) { clipper.stop(); }
        if (snapshots) { snapshots.stop(); }
        if (settings) { settings.stop(); }
        if (defaults) { defaults.stop(); }
        stopped = true; disconnect(); if (operationTimer) { clearTimeout(operationTimer); }
        let failure=null;
        try { await http('DELETE', '/api/v1/session'); } catch (e) { failure=e; }
        state = null; currentId = ''; controls();
        if(failure){
            connection('Server shutdown unconfirmed', false);
            notice('Local view stopped; server shutdown is unconfirmed. Check the local launcher before starting another session. Earlier approved writes may have completed; their recovery records are retained. No automatic retry. '+message(failure.message));
            return;
        }
        if (drcPanel) { drcPanel.stop(true); }
        if (defaults) { defaults.stop(true); }
        if (indexOpen) { try { indexOpen.clear(); } catch (_) { /* Confirmed ended session cannot accept this record. */ } }
        try { sessionStorage.removeItem(sessionKey); } catch (_) { /* storage may be disabled */ }
        connection('Session ended', false); notice('Session ended. Close this tab.');
    }
    el('levels-all').onchange = function () { Array.from(el('level-list').querySelectorAll('input')).forEach(function (box) { box.disabled = el('levels-all').checked; }); };
    el('level-more').onclick = function () { moreLevels().catch(report); };
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
        if (!selectedStyle || !state || selectedStyle.view !== currentId || selectedStyle.key !== state.render_key || !selectedStyle.valid()) { notice('Layer styles changed. Select the layer again before applying.'); return; }
        const fill = {kind: el('style-fill').value}, width = Number(el('style-width').value);
        if (!Number.isInteger(width) || width < 1 || width > 8) { notice('Line width must be 1–8 device pixels.'); return; }
        if (fill.kind === 'pattern') {
            const rows = el('style-pattern').value.trim().split(/\s+/);
            if (rows.length !== 16 || !rows.every(function (s) { return /^[0-9a-f]{4}$/i.test(s); })) { notice('A pattern needs exactly 16 four-digit hex rows.'); return; }
            fill.rows = rows.map(function (s) { return parseInt(s, 16); });
        }
        const row = selectedStyle.row, delta = {pairs:[row.pair],collapsed:row.closed?[row.pair]:[]};
        if (fill.kind !== row.fill.kind || (fill.kind === 'pattern' && fill.rows.some(function (n, i) { return n !== row.fill.rows[i]; }))) { delta.fill = fill; }
        if (width !== row.width) { delta.width = width; }
        if (delta.fill || delta.width !== undefined) { edit({style_batch: delta}); }
        selectedStyle = null; el('style-editor').hidden = true;
    };
    const nav = function (n) { edit({navigation: n}); };
    const zoom = function (factor, anchor) { nav({kind: 'zoom', factor: factor, anchor: anchor || [0.5, 0.5]}); };
    el('fit').onclick = function () { nav({kind: 'fit'}); };
    el('zoom-in').onclick = function () { zoom(0.8); };
    el('zoom-out').onclick = function () { zoom(1.25); };
    el('goto-form').onsubmit = function (event) {
        event.preventDefault();
        try {
            const values = gotoFields.map(function(k){return P.decimal(el(k).value);}), rev = gotoRevision, id = currentId;
            gotoDirty = true;
            edit({navigation:{kind:'goto',center_um:values.slice(0,2),width_um:values[2]}},function(error){
                if (!error && id === currentId && rev === gotoRevision) { gotoDirty = false; syncGoto(true); }
            });
        }
        catch (e) { report(e); }
    };
    gotoFields.forEach(function(k){
        el(k).addEventListener('input',function(){gotoDirty = true; ++gotoRevision;});
        el(k).addEventListener('keydown',function(e){
            if (e.key === 'Escape' && !e.isComposing && !e.ctrlKey && !e.metaKey && !e.altKey) {
                e.preventDefault(); gotoDirty = false; ++gotoRevision; syncGoto(true);
            }
        });
    });
    function overlays(mode) {
        if(!['all','focus','none'].includes(mode)){return;}
        overlayMode=mode;el('overlays').value=mode;
        inspector.showOverlay(mode!=='none');measurement.showOverlay(mode!=='none');drcPanel.overlayMode(mode);
    }
    el('overlays').onchange=function(){overlays(el('overlays').value);};
    viewport.addEventListener('mousedown', function () { viewport.focus(); });
    viewport.addEventListener('keydown', function (event) {
        if (!live()) { return; }
        if(event.target===viewport&&!event.altKey&&!event.shiftKey&&!event.isComposing&&(event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='c') {
            const selected=typeof window.getSelection==='function'&&window.getSelection();
            if(!selected||selected.isCollapsed){event.preventDefault();snapshots.request('copy');}return;
        }
        if (gesture && gesture.active()) { if (event.key === 'Escape') { event.preventDefault(); gesture.cancel(); } return; }
        if (event.metaKey || event.altKey || event.isComposing) { return; }
        let key = event.key;
        if (key.length === 1 && key.charCodeAt(0) > 127 && /^Key[A-Z]$/.test(event.code || '')) { key = event.shiftKey ? event.code.slice(3) : event.code.slice(3).toLowerCase(); }
        if (event.ctrlKey) {
            if (key.toLowerCase() === 'a') { event.preventDefault(); nav({kind: 'fit'}); }
            else if (key.toLowerCase() === 'z') { event.preventDefault(); zoom(0.5); }
            else if (key === '.') { event.preventDefault(); el('goto-x').focus(); el('goto-x').select(); }
            else if (key === ',' && !event.shiftKey && !event.repeat && event.target === viewport && modeSupported && state.capabilities.mode) {
                event.preventDefault(); changeDeckMode(currentMode === 'level' ? 'chip' : 'level').catch(report);
            }
            return;
        }
        if(key==='Tab'&&!event.shiftKey&&event.target===viewport){event.preventDefault();const modes=['all','focus','none'];overlays(modes[(modes.indexOf(overlayMode)+1)%3]);return;}
        if(key==='q'&&!event.shiftKey&&event.target===viewport){event.preventDefault();sessionExit.open();return;}
        if (key === 'r' && drcPanel && drcPanel.boxActive()) { drcPanel.key('e'); }
        if (measurement && !(drcPanel && drcPanel.boxActive()) && measurement.key(key)) {
            event.preventDefault(); return;
        }
        if (drcPanel && drcPanel.key(key)) { event.preventDefault(); return; }
        if (inspector && inspector.key(key)) { event.preventDefault(); return; }
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
        else if (key === '<' || key === '>') { event.preventDefault(); edit({depth_step:key === '<' ? -1 : 1}); }
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
    function reviewCursor() {
        if (measurement && measurement.active() && drcPanel && drcPanel.boxActive()) { measurement.leave(); }
        viewport.style.cursor = gesture && gesture.bandActive() ? 'crosshair' : gesture && gesture.active() ? 'grabbing' : (drcPanel && drcPanel.boxActive()) || (measurement && measurement.active()) ? 'crosshair' : '';
        if (inspector && (!gesture || !gesture.active())) { inspector.changed(); }
    }
    viewport.addEventListener('mousemove', function (event) {
        if (!gesture || !gesture.active()) {
            if (drcPanel) { drcPanel.move(event.clientX, event.clientY); }
            if (measurement && measurement.active()) { measurement.move(event.clientX, event.clientY, event); }
            else if (inspector) { inspector.move(event.clientX, event.clientY); }
        }
    });
    viewport.addEventListener('mouseleave', function () { if (drcPanel) { drcPanel.move(NaN, NaN); } if (inspector) { inspector.move(NaN, NaN); } if (measurement) { measurement.move(NaN, NaN); } });
    gesture = window.FloeGestures.bind({viewport: viewport, window: window, document: document,
        dimensions: dims, ready: function () { return !indexBlocked() && live() && displayed && !!epoch && !inflight && queue.length === 0; },
        stamp: function () { return currentId + ':' + epoch + ':' + (state ? state.state_rev : ''); },
        requestAnimationFrame: function (fn) { return window.requestAnimationFrame(fn); },
        cancelAnimationFrame: function (id) { window.cancelAnimationFrame(id); },
        preview: function (p, paint) { dragShift = p; if (paint) { present(); } },
        band:nav,notice:notice,
        bandReady:function(){
            if(!state||!lastPlacement||document.hidden){return false;}
            const d=dims();if(d.pixels[0]!==state.pixels[0]||d.pixels[1]!==state.pixels[1]){return false;}
            const h=lastPlacement.full?marginFrame:foregroundFrame;
            return !!h&&P.matches(h,state);
        },
        bandPreview:function(b){
            const box=el('zoom-band'),hint=el('zoom-band-hint');box.hidden=hint.hidden=!b;if(!b){return;}
            const d=b.dimensions,x=b.start[0]*d.pixels[0],y=b.start[1]*d.pixels[1],ex=b.end[0]*d.pixels[0],ey=b.end[1]*d.pixels[1];
            box.style.left=((d.left||0)+Math.round(Math.min(x,ex))/d.dpr)+'px';box.style.top=((d.top||0)+Math.round(Math.min(y,ey))/d.dpr)+'px';
            box.style.width=(Math.max(1,Math.round(Math.abs(ex-x)))/d.dpr)+'px';box.style.height=(Math.max(1,Math.round(Math.abs(ey-y)))/d.dpr)+'px';
            box.style.borderWidth=(1/d.dpr)+'px';hint.textContent=(b.outward?'Zoom out':'Zoom in')+' · release to apply · Esc cancels';
        },
        cursor: reviewCursor, pan: nav, objectClicks: true, selectionMode: function () { return !!drcPanel && drcPanel.boxActive(); },
        click: function (x, y, twice, modifiers) {
            if (measurement && measurement.active()) { measurement.click(x, y, modifiers); return; }
            const m = modifiers || {};
            if (drcPanel && (drcPanel.boxActive() || (!m.ctrlKey && !m.metaKey && !m.shiftKey)) && drcPanel.click(x, y, twice, modifiers)) {
                if (inspector) { inspector.interrupt(); } return;
            }
            if (inspector) { inspector.click(x, y, modifiers); }
        }});
    el('index').onclick = function () {
        try { submitOperation({kind: 'index', source_id: el('source').value, levels: levels(), options: {jobs: Number(el('index-jobs').value), force: el('index-force').checked, lod: el('index-lod').checked, occupancy: el('index-occupancy').checked}}).catch(report); }
        catch (e) { report(e); }
    };
    el('cancel-job').onclick = function () { const id = el('cancel-job').dataset.seq; if (id && !indexBlocked()) { http('POST', '/api/v1/operations/' + id + '/cancel', {}).then(operationState).catch(report); } };
    function resized() {
        if (gesture && gesture.active()) { gesture.cancel(); }
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
        history:rulerHistory, rulerKey:function (key) { return measurement && !drcPanel.boxActive() && measurement.key(key); },
        stateStore: window.FloePanelState, rulers: window.FloeRulers, groups: window.FloeDRCGroups, builds: window.FloeDRCBuild, cursor: reviewCursor,
        notes: window.FloeDRCNotes, noteDisplay: window.FloeDRCNoteDisplay, waives: window.FloeDRCWaives, transfers: window.FloeDRCTransfer, session: function () { return auth ? auth.session_id : ''; },
        transferChunk: function (kind, request, offset, blob, token) {
            if (!['notes','waives'].includes(kind)||!blob||blob.size<1||blob.size>1048576) {return Promise.reject(new Error('Invalid review chunk'));}
            return http('POST','/api/v1/drc/review/'+kind+'/transfer/chunk',undefined,false,token,{blob:blob,headers:{
                'Content-Type':'application/octet-stream','X-Floe-Transfer-Token':request.token,'X-Floe-Transfer-Seq':request.seq,
                'X-Floe-Transfer-Offset':String(offset),'X-Floe-DRC':request.context.drc_id,'X-Floe-Revision':request.context.revision,'X-Floe-View':request.context.view_id}});
        },
        transferDownload: function (kind, id) {if(stopped||!auth){throw new Error('The owner session is closed.');}window.FloeDRCTransfer.download(document,auth.csrf,kind,id,P);},
        loadNotePending: function () { return sessionStorage.getItem('floe-note-pending'); },
        saveNotePending: function (value) { if (value === null) { sessionStorage.removeItem('floe-note-pending'); } else { sessionStorage.setItem('floe-note-pending', value); } },
        loadWaivePending: function () { return sessionStorage.getItem('floe-waive-pending'); },
        saveWaivePending: function (value) { if (value === null) { sessionStorage.removeItem('floe-waive-pending'); } else { sessionStorage.setItem('floe-waive-pending', value); } },
        context: function () { return !stopped && state && currentId ? {id: currentId, source: currentSource, state: state,
            connected: !!epoch && !!socket && socket.readyState === WebSocket.OPEN, pending: !!inflight || queue.length > 0 || !!dragShift} : null; },
        navigate: function (n, token, done) { return edit({prepared_token: token}, done); },
        restoreLayers: function (done) { return edit({restore_layers: true}, done); }, resize: resized});
    inspector = window.FloeInspect.bind({document: document, window: window, protocol: P, query: window.FloeQuery,
        context: queryContext, send: send, layers: highlightPicked, now: function () { return Date.now(); },
        setTimeout: setTimeout.bind(window), clearTimeout: clearTimeout.bind(window)});
    measurement = window.FloeMeasure.bind({document: document, window: window, protocol: P, query: window.FloeQuery, rulers: window.FloeRulers,
        history:rulerHistory, selection:function () { return inspector.selection(); },
        popCD:function (all) { return drcPanel.key(all?'K':'k'); }, cdBusy:function () { return drcPanel.rulersBusy(); },
        context: queryContext, send: send, now: function () { return Date.now(); }, setTimeout: setTimeout.bind(window), clearTimeout: clearTimeout.bind(window),
        modeChanged: function () {
            if (measurement && measurement.active()) { if (drcPanel.boxActive()) { drcPanel.key('e'); } inspector.interrupt(); }
            if (gesture) { gesture.cancel(); } reviewCursor();
        }});
    clipper = window.FloeClip.bind({document: document, protocol: P, query: window.FloeQuery, context: queryContext,
        // Window timer methods cannot be invoked with a controller as `this`.
        http: http, send: send, now: function () { return Date.now(); }, setTimeout: setTimeout.bind(window), clearTimeout: clearTimeout.bind(window),
        download: function (id) {
            if (stopped || !auth) { throw new Error('The owner session is closed.'); }
            window.FloeClip.download(document, auth.csrf, id, P);
        }});
    function snapshotReady() {
        if(stopped||document.hidden||!displayed||!lastPlacement||!currentId){return false;}
        try{const size=dims();return size.pixels.every(function(n,i){return n===lastPlacement.pixels[i];});}catch(_){return false;}
    }
    snapshots=window.FloeSnapshot.bind({document:document,window:window,protocol:P,ready:snapshotReady,
        setTimeout:setTimeout.bind(window),clearTimeout:clearTimeout.bind(window),scene:function(){
            if(!snapshotReady()){return null;}
            // Flush only annotation canvases. Neither native pixels nor world
            // coordinates are recomputed, and there is no HTTP/WS request.
            inspector.flush();measurement.flush();drcPanel.flush();
            const p=lastPlacement,layers=[];
            if(p.margin&&!marginCanvas.hidden){layers.push({canvas:marginCanvas,offset:p.margin.map(function(n){return -n;})});}
            if(!canvas.hidden){layers.push({canvas:canvas,offset:p.foreground.map(function(n){return -n;})});}
            ['query-canvas','drc-canvas','ruler-canvas'].forEach(function(id){const c=el(id);if(!c.hidden){layers.push({canvas:c,offset:[0,0]});}});
            return {pixels:p.pixels.slice(),layers:layers};
        }});
    function settingsContext(){return state?{id:currentId,epoch:epoch,rev:state.state_rev,ready:!stopped&&!document.hidden&&live()&&socket&&socket.readyState===WebSocket.OPEN,
        idle:!indexBlocked()&&!inflight&&!accepted&&!queue.length&&!submitting&&!ownerBusy}:null;}
    defaults=window.FloeDefaults.bind({el:el,protocol:P,query:window.FloeQuery,http:http,context:settingsContext,
        session:function(){return auth?auth.session_id:'';},now:function(){return Date.now();},setTimeout:setTimeout.bind(window),clearTimeout:clearTimeout.bind(window),
        loadPending:function(){return sessionStorage.getItem('floe-default-pending');},
        savePending:function(value){if(value===null){sessionStorage.removeItem('floe-default-pending');}else{sessionStorage.setItem('floe-default-pending',value);}}});
    settings=window.FloeSettings.bind({el:el,window:window,document:document,XHR:XMLHttpRequest,Blob:Blob,Encoder:TextEncoder,Decoder:TextDecoder,
        csrf:function(){return auth?auth.csrf:'';},message:message,edit:edit,setTimeout:setTimeout.bind(window),clearTimeout:clearTimeout.bind(window),
        context:settingsContext});
    palette=window.FloePalette.bind({el:el,document:document,window:window,http:http,edit:edit,styles:paletteStyle,presets:window.FloePresets,
        closeRowStyle:function(){selectedStyle=null;el('style-editor').hidden=true;},
        painted:function(){highlightPicked(pickedPairs);},
        context:function(){return !stopped&&state&&currentId?{id:currentId,key:state.render_key,
            connected:!document.hidden&&live()&&!!epoch&&!!socket&&socket.readyState===WebSocket.OPEN&&!ownerBusy&&!submitting&&!indexBlocked(),
            editable:!inflight&&!accepted&&!queue.length}:null;}});
    about=window.FloeAbout.bind({el:el,document:document,http:http,bundle:bundle});
    sessionExit=window.FloeSessionExit.bind({el:el,document:document,confirm:endSession});
    minimap=window.FloeMinimap.bind({el:el,document:document,http:http,state:function(){return !stopped&&!document.hidden&&live()&&epoch?state:null;},
        ready:function(){return !!epoch&&live()&&!inflight&&!accepted&&queue.length===0&&!(gesture&&gesture.active())&&!document.hidden;},
        navigate:nav,focus:function(){viewport.focus();}});
    function launchReady() {
        return !indexBlocked() && !stopped && !document.hidden && (!picker || !picker.blocked()) && !startupWaiting && !submitting && !ownerBusy && !inflight && !accepted && !queue.length &&
            !(gesture && gesture.active()) && (!live() || ['idle','rendering'].includes(state.status));
    }
    launcher=window.FloeLauncher.bind({el:el,http:http,protocol:P,ready:launchReady,changed:controls,
        setTimeout:function(fn,ms){return setTimeout(fn,ms);},clearTimeout:function(id){clearTimeout(id);},
        loadPending:function(){return sessionStorage.getItem('floe-launch-pending:'+auth.session_id);},
        savePending:function(value){const key='floe-launch-pending:'+auth.session_id;if(value===null){sessionStorage.removeItem(key);}else{sessionStorage.setItem(key,value);}},
        present:function(){window.focus();},completed:operationState,
        prepare:async function(item){await refreshCatalog();prepareStartup(item.request);if(item.confirm_levels){el('level-options').open=true;}controls();},
        open:async function(item,send){
            if(!launchReady()){throw new Error('Wait for pending view inputs before opening the CLI request.');}
            if(el('source').value!==item.request.source_id){prepareStartup(item.request);throw new Error('Requested source restored. Review its levels before opening.');}
            submitting=true;controls();
            try{
                const all=await operationState();if(all.active!==null){throw new Error('An operation is already running.');}
                const current=await http('GET','/api/v1/view',undefined,true);
                const input={action:'open',seq:P.next(all.last_seq),pixels:dims().pixels,levels:levels()};
                if(current&&['idle','rendering'].includes(current.view.status)){input.view_id=current.view.view_id;input.state_rev=current.view.state_rev;}
                ownerBusy=true;controls();notice('');await send(input);
            }finally{try{await operationState();}finally{submitting=false;controls();pump();}}
        }});
    picker=window.FloeBrowse.bind({el:el,document:document,http:http,protocol:P,changed:controls,
        available:function(){return !indexBlocked()&&!stopped&&!submitting&&!ownerBusy&&(!launcher||!launcher.blocked());},
        selected:function(){startupWaiting=false;pendingStartup=null;notice('File selected. Preparing the open request; indexing requires separate approval.');},
        loadPending:function(){return sessionStorage.getItem('floe-browse-pending:'+auth.session_id);},
        savePending:function(value){const key='floe-browse-pending:'+auth.session_id;if(value===null){sessionStorage.removeItem(key);}else{sessionStorage.setItem(key,value);}},
        setTimeout:function(fn,ms){return setTimeout(fn,ms);},clearTimeout:function(id){clearTimeout(id);}});
    indexOpen=window.FloeIndexOpen.bind({el:el,document:document,protocol:P,http:http,message:message,pixels:function(){return dims().pixels;},
        session:function(){return auth.session_id;},changed:controls,
        ready:function(){return !stopped&&!document.hidden&&!submitting&&!ownerBusy&&!inflight&&!accepted&&!queue.length&&!(gesture&&gesture.active())&&
            (!picker||!picker.blocked())&&(!launcher||!launcher.blocked())&&(!live()||['idle','rendering'].includes(state.status));},
        randomId:function(){if(!window.crypto||typeof window.crypto.getRandomValues!=='function'){throw Error('Secure random IDs are unavailable in this browser. No approval was submitted.');}
            const bytes=new Uint8Array(32);window.crypto.getRandomValues(bytes);return Array.from(bytes,function(n){return ('0'+n.toString(16)).slice(-2);}).join('');},
        loadPending:function(){return sessionStorage.getItem('floe-index-open:'+auth.session_id);},
        savePending:function(value){const key='floe-index-open:'+auth.session_id;if(value===null){sessionStorage.removeItem(key);}else{sessionStorage.setItem(key,value);}},
        completed:async function(value){if(value.phase==='succeeded'){await restore();pendingStartup=null;notice('');}await operationState();},
        setTimeout:function(fn,ms){return setTimeout(fn,ms);},clearTimeout:function(id){clearTimeout(id);}});
    document.addEventListener('visibilitychange', function () { settings.changed(); defaults.changed(); minimap.changed(); palette.changed(); if (document.hidden) { indexOpen.stop(); finishDecode(); inspector.changed(); measurement.changed(); clipper.changed(); } else if (!stopped) { indexOpen.resume().catch(report); if(live()){connect();} } });
    window.addEventListener('blur', function () { inspector.move(NaN, NaN); measurement.interrupt(); });
    setInterval(function () { if (socket && socket.readyState === WebSocket.OPEN && epoch) { try { send({type: 'ping'}); } catch (e) { report(e); } } }, 10000);
    window.addEventListener('pagehide', function () { palette.suspend(); indexOpen.stop(); picker.stop(); launcher.stop(); minimap.suspend(); about.stop(); sessionExit.stop(); disconnect(); inspector.stop(); measurement.stop(); clipper.stop(); snapshots.stop(); settings.stop(); defaults.stop(); clearTimeout(operationTimer); clearTimeout(resizeTimer); if (sizeObserver) { sizeObserver.disconnect(); } drcPanel.stop(); });
    window.addEventListener('pageshow', function (event) {
        if (event.persisted && auth && !stopped) { palette.resume(); minimap.resume(); about.init(); sessionExit.init(); inspector.resume(); measurement.resume(); clipper.resume(); snapshots.resume(); settings.resume(); defaults.resume(); if (sizeObserver) { sizeObserver.observe(viewport); } drcPanel.resume().then(function(){return indexOpen.resume();}).then(operationState).then(restore).then(resized).then(function(){return picker.resume();}).then(function(){return launcher.resume();}).catch(report); }
    });
    start().catch(function (e) { connection('Not connected', false); report(e); el('empty-message').textContent = e.message; });
}());
