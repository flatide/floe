/* Local, ES2017 CAD client. The server owns world coordinates and view state. */
(function () {
    'use strict';
    const P = window.FloeProtocol, V = window.FloeViewer;
    const bundle = document.querySelector('meta[name="floe-bundle"]').content;
    const el = function (id) { return document.getElementById(id); };
    const timing = window.FloeDisplayTiming.bind({el: el, window: window});
    const editTimings = new WeakMap();
    document.addEventListener('visibilitychange', function () { if (document.hidden) { timing.stop(); } });
    window.addEventListener('pagehide', function () { timing.stop(); timing.clear(); });
    const canvas = el('canvas'), viewport = el('viewport');
    const context = canvas.getContext('2d', {alpha: false});
    const marginCanvas = el('margin-canvas'), marginContext = marginCanvas.getContext('2d', {alpha: false});
    marginCanvas.hidden = true;
    el('zoom-band').hidden = el('zoom-band-hint').hidden = true;
    let foregroundFrame = null, marginFrame = null, inflightBody = null, foregroundPerf = '';
    let gesture = null, dragShift = null, lastPlacement = null, viewControls = null, decodePurpose = null;
    let panes = null, menubar = null, cells = null, currentTitle = '';
    // The GTK title gains "· root NAME" under a view root.
    function titleSuffix() {
        const root = state && state.root_name ? ' · root ' + state.root_name : '';
        el('document-title').textContent = currentTitle + root; document.title = currentTitle + root + ' · floe2';
    }
    let completedFrame = null, decodeFailed = false;
    let drcPanel = null, displayProjection = null, frozenProjection = null;
    let inspector = null, measurement = null, clipper = null, snapshots = null, overlayMode = 'all', pickedPairs = [];
    let settings = null, defaults = null, about = null, sessionExit = null, dumps = null;
    let minimap = null, launcher = null, picker = null, indexOpen = null, palette = null;
    let sharing = null;
    const rulerHistory = window.FloeRulers.history();
    let ackedFrames = {foreground: null, margin: null};
    const sessionKey = 'floe-session:' + location.origin;
    let auth = null, stopped = false, socket = null, epoch = '', state = null;
    let pageRun = 0, pageSuspended = false, restoreRead = 0;
    function currentPage(run) { return !stopped && !pageSuspended && run === pageRun; }
    let startupTask = null, startupComplete = false, startupSent = false;
    let seq = '0', queue = [], inflight = null, accepted = null, lastSend = 0;
    const editCallbacks = new WeakMap();
    let socketSerial = 0, decode = null, reconnectTimer = null, reconnectDelay = 500;
    let catalog = [], currentId = '', currentSource = '', currentMode = 'level', ownerBusy = false, submitting = false;
    let revisionSupported = false, revisionCandidate = null, currentRevision = null, currentLevels = null;
    let revisionUsage = null;
    let reclaimSupported=false,reclaimPreview=null,reclaimSeen='',reclaimSpent='',reclaimDeadline=0,reclaimTimer=null,reclaimChoices='';
    let modeReceipt = '', modeSupported = false, levelsSupported = false, fillEditSupported = false;
    let pendingStartup = null, startupWaiting = false;
    let selectedStyle = null;
    let levelNext = null, levelSource = '', levelIds = new Set(), levelLoad = 0, levelBusy = false;
    let operationTimer = null, resizeTimer = null, displayed = false;
    const errors = {
        index_unavailable: 'A current index is required. Review “Index and open…” or use “Index this source”; opening never indexes automatically.',
        busy: 'Resources or cache are busy. Wait, choose fewer index jobs, or explicitly close a reader before rebuilding its cache.',
        worker_version: 'The native renderer version does not match. Rebuild the matched binaries.',
        worker_failed: 'The renderer failed. Any remaining image is the last displayed frame, not a live result. Close this layout, then use Open layout to retry. Check the local service diagnostics.',
        stale_state: 'The view changed in another connection. That edit was not replayed.',
        prepared_edit_expired: 'This prepared move is no longer current. Select the error again; it was not replayed.',
        prepared_edit_unavailable: 'The move could not be prepared. Try again.',
        prepared_edit_limit: 'This view cannot prepare another move. Close and reopen it.',
        invalid_request: 'The requested value or selection is not supported.',
        preview_unavailable: 'This reclamation preview is no longer valid. Prepare the exact source/set again and give new approval; deletion was not replayed.',
        approval_required: 'Separate explicit approval is required for this operation.',
        fill_edit_disabled: 'Bitmap-slot editing requires FLOE_FILL_EDIT at launch. Nothing was applied.',
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
        drc_context_changed: 'The view changed while reading DRC. Select the error again.', drc_view_root: 'View root active: DRC positions are top-cell coordinates. Return to the top cell first.',
        drc_busy: 'The DRC read queue is busy. Retry this page.',
        drc_closed: 'The DRC reader is closed.',
        drc_selection_conflict: 'Selection changed in another request. Server state will be reloaded; no command is retried.',
        drc_selection_limit: 'Selection limit reached. The previous selection was preserved.',
        invalid_drc_request: 'Invalid DRC index, cursor, coordinate or page limit.'
    };
    function notice(text) { el('notice').textContent = text || ''; el('notice').hidden = !text; }
    function message(error) { return errors[error] || String(error || 'Request failed'); }
    function report(error) { notice(message(error.message || error)); }
    function http(method, path, body, missing, token, options) {
        // options: {blob, headers} for an upload, {limit} for a reply larger
        // than the 1 MiB default (bounded by the caller's own contract).
        const run = pageRun, upload = options && options.blob ? options : null, limit = options && options.limit ? options.limit : 1024 * 1024;
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
                    if (xhr.responseText.length > limit) { throw new Error('Reply limit'); }
                    if (xhr.responseText) { value = JSON.parse(xhr.responseText); }
                } catch (e) { reject(e); return; }
                if (xhr.status < 200 || xhr.status >= 300) {
                    if (xhr.status === 401 && currentPage(run)) { stopped = true; el('connection').setAttribute('data-session-state', 'restart-required'); if (sharing) { sharing.stop(); } if (dumps) { dumps.stop(); } if (indexOpen) { indexOpen.stop(); } if (picker) { picker.stop(); } if (launcher) { launcher.stop(); } connection('Session expired', false); }
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
    function connection(text, ready) { el('connection').textContent = text; el('connection').className = 'connection' + (ready ? ' ready' : ''); updateCursor(); }
    function dims() {
        return V.dimensions(P, viewport.getBoundingClientRect(), window.devicePixelRatio || 1);
    }
    function pendingPan() {
        return V.panDelta(P, state, (inflightBody ? [inflightBody] : []).concat(queue), dragShift);
    }
    function dumpChanged() { if (dumps) { dumps.changed(); } }
    function viewerContext() {
        const h = lastPlacement && lastPlacement.full ? marginFrame : foregroundFrame;
        return {active: !stopped && !pageSuspended && !document.hidden && !decodeFailed,
            connected: !!socket && socket.readyState === WebSocket.OPEN && !!epoch,
            connecting: !!socket && (!epoch || !state || state.connection_epoch !== epoch || socket.readyState === 0), state: state, frame: h, completedFrame: completedFrame,
            pending: !!inflight || !!accepted || queue.length > 0, decoding: !!decode && decodePurpose === 'foreground',
            acked: !!h && ackedFrames[h.purpose] === h.frame_id,
            gesture: !!(gesture && gesture.active()) || ownerBusy || submitting || indexBlocked()};
    }
    function updateCursor() {
        V.cursor(el('app-shell'), viewport, V.busy(P, viewerContext()), gesture,
            !!((drcPanel && drcPanel.boxActive()) || (measurement && measurement.active())));
    }
    function present() {
        if (stopped) { updateCursor(); return; }
        const size = dims(), delta = state && pendingPan();
        lastPlacement = V.compose({protocol: P, canvas: canvas, marginCanvas: marginCanvas, foreground: foregroundFrame,
            margin: marginFrame, state: state, size: size, delta: delta});
        const mp = lastPlacement.margin, full = lastPlacement.full;
        if (full) { displayProjection = window.FloeDRC.projection(marginFrame, mp, state.dbu_um); }
        else if (foregroundFrame) { displayProjection = window.FloeDRC.projection(foregroundFrame, lastPlacement.foreground, state.dbu_um); }
        else { displayProjection = frozenProjection && window.FloeDRC.shifted(frozenProjection.projection,
            [-Math.round((size.pixels[0] - frozenProjection.pixels[0]) / 2), -Math.round((size.pixels[1] - frozenProjection.pixels[1]) / 2)]); }
        if (drcPanel) { drcPanel.paint(displayProjection, size); }
        if (inspector) { inspector.changed(); inspector.paint(displayProjection, size); }
        if (measurement) { measurement.changed(); measurement.paint(displayProjection, size); }
        if (clipper) { clipper.changed(); }
        if (snapshots) { snapshots.changed(); }
        dumpChanged();
        if (minimap) { minimap.changed(); }
        const failed = state && state.status === 'failed';
        if (failed) {
            // Retained pixels remain useful for reference, but cannot turn a
            // terminal worker failure back into a "Live" margin crop.
            el('status').textContent = 'failed · ' + (displayed ? 'last displayed image (not live)' : 'no frame displayed');
        } else if (full) {
            const pending = !!inflightBody || queue.length > 0 || !!dragShift;
            el('status').textContent = (pending ? 'Pan preview' : 'Live') + ' · margin crop · gen ' + marginFrame.generation;
        }
        el('perf').textContent = foregroundPerf + (failed ? (foregroundPerf ? ' · previous frame' : '') : full ? ' · crop (no foreground render)' : '');
        el('margin-info').textContent = failed ? '' : state && state.capabilities.margin ?
            (state.margin_working ? 'Prefetching' : (mp ? 'Margin ready' : 'Margin pending')) +
            (marginFrame ? ' · ' + (Number((marginFrame.perf || {}).raster_us || 0) / 1000).toFixed(1) + ' ms bg' : '') +
            (marginFrame && marginFrame.labels_truncated ? ' · labels partial' : '') + (state.margin_failure ? ' · prefetch failed' : '') : '';
        updateCursor();
    }
    function freezeMargin() {
        // Freeze the actual composite, including a truncated margin's incoming
        // strip. A non-period mouse release must not recenter the previous image.
        const p = lastPlacement;
        if (displayed && p) {
            frozenProjection = {projection: displayProjection, pixels: p.pixels.slice()};
            const copy = V.freeze({canvas: canvas, marginCanvas: marginCanvas, placement: p, document: document, size: dims()});
            // A no-op/rejected edit produces no replacement frame. If the
            // pixels were not copied/recomposed, their original receipt is
            // still valid; pending/revision checks already prevent queries
            // during a real state change.
            if (copy) { foregroundFrame = null; }
            lastPlacement = {pixels: p.pixels, margin: null, full: false, foreground: [0, 0]};
        }
        marginCanvas.hidden = true;
    }
    function clearBuffers() {
        timing.clear();
        if (dumps) { dumps.reset(); }
        foregroundFrame = null; marginFrame = null; foregroundPerf = '';
        completedFrame = null; decodeFailed = false;
        lastPlacement = null; dragShift = null;
        displayProjection = null; frozenProjection = null;
        canvas.hidden = false; canvas.width = 1; canvas.height = 1;
        marginCanvas.hidden = true; marginCanvas.width = 1; marginCanvas.height = 1;
        [canvas, marginCanvas].forEach(function (target) {
            delete target.dataset.frameId; delete target.dataset.renderRev; delete target.dataset.bboxDbu;
        });
        ackedFrames = {foreground: null, margin: null};
        updateCursor();
    }
    function live() { return !stopped && state && !['closed', 'failed'].includes(state.status); }
    function indexBlocked() { return !!indexOpen && indexOpen.blocked(); }
    function deckModeReady() {
        return modeSupported && live() && state.capabilities.mode &&
            ['idle','rendering'].includes(state.status) && socket && socket.readyState === WebSocket.OPEN && epoch &&
            !indexBlocked() && !submitting && !ownerBusy && !inflight && !accepted && !queue.length && !(gesture && gesture.active());
    }
    function controls() {
        if(sharing){sharing.changed();}
        const enabled = live() && socket && socket.readyState === WebSocket.OPEN && !!epoch && !submitting && !ownerBusy && !indexBlocked();
        ['fit', 'zoom-in', 'zoom-out', 'goto', 'depth', 'detail', 'thin', 'frames', 'labels', 'mono', 'layers-all', 'layers-none'].forEach(function (id) { el(id).disabled = !enabled; });
        el('overlays').disabled=!live();
        el('labels').disabled = !enabled || !state.capabilities.labels;
        el('font-px').disabled = !enabled || !state.capabilities.labels;
        const launchPending = launcher && launcher.blocked(), source = catalog.find(function (s) { return s.source_id === el('source').value; });
        el('open').disabled = stopped || submitting || ownerBusy || !!live() || !source || launchPending || indexBlocked();
        el('source').disabled = stopped || !catalog.length || launchPending || ownerBusy || submitting || indexBlocked();
        el('mode').disabled = stopped || !source || !source.deck || launchPending || ownerBusy || submitting || indexBlocked();
        el('close').disabled = !currentId || submitting || ownerBusy || indexBlocked();
        el('index').disabled = stopped || submitting || ownerBusy || !source || indexBlocked();
        const revisionReady = revisionSupported && !!source && !stopped && !submitting && !ownerBusy && !indexBlocked() && !launchPending && !document.hidden;
        el('revision-build').disabled = !revisionReady || !el('revision-approve').checked;
        el('revision-check').disabled = !revisionReady;
        el('revision-usage').disabled = !revisionReady;
        const usageText = revisionUsage && revisionUsage.source_id === el('source').value ? revisionUsage.text : 'Storage usage not checked for this source.';
        if(el('revision-usage-status').textContent !== usageText){el('revision-usage-status').textContent = usageText;}
        const reclaimReady=revisionReady&&reclaimSupported;
        el('reclaim-prepare').disabled=!reclaimReady||!window.FloeIndexRevisions.id(el('reclaim-id').value);
        el('reclaim-id').disabled=!reclaimReady;el('reclaim-choice').disabled=!reclaimReady;
        const rp=reclaimPreview&&reclaimPreview.preview;
        const canReclaim=reclaimReady&&!!rp&&!rp.complete&&rp.token!==reclaimSpent&&Date.now()<reclaimDeadline&&
            reclaimPreview.source_id===el('source').value&&rp.revision===el('reclaim-id').value;
        if(!canReclaim){el('reclaim-approve').checked=false;}
        el('reclaim-approve').disabled=!canReclaim;
        el('reclaim-run').disabled=!canReclaim||!el('reclaim-approve').checked;
        let revisionMatches = false;
        try { revisionMatches = window.FloeIndexRevisions.matches(revisionCandidate, el('source').value, levels()); } catch (_) { /* incomplete selection */ }
        el('revision-use').disabled = !revisionReady || !revisionMatches || !!inflight || !!accepted || !!queue.length || !!(gesture && gesture.active()) || !!(live() && !epoch) || !!(drcPanel && drcPanel.recoveryBusy());
        el('cancel-job').disabled = stopped || !ownerBusy || indexBlocked();
        el('levels-all').disabled = indexBlocked();
        el('level-more').disabled = indexBlocked() || levelBusy || levelNext === null;
        Array.from(el('level-list').querySelectorAll('input')).forEach(function (box) { box.disabled = indexBlocked() || el('levels-all').checked; });
        el('live-mode-row').hidden = !modeSupported || !state || !state.capabilities.mode;
        el('live-mode').disabled = !deckModeReady();
        el('live-mode').value = currentMode;
        el('reselect-levels').hidden = !levelsSupported || !state || !state.capabilities.mode || el('source').value !== currentSource;
        el('reselect-levels').disabled = !deckModeReady() || !levelsSupported || el('source').value !== currentSource || !!launchPending;
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
        if (cells) { cells.changed(); }
        if (panes) { panes.changed(); }
        if (menubar) { menubar.refresh(); }
        updateCursor();
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
        timing.endEdit(editTimings.get(body), !error); editTimings.delete(body);
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
            if(body.slot_request){
                const draft=body.slot_request;wire.type='view.fill_slot';wire.connection_epoch=draft.context.epoch;
                wire.view_id=draft.context.id;wire.base_state_rev=draft.context.rev;wire.body=draft.body;
            }else if (body.prepared_token) { wire.token = body.prepared_token; } else { wire.body = body; }
            inflight = send(wire);
            timing.sent(editTimings.get(body));
            lastSend = Date.now();
        } catch (e) { rejectEdits(e.message); report(e); }
    }
    function edit(body, done) {
        if (done) { editCallbacks.set(body, done); }
        const error = ownerBusy || submitting || indexBlocked() ? 'An owner operation or approval is pending. This input was not applied.' : !live() || !epoch ? 'Open a connected view first.' : queue.length >= 64 ? 'Input queue is full. This input was not applied.' : null;
        if (error) { notice(error); settleEdit(body, error); return null; }
        decodeFailed = false;
        if (!body.navigation || body.navigation.kind !== 'pan' || !body.navigation.snap) { freezeMargin(); }
        const editTiming = timing.beginEdit(body.navigation && body.navigation.kind);
        if (editTiming) { editTimings.set(body, editTiming); }
        queue.push(body); pump(); present();
        return function () {
            const i = queue.indexOf(body);
            if (i < 0) { return false; } // A sent edit may already be committed.
            queue.splice(i, 1); settleEdit(body, 'Request cancelled'); controls(); present(); return true;
        };
    }
    function syncGoto(force) {
        if (viewControls) { viewControls.syncGoto(force); }
    }
    function statusSnapshot(s) {
        if (s.view_id !== currentId || s.connection_epoch !== epoch) { return; }
        if (state && state.connection_epoch === epoch && P.compare(s.state_rev, state.state_rev) < 0) { return; }
        if (s.status === 'closed') { clearClosedView(); return; }
        if (!state || s.render_rev !== state.render_rev) { decodeFailed = false; }
        if (gesture && gesture.active() && state && (s.state_rev !== state.state_rev || s.connection_epoch !== state.connection_epoch)) { gesture.cancel(); }
        if (marginFrame && !P.placement(marginFrame, s)) {
            freezeMargin(); marginFrame = null; marginCanvas.width = 1; marginCanvas.height = 1;
        }
        state = s;
        if (s.status === 'failed') { finishDecode(); }
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
        viewControls.sync();
        const b = s.bbox_dbu.map(Number), dbu = Number(s.dbu_um);
        el('viewport-info').textContent = ((b[2] - b[0]) * dbu).toPrecision(6) + ' × ' + ((b[3] - b[1]) * dbu).toPrecision(6) + ' µm';
        el('status').textContent = s.status + ' · depth ' + s.depth + ' · thin:' + s.effective_thin + (s.source_stale ? ' · SOURCE STALE' : '');
        if (currentTitle) { titleSuffix(); }
        el('dstatus').textContent = 'depth: ' + s.depth + (s.max_depth == null ? '' : '/' + s.max_depth) + ' · detail: ' + s.detail + ' · thin:' + s.effective_thin + ' · frame:' + (s.frames ? 'on' : 'off');
        pump();
        present();
    }
    function finishDecode() { if (decode) { decode(); decode = null; } decodePurpose = null; updateCursor(); }
    function decodeImage(h,data,callback){
        return window.FloeImageDecode.create({Image:Image,ImageData:ImageData,Blob:Blob,URL:URL,
            setTimeout:setTimeout.bind(window),clearTimeout:clearTimeout.bind(window)},h,data,callback);
    }
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
        const receivedAt = timing.now();
        let packet;
        try { packet = P.packet(buffer); } catch (e) { report(e); ws.close(); return; }
        const h = packet.header;
        const valid = function () { return live() && ws === socket && serial === socketSerial && P.matches(h, state) &&
            (!accepted || (h.purpose === 'foreground' && P.compare(h.render_rev, accepted.render) >= 0)) && !document.hidden; };
        if (!valid()) { acknowledge(h, 'discarded', ws, serial); return; }
        if (decode) { notice('Frame credit violation'); ws.close(); return; }
        const frameTiming = timing.beginFrame(h, receivedAt);
        let done = false;
        function finish(draw) {
            if (done) { return; } done = true;
            let disposition = 'discarded', submitted = false;
            try {
                if (draw && valid()) {
                    const target = h.purpose === 'margin' ? marginCanvas : canvas;
                    V.paint(target, h, draw);
                    if (dumps) { dumps.received(h, target); }
                    if (h.purpose === 'margin') { marginFrame = h; } else { foregroundFrame = h; completedFrame = h; frozenProjection = null; }
                    if (!displayed && document.activeElement === document.body) { viewport.focus(); }
                    displayed = true; el('empty').hidden = true; disposition = 'displayed';
                    target.dataset.frameId = h.frame_id; target.dataset.renderRev = h.render_rev;
                    target.dataset.bboxDbu = JSON.stringify(h.bbox_dbu);
                    const perf = h.perf || {}, ms = function (name) { return perf[name] ? (Number(perf[name]) / 1000).toFixed(1) : '0'; };
                    const fitted = ['fit_pct','fit_cull','fit_over','fit_thin'].some(function (k) { return Number(perf[k] || 0) > 0; });
                    el('status').textContent = V.frameStatus(h);
                    if (h.purpose === 'foreground') {
                        foregroundPerf = 'Rust foreground · plan ' + ms('plan_us') + ' ms · decode ' + ms('decode_us') + ' ms · draw ' + ms('raster_us') +
                            ' ms · ' + (perf.pages || '0') + ' pages · ' + h.width + ' × ' + h.height + ' px · ' + h.format +
                            (perf.wall_us !== undefined ? ' · native wall ' + ms('wall_us') + ' ms/queue ' + ms('queue_us') + ' ms/text ' + ms('text_plan_us') + ' ms' : '') +
                            (fitted ? ' · budget fit: cut ×' + (Number(perf.fit_pct || '100') / 100).toFixed(2) +
                                (Number(perf.fit_thin || 0) > 0 ? ', boundary class 1/2^' + perf.fit_thin : '') +
                                (Number(perf.fit_full_pct || 0) > 0 ? ', full ≥×' + (Number(perf.fit_full_pct) / 100).toFixed(2) : ', no full class') +
                                (Number(perf.fit_none_pct || 0) > 0 ? ', omitted <×' + (Number(perf.fit_none_pct) / 100).toFixed(2) : ', no wholly omitted class') +
                                (Number(perf.fit_cull || 0) > 0 ? ', hairline cull' : '') + (Number(perf.fit_over || 0) > 0 ? ', over limit' : '') : '') +
                            (Number(perf.once_tiles || 0) + Number(perf.once_passes || 0) + Number(perf.once_items || 0) > 0 ?
                                ' · write-once ' + (perf.once_tiles || '0') + ' tiles/' + (perf.once_passes || '0') + ' passes/' + (perf.once_items || '0') + ' items skipped' : '') +
                            ' · round ' + h.round + (h.final ? ' · final frame' : ' · refining') +
                            (Number(perf.stored_rep_points || 0) > 0 ? ' · stored reps ' + perf.stored_rep_points + '/tested ' + (perf.stored_rep_tested || '0') +
                                (Number(perf.stored_rep_limited || 0) > 0 ? ' (capped)' : '') : '') +
                            (Number(perf.stored_rep_nodes || 0) > 0 ? ' · OVR tree ' + perf.stored_rep_nodes + ' nodes/' + (perf.stored_rep_proxies || '0') +
                                ' proxies/' + (perf.stored_rep_bytes || '0') + ' bytes/' + (perf.stored_rep_painted_pixels || '0') + ' painted px' : '');
                    }
                    present();
                    submitted = true;
                }
            } catch (e) { decodeFailed = true; report(e); }
            timing.endFrame(frameTiming, submitted);
            decode = null; decodePurpose = null; acknowledge(h, disposition, ws, serial); updateCursor();
        }
        const task=decodeImage(h,packet.data,
            function(draw,error){timing.decoded(frameTiming);if(error){decodeFailed=true;notice(error.message);}finish(draw);});
        decodeFailed=false;decode=function(){task.cancel();timing.endFrame(frameTiming,false);};decodePurpose=h.purpose;updateCursor();task.start();
    }
    function disconnect() {
        timing.interrupt();
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
        updateCursor();
    }
    function connect() {
        disconnect(); if (stopped || pageSuspended || !currentId || !live()) { return; }
        const serial = socketSerial;
        const ws = new WebSocket(location.origin.replace(/^http/, 'ws') + '/api/v1/events',
            ['floe.v1', 'bundle.' + bundle, 'csrf.' + auth.csrf]);
        socket = ws; ws.binaryType = 'arraybuffer'; seq = '0'; connection('Connecting', false);updateCursor();
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
                    timing.ack(editTimings.get(inflightBody));
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
            timing.interrupt();
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
        ws.onerror = function () { if (!stopped && socket === ws && serial === socketSerial) { connection('Connection error', false); } };
    }
    async function restore() {
        const run = pageRun;
        if (!currentPage(run)) { return false; }
        const ticket = ++restoreRead, serial = socketSerial, id = currentId, observed = state;
        const active = function () { return currentPage(run) && ticket === restoreRead && serial === socketSerial; };
        let current;
        try { current = await http('GET', '/api/v1/view', undefined, true); }
        catch (e) {
            if (active() && id === currentId && state === observed) { throw e; }
            return false;
        }
        // Reconnect and operation completion can read concurrently on one page.
        // Neither an earlier read nor a pre-snapshot revision can replace newer
        // connection state. A current-page 401 is still handled by http().
        if (!active()) { return false; }
        if (current && current.view.view_id === currentId && state) {
            const revision = P.compare(current.view.state_rev, state.state_rev);
            // Render progress/failure can update a snapshot without changing
            // state_rev; equal revisions cannot order those two observations.
            if (revision < 0 || revision === 0 && state !== observed) { return false; }
        }
        if (!current) {
            // A 404 has no revision to compare with a snapshot received after
            // this read began; it cannot displace that newer live observation.
            if (id !== currentId || state !== observed) { return false; }
            if (currentId || displayed) { clearClosedView(); } else { controls(); }
            return true;
        }
        if (current.view.status === 'closed') { clearClosedView(); return true; }
        const changed = currentId !== current.view.view_id;
        if (changed) { displayed = false; clearBuffers(); el('empty').hidden = false; selectedStyle = null; el('style-editor').hidden = true; }
        currentId = current.view.view_id; currentSource = current.source_id; currentMode = current.mode; state = current.view;
        currentRevision = current.index_revision || null; currentLevels = current.levels;
        el('revision-current').textContent = currentRevision ? 'Open index revision: ' + currentRevision : 'Open index: legacy mutable cache';
        currentTitle = current.title; titleSuffix();
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
        syncGoto(false); controls(); connect(); return true;
    }
    function paletteStyle(r, scope, valid) {
        const color = document.createElement('input'); color.type = 'color'; color.value = r.color; color.className = 'layer-edit'; color.setAttribute('aria-label', 'Color ' + r.name);
        color.onchange = function () {
            const value=color.value; color.value=r.color;
            if (!valid()) { notice('Layer styles changed or an input is pending. Select the layer again.'); return; }
            edit({style_batch: {pairs:[r.pair],collapsed:r.closed?[r.pair]:[],color:value}});
        };
        const style = document.createElement('button'); style.type = 'button'; style.className = 'layer-swatch';
        style.setAttribute('aria-label', 'Edit style ' + r.name); style.title = r.color + ' · ' + r.fill.kind + ' · ' + r.width + ' px';
        const swatch = document.createElement('canvas'); swatch.setAttribute('aria-hidden', 'true'); window.FloePalette.swatch(swatch, r); style.appendChild(swatch);
        style.onclick = function () {
            if (!valid()) { return; }
            palette.closeStyle();
            selectedStyle = {row: r, key: scope.key, view: scope.id, valid:valid};
            el('style-title').textContent = r.name; el('style-fill').value = r.fill.kind; el('style-width').value = r.width; el('style-color').value = r.color;
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
    function indexSummaryControls() {
        el('index-occupancy-prune').disabled = !el('index-occupancy').checked;
        el('index-representatives-format').disabled = el('index-representatives').disabled || !el('index-representatives').checked;
    }
    function sourceSelection() {
        const source = catalog.find(function (r) { return r.source_id === el('source').value; });
        if (!source) { el('source-note').textContent = 'No source registered in this workspace.'; el('level-options').hidden = true; controls(); return; }
        if (pendingStartup && pendingStartup.source_id !== source.source_id) { pendingStartup = null; startupWaiting = false; }
        el('mode').disabled = !source.deck; el('level-options').hidden = !source.deck;
        el('index-representatives').disabled = source.deck;
        if (!source.deck) { el('mode').value = 'level'; }
        if (levelSource !== source.source_id) {
            reclaimPreview=null;reclaimSeen='';reclaimDeadline=0;clearTimeout(reclaimTimer);
            el('reclaim-approve').checked=false;el('reclaim-id').value='';
            el('reclaim-status').textContent='No reclamation preview for this source.';
            el('index-occupancy').checked = source.deck;
            el('index-representatives').checked = false;
            el('index-occupancy-prune').value = '';
            el('index-representatives-format').value = '';
            ++levelLoad; levelBusy = false;
            levelSource = source.source_id; levelNext = null; levelIds = new Set(); el('level-list').textContent = ''; el('levels-all').checked = true;
            if (source.deck) { moreLevels().catch(report); }
        }
        el('source-note').textContent = source.deck ? 'Jobdeck · select levels before opening.' : 'OASIS layout · current index required.';
        indexSummaryControls();
        refreshReclaimChoices();
        controls();
    }
    async function refreshCatalog() {
        const run = pageRun, selected = el('source').value;
        const result = await http('GET', '/api/v1/catalog');
        if (!currentPage(run)) { return; }
        catalog = result.sources;
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
        if (op.kind === 'index_revision' && op.phase === 'succeeded') { return 'New index published: ' + op.index_revision + '. Current view unchanged. Check, then Use to switch.' + (op.revision_sync_warning ? ' WARNING: published, sync or retirement receipt incomplete.' : ''); }
        const p = op.native || {};
        return op.kind + ' · ' + op.phase + (p.phase ? ' · ' + p.phase : '') + (op.error ? ' · ' + message(op.error) : '') +
            (p.representatives_missing === true ? ' · WARNING: base cache completed without representative points; retry with --representatives-only' : '') +
            (op.kind === 'index' ? window.FloeIndexOpen.renameText(op) : '');
    }
    async function operationState() {
        const run = pageRun;
        try { return await readOperationState(); }
        catch (e) {
            // Read-only reconciliation after an uncertain mutation response;
            // never re-submit the mutation automatically.
            if (currentPage(run) && !document.hidden) {
                clearTimeout(operationTimer);
                operationTimer = setTimeout(function () { operationState().catch(report); }, 1000);
            }
            throw e;
        }
    }
    async function readOperationState() {
        const run = pageRun;
        const all = await http('GET', '/api/v1/operations');
        if (!currentPage(run)) { return all; }
        if (indexOpen) { indexOpen.observe(all); }
        ownerBusy = all.active !== null; el('cancel-job').disabled = !ownerBusy;
        el('cancel-job').dataset.seq = all.active || ''; controls();
        const recent = all.history || [], last = recent[recent.length - 1];
        revisionCandidate = window.FloeIndexRevisions.candidate(recent);
        revisionUsage = window.FloeIndexRevisions.usage(recent);
        observeReclamation(recent);
        el('revision-status').textContent = revisionCandidate ? 'Checked revision: ' + revisionCandidate.index_revision + ' (selection must match)' : 'No matching published revision checked. Check the selected source/levels; no indexing or switch is automatic.';
        controls();
        if (last) { el('operation').textContent = operationLabel(last); }
        el('live-mode-note').textContent = ownerBusy && last && last.kind === 'mode' ? 'Changing jobdeck mode…' : 'Ctrl+, toggles level/chip · same camera and loaded levels. Mode defaults reload.';
        if (!ownerBusy && last && ['failed', 'incomplete', 'cancelled'].includes(last.phase)) {
            notice(message(last.error || last.phase));
            if (!currentId) { el('empty-message').textContent = message(last.error || last.phase); connection('Local · ready', true); }
        }
        if (ownerBusy) { operationTimer = setTimeout(function () { operationState().catch(report); }, 500); }
        else if (last && (last.kind === 'open' || last.kind === 'reselect_levels' || last.kind === 'use_revision' || last.kind === 'index_open' && !indexOpen.pending()) && last.phase === 'succeeded') {
            if (last.view_id !== currentId) { await restore(); }
            else if (pendingStartup && pendingStartup.source_id === currentSource) { pendingStartup = null; }
        }
        else if (last && last.kind === 'mode' && last.seq !== modeReceipt) {
            const applied = await restore();
            if (!currentPage(run) || !applied) { return all; }
            modeReceipt = last.seq;
            if (last.phase === 'succeeded' && (!state || !state.failure)) { notice(''); }
        }
        if (!currentPage(run)) { return all; }
        if (!ownerBusy && !submitting) { pump(); }
        return all;
    }
    async function submitOperation(request, beforeSend) {
        const run = pageRun;
        if (!currentPage(run)) { return; }
        if (ownerBusy || submitting || indexBlocked()) { throw new Error('An operation or approval is already pending.'); }
        submitting = true; controls();
        if (operationTimer) { clearTimeout(operationTimer); operationTimer = null; }
        try {
            const all = await operationState();
            if (!currentPage(run)) { return; }
            if (all.active !== null) { throw new Error('An operation is already running.'); }
            request.seq = P.next(all.last_seq); ownerBusy = true; controls(); notice('');
            if (beforeSend) { beforeSend(); }
            try { await http('POST', '/api/v1/operations', request); }
            finally { if (currentPage(run)) { await operationState(); } }
        } finally { submitting = false; controls(); pump(); }
    }
    async function changeDeckMode(mode) {
        if (!['level','chip','layer'].includes(mode)) { throw new Error('Invalid jobdeck mode.'); }
        if (!deckModeReady()) { el('live-mode').value = currentMode; throw new Error('Wait for the current view and pending inputs before changing jobdeck mode.'); }
        if (mode === currentMode) { return; }
        await submitOperation({kind:'mode', view_id:currentId, base_state_rev:state.state_rev, mode:mode});
    }
    el('live-mode').onchange = function () { changeDeckMode(el('live-mode').value).catch(report); };
    el('reselect-levels').onclick = function () {
        if (!levelsSupported || !deckModeReady() || el('source').value !== currentSource || launcher && launcher.blocked()) {
            report(Error('Select the open jobdeck and wait for pending inputs before changing loaded levels.')); return;
        }
        try {
            submitOperation({kind:'reselect_levels',view_id:currentId,base_state_rev:state.state_rev,levels:levels()}).catch(report);
        } catch (e) { report(e); }
    };
    async function openSource(startup) {
        const run = pageRun;
        const remembered = pendingStartup && pendingStartup.source_id === el('source').value ? pendingStartup : null;
        const request = Object.assign({}, startup || {kind: 'open', mode: el('mode').value, source_id: el('source').value, levels: levels(),
            display_policy: remembered ? remembered.display_policy || 'explicit' : 'window', body: remembered ? remembered.body : {}});
        if (!startup && remembered && remembered.label_preference !== undefined) { request.label_preference = remembered.label_preference; }
        request.body = Object.assign({}, request.body, {pixels:dims().pixels});
        await submitOperation(request, startup ? function () { startupSent = true; } : null);
        if (currentPage(run)) { startupWaiting = false; }
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
    async function authenticate() {
        const fragment = location.hash;
        if (fragment) { history.replaceState(null, '', location.pathname); }
        if (!context || !marginContext || typeof WebSocket !== 'function' || typeof TextEncoder !== 'function' ||
            typeof TextDecoder !== 'function' || typeof ImageData !== 'function' || typeof URL.createObjectURL !== 'function' ||
            typeof window.requestAnimationFrame !== 'function' || typeof window.cancelAnimationFrame !== 'function') {
            throw new Error('This browser lacks Canvas 2D/binary WebSocket/UTF-8 image APIs. Use a supported Firefox and restart the local session.');
        }
        if (/^#bootstrap=[0-9a-f]{64}$/.test(fragment)) {
            const result = await http('POST', '/api/v1/session/exchange', {bootstrap: fragment.slice(11), protocol: 1, bundle: bundle});
            if (stopped) { return; }
            // Preserve the one-shot exchange receipt even if pagehide happened
            // while waiting. It must not be submitted again after BFCache restore.
            auth = result;
            try { sessionStorage.setItem(sessionKey, JSON.stringify(auth)); } catch (_) { notice('Session storage unavailable. Reloading requires a new local session.'); }
        } else {
            try { auth = JSON.parse(sessionStorage.getItem(sessionKey)); } catch (_) { auth = null; }
            if (!auth) {
                // A terminal bootstrap/storage condition, not transient network
                // delay. Native hosts read only this fixed, non-secret marker.
                el('connection').setAttribute('data-session-state', 'restart-required');
                throw new Error('Session credentials are unavailable. Start a new floe2-desktop session, or use a fresh private session link from floe2-web.');
            }
        }
    }
    async function start(run) {
        if (!auth) { await authenticate(); }
        if (!currentPage(run)) { return; }
        const caps = await http('GET', '/api/v1/capabilities');
        if (!currentPage(run)) { return; }
        if (caps.protocol !== 1 || caps.bundle !== bundle) { throw new Error('Client/server version mismatch. Reload the page.'); }
        sharing.init(caps.share_grants);
        about.init(); sessionExit.init();
        modeSupported = caps.jobdeck_modes === true;
        levelsSupported = caps.jobdeck_levels === true;
        fillEditSupported = caps.fill_slot_edit === true;
        await refreshCatalog();
        if (!currentPage(run)) { return; }
        if (caps.drc) { await (run === 0 ? drcPanel.init() : drcPanel.resume()); }
        if (!currentPage(run)) { return; }
        await clipper.init(caps.exports);
        if (!currentPage(run)) { return; }
        snapshots.init(caps.snapshot_png);
        dumps.init(caps.display_dump, pageRun === 0 && caps.dump_on_start);
        settings.capabilities(caps.layer_settings);
        await defaults.init(caps.design_defaults);
        if (!currentPage(run)) { return; }
        sourceSelection();
        revisionSupported = !!caps.index_revisions; el('revision-panel').hidden = !revisionSupported;
        reclaimSupported=!!caps.index_reclamation;el('reclaim-panel').hidden=!reclaimSupported;
        await indexOpen.init(caps.index_open);
        if (!currentPage(run)) { return; }
        const operations = await operationState();
        if (!currentPage(run)) { return; }
        // Operation reconciliation may already have restored the submitted open.
        if (!socket) { await restore(); }
        if (!currentPage(run)) { return; }
        if (!currentId && operations.last_seq === '0' && !startupSent) {
            const preferences = await http('GET', '/api/v1/startup'), startup = preferences.request;
            if (!currentPage(run)) { return; }
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
        } else if (!currentId && operations.last_seq === '0' && startupSent) {
            el('empty-message').textContent = 'The initial open was already submitted. Check operation status before opening again. No automatic retry.';
            connection('Local · ready', true);
        }
        if (!currentPage(run)) { return; }
        await picker.init(caps.file_picker, !catalog.length);
        if (!currentPage(run)) { return; }
        await launcher.init(caps.launcher || caps.file_picker);
        if (currentPage(run)) { startupComplete = true; }
    }
    function continueStartup() {
        if (startupTask || startupComplete || stopped || pageSuspended) { return; }
        const run = pageRun;
        // One initializer owns capabilities and controller setup. A restored
        // page waits for its predecessor to retire, then repeats only reads;
        // an already-submitted initial open is reconciled, never replayed.
        startupTask = start(run).catch(function (e) {
            if (currentPage(run)) { connection('Not connected', false); report(e); el('empty-message').textContent = e.message; }
        }).then(function () {
            startupTask = null;
            if (run !== pageRun && currentPage(pageRun)) { continueStartup(); }
        });
    }
    el('source').onchange = sourceSelection;
    el('open').onclick = function () { openSource(null).catch(report); };
    function clearClosedView() {
        ++restoreRead;
        disconnect(); state = null; currentId = ''; displayed = false; clearBuffers();
        currentRevision = null; currentLevels = null; el('revision-current').textContent = '';
        selectedStyle = null; el('style-editor').hidden = true;
        controls(); el('empty').hidden = false; el('empty-message').textContent = 'View closed. Choose a source to reopen.';
        el('rendering').hidden = true; el('layers').textContent = ''; el('status').textContent = 'View closed';
        ['perf', 'margin-info', 'viewport-info', 'max-depth'].forEach(function (id) { el(id).textContent = ''; });
        connection('Local · ready', true);
    }
    el('close').onclick = async function () {
        const run = pageRun, id = currentId;
        if (!currentPage(run) || !id) { return; }
        try {
            await http('DELETE', '/api/v1/views/' + id);
            // A close can commit before its HTTP reply arrives. A restored or
            // replacement view owns its own connection and displayed buffers.
            if (!currentPage(run) || currentId !== id) { return; }
            clearClosedView();
        } catch (e) { if (currentPage(run) && currentId === id) { report(e); } }
    };
    async function endSession() {
        clearTimeout(reclaimTimer);reclaimPreview=null;el('reclaim-approve').checked=false;
        sharing.stop();
        palette.stop();
        if (indexOpen) { indexOpen.stop(); }
        if (launcher) { launcher.stop(); }
        if (picker) { picker.stop(); }
        if (minimap) { minimap.stop(); }
        about.stop();
        if (drcPanel) { drcPanel.stop(); }
        if (clipper) { clipper.stop(); }
        if (snapshots) { snapshots.stop(); }
        if (dumps) { dumps.stop(); }
        if (settings) { settings.stop(); }
        if (defaults) { defaults.stop(); }
        stopped = true; disconnect(); if (operationTimer) { clearTimeout(operationTimer); }
        clearTimeout(resizeTimer); if (sizeObserver) { sizeObserver.disconnect(); }
        // Stop local presentation immediately, even if shutdown times out.
        // Recovery records are only retired after the server confirms below.
        state = null; currentId = ''; displayed = false; clearBuffers(); controls();
        inspector.stop(); measurement.stop(); selectedStyle = null; el('style-editor').hidden = true;
        el('empty').hidden = false; el('empty-message').textContent = 'Local view stopped.';
        el('rendering').hidden = true; el('layers').textContent = '';
        el('status').textContent = 'Local view stopped';
        ['perf', 'margin-info', 'viewport-info', 'max-depth'].forEach(function (id) { el(id).textContent = ''; });
        let failure=null;
        try { await http('DELETE', '/api/v1/session'); } catch (e) { failure=e; }
        if(failure){
            connection('Server shutdown unconfirmed', false);
            notice('Local view stopped; server shutdown is unconfirmed. Check the local launcher before starting another session. Earlier approved writes may have completed; their recovery records are retained. No automatic retry. '+message(failure.message));
            return;
        }
        if (drcPanel) { drcPanel.stop(true); }
        if (defaults) { defaults.stop(true); }
        if (indexOpen) { try { indexOpen.clear(); } catch (_) { /* Confirmed ended session cannot accept this record. */ } }
        try { sessionStorage.removeItem(sessionKey); } catch (_) { /* storage may be disabled */ }
        el('status').textContent = 'Session ended'; el('empty-message').textContent = 'Session ended. Close this tab.';
        connection('Session ended', false); notice('Session ended. Close this tab.');
    }
    el('levels-all').onchange = function () { Array.from(el('level-list').querySelectorAll('input')).forEach(function (box) { box.disabled = el('levels-all').checked; }); };
    el('level-more').onclick = function () { moreLevels().catch(report); };
    viewControls = V.bindControls({el: el, document: document, protocol: P, edit: edit, notice: notice,
        gesture: function () { return gesture; }, context: function () { return {id: currentId, state: state, ready: live(),
            gotoBlocked: !currentId || (launcher && launcher.blocked()) || (pendingStartup && pendingStartup.source_id !== currentSource)}; }});
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
        const color = String(el('style-color').value || '').toLowerCase();
        if (/^#[0-9a-f]{6}$/.test(color) && color !== String(row.color).toLowerCase()) { delta.color = color; }
        if (fill.kind !== row.fill.kind || (fill.kind === 'pattern' && fill.rows.some(function (n, i) { return n !== row.fill.rows[i]; }))) { delta.fill = fill; }
        if (width !== row.width) { delta.width = width; }
        if (delta.fill || delta.color || delta.width !== undefined) { edit({style_batch: delta}); }
        selectedStyle = null; el('style-editor').hidden = true;
    };
    const nav = viewControls.navigate;
    panes = window.FloePanes.bind({el: el, document: document, window: window, focus: function () { viewport.focus(); },
        resized: function () { resized(); }, changed: function () { if (menubar) { menubar.refresh(); } },
        blocked: function () { return stopped; }});
    function overlays(mode) {
        if(!['all','focus','none'].includes(mode)){return;}
        overlayMode=mode;el('overlays').value=mode;
        inspector.showOverlay(mode!=='none');measurement.showOverlay(mode!=='none');drcPanel.overlayMode(mode);
    }
    el('overlays').onchange=function(){overlays(el('overlays').value);};
    viewport.addEventListener('mousedown', function () { viewport.focus(); });
    viewport.addEventListener('keydown', function (event) {
        if (!live() || event.isComposing || event.keyCode === 229) { return; }
        if(event.target===viewport&&!event.altKey&&!event.shiftKey&&(event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='c') {
            const selected=typeof window.getSelection==='function'&&window.getSelection();
            if(!selected||selected.isCollapsed){event.preventDefault();snapshots.request('copy');}return;
        }
        if (gesture && gesture.active()) { if (event.key === 'Escape') { event.preventDefault(); gesture.cancel(); } return; }
        if (event.metaKey || event.altKey) { return; }
        const key = V.keyName(event);
        if (event.ctrlKey) {
            if (key === ',' && !event.shiftKey && !event.repeat && event.target === viewport && modeSupported && state.capabilities.mode) {
                event.preventDefault(); changeDeckMode(currentMode === 'level' ? 'chip' : 'level').catch(report);
            } else { viewControls.key(event); }
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
        if (cells && cells.key(key, event)) { event.preventDefault(); return; }
        viewControls.key(event);
    });
    viewport.addEventListener('wheel', function (event) {
        V.wheel({protocol: P, gestures: window.FloeGestures, viewport: viewport, navigate: nav, context: function () {
            const c = viewerContext(); c.active = c.active && live(); c.decoding = !!decode;
            try { c.size = dims(); } catch (_) { c.size = null; } return c;
        }}, event);
    }, {passive: false});
    function reviewCursor() {
        if (measurement && measurement.active() && drcPanel && drcPanel.boxActive()) { measurement.leave(); }
        updateCursor();
        if (inspector && (!gesture || !gesture.active())) { inspector.changed(); }
    }
    // GTK's cursor readout: a display-only linear projection of the current
    // viewport; queries and navigation still resolve coordinates in Rust.
    function cursorReadout(x, y) {
        const c = el('cursor-info');
        if (!state || !currentId || !live() || !Number.isFinite(x)) { c.textContent = ''; return; }
        const r = viewport.getBoundingClientRect(), b = state.bbox_dbu.map(Number), dbu = Number(state.dbu_um);
        if (!(r.width > 0 && r.height > 0)) { c.textContent = ''; return; }
        const wx = (b[0] + (x - r.left) / r.width * (b[2] - b[0])) * dbu, wy = (b[3] - (y - r.top) / r.height * (b[3] - b[1])) * dbu;
        c.textContent = 'x ' + wx.toFixed(3) + '  y ' + wy.toFixed(3) + ' um';
    }
    viewport.addEventListener('mousemove', function (event) {
        cursorReadout(event.clientX, event.clientY);
        if (!gesture || !gesture.active()) {
            if (drcPanel) { drcPanel.move(event.clientX, event.clientY); }
            if (measurement && measurement.active()) { measurement.move(event.clientX, event.clientY, event); }
            else if (inspector) { inspector.move(event.clientX, event.clientY); }
        }
    });
    viewport.addEventListener('mouseleave', function () { cursorReadout(NaN, NaN); if (drcPanel) { drcPanel.move(NaN, NaN); } if (inspector) { inspector.move(NaN, NaN); } if (measurement) { measurement.move(NaN, NaN); } });
    gesture = window.FloeGestures.bind({viewport: viewport, window: window, document: document,
        now: timing.now, previewMeasured: timing.preview,
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
        bandPreview:function(b){V.band(el,b);},
        cursor: function(active){if(!active){timing.endGesture();}reviewCursor();}, pan: nav, objectClicks: true, selectionMode: function () { return !!drcPanel && drcPanel.boxActive(); },
        click: function (x, y, twice, modifiers) {
            if (measurement && measurement.active()) { measurement.click(x, y, modifiers); return; }
            const m = modifiers || {};
            if (drcPanel && (drcPanel.boxActive() || (!m.ctrlKey && !m.metaKey && !m.shiftKey)) && drcPanel.click(x, y, twice, modifiers)) {
                if (inspector) { inspector.interrupt(); } return;
            }
            if (inspector) { inspector.click(x, y, modifiers); }
        }});
    el('index-occupancy').onchange = indexSummaryControls;
    el('index-representatives').onchange = indexSummaryControls;
    function indexOptions() {
        const options = {jobs: Number(el('index-jobs').value), force: el('index-force').checked, lod: el('index-lod').checked,
            occupancy: el('index-occupancy').checked, representatives: !el('index-representatives').disabled && el('index-representatives').checked};
        if (options.occupancy && el('index-occupancy-prune').value !== '') { options.occupancy_prune = Number(el('index-occupancy-prune').value); }
        if (options.representatives && el('index-representatives-format').value !== '') { options.representatives_format = Number(el('index-representatives-format').value); }
        return options;
    }
    el('revision-approve').onchange = controls;
    function observeReclamation(recent){
        const next=window.FloeIndexRevisions.reclaimCandidate(recent),p=next&&next.preview;
        if(!p||p.token!==reclaimSeen){
            el('reclaim-approve').checked=false;reclaimSeen=p?p.token:'';
            reclaimDeadline=p?Date.now()+p.expires_in_s*1000:0;
        }
        reclaimPreview=next;clearTimeout(reclaimTimer);reclaimTimer=null;
        if(p&&reclaimDeadline>Date.now()){reclaimTimer=setTimeout(controls,reclaimDeadline-Date.now());}
        el('reclaim-status').textContent=window.FloeIndexRevisions.reclaimText(recent);
        refreshReclaimChoices();
    }
    function refreshReclaimChoices(){
        const choices=revisionUsage&&revisionUsage.source_id===el('source').value?revisionUsage.choices:[];
        const key=JSON.stringify([el('source').value,choices]);
        if(key!==reclaimChoices){
            reclaimChoices=key;el('reclaim-choice').textContent='';
            const empty=document.createElement('option');empty.value='';empty.textContent='Select an observed set (not deletion permission)';el('reclaim-choice').appendChild(empty);
            choices.forEach(function(c){const option=document.createElement('option');option.value=c.revision;option.textContent=c.label;option.disabled=c.current;el('reclaim-choice').appendChild(option);});
            el('reclaim-choice').value='';
        }
    }
    el('reclaim-id').oninput=function(){el('reclaim-approve').checked=false;controls();};
    el('reclaim-choice').onchange=function(){el('reclaim-id').value=el('reclaim-choice').value;el('reclaim-approve').checked=false;controls();};
    el('reclaim-approve').onchange=controls;
    el('reclaim-prepare').onclick=function(){
        if(el('reclaim-prepare').disabled){return;}
        el('reclaim-approve').checked=false;
        submitOperation({kind:'prepare_reclaim',source_id:el('source').value,revision:el('reclaim-id').value}).catch(report);
    };
    el('reclaim-run').onclick=function(){
        if(el('reclaim-run').disabled){return;}
        try{
            if(Date.now()>=reclaimDeadline){throw Error('Reclamation preview expired; prepare again.');}
            const request=window.FloeIndexRevisions.reclaimRequest(reclaimPreview,el('source').value,el('reclaim-id').value,el('reclaim-approve').checked);
            reclaimSpent=request.token;el('reclaim-approve').checked=false;
            submitOperation(request).catch(report);
        }catch(e){report(e);}
    };
    window.addEventListener('pagehide',function(){clearTimeout(reclaimTimer);el('reclaim-approve').checked=false;reclaimPreview=null;});
    el('revision-build').onclick = function () {
        if (el('revision-build').disabled || !el('revision-approve').checked) { return; }
        try {
            const options = indexOptions(); options.force = false;
            const request = {kind:'index_revision', source_id:el('source').value, levels:levels(), approved:true, options:options};
            el('revision-approve').checked = false;
            submitOperation(request).catch(report);
        } catch(e) { report(e); }
    };
    el('revision-check').onclick = function () {
        if(el('revision-check').disabled){return;}
        try { submitOperation({kind:'check_revision',source_id:el('source').value,levels:levels()}).catch(report); } catch(e) { report(e); }
    };
    el('revision-usage').onclick = function () {
        if(el('revision-usage').disabled){return;}
        submitOperation({kind:'revision_usage',source_id:el('source').value}).catch(report);
    };
    el('revision-use').onclick = function () {
        if(el('revision-use').disabled){return;}
        try {
            const current = live() ? {source_id:currentSource,mode:currentMode,levels:currentLevels===null?{mode:'all'}:{mode:'only',ids:currentLevels},view_id:currentId,state_rev:state.state_rev} : null;
            const request = window.FloeIndexRevisions.useRequest(revisionCandidate,el('source').value,levels(),el('mode').value,dims().pixels,current);
            submitOperation(request).catch(report);
        } catch(e) { report(e); }
    };
    el('index').onclick = function () {
        try {
            const options = indexOptions();
            submitOperation({kind: 'index', source_id: el('source').value, levels: levels(), options: options}).catch(report);
        }
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
    drcPanel = window.FloeDRC.bind({document: document, window: window, protocol: P, http: http, painted:dumpChanged,
        history:rulerHistory, rulerKey:function (key) { return measurement && !drcPanel.boxActive() && measurement.key(key); },
        stateStore: window.FloePanelState, rulers: window.FloeRulers, groups: window.FloeDRCGroups, builds: window.FloeDRCBuild, cursor: reviewCursor,
        notes: window.FloeDRCNotes, noteDisplay: window.FloeDRCNoteDisplay, waives: window.FloeDRCWaives, transfers: window.FloeDRCTransfer, recovery: window.FloeDRCRecovery, session: function () { return auth ? auth.session_id : ''; },
        loadRecoveryPending: function () { return sessionStorage.getItem('floe-review-recovery'); },
        saveRecoveryPending: function (value) { if (value === null) { sessionStorage.removeItem('floe-review-recovery'); } else { sessionStorage.setItem('floe-review-recovery', value); } },
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
    inspector = window.FloeInspect.bind({document: document, window: window, protocol: P, query: window.FloeQuery, painted:dumpChanged,
        context: queryContext, send: send, layers: highlightPicked, now: function () { return Date.now(); },
        setTimeout: setTimeout.bind(window), clearTimeout: clearTimeout.bind(window)});
    cells = window.FloeCells.bind({el: el, document: document, http: http, edit: edit,
        context: function () { return !stopped && state && currentId ? {id: currentId, state: state,
            connected: !document.hidden && live() && !!epoch && !!socket && socket.readyState === WebSocket.OPEN && !ownerBusy && !submitting && !indexBlocked()} : null; },
        size: function () { try { return dims(); } catch (_) { return null; } }, unit: function () { return state ? Number(state.dbu_um) : 1; },
        focus: function () { viewport.focus(); }, raise: function () { panes.raise('left', 'cells-page'); },
        rootAllowed: function () { return !!(state && state.capabilities && state.capabilities.cell_root); },
        buildAllowed: function () { return false; },
        setTimeout: setTimeout.bind(window), clearTimeout: clearTimeout.bind(window)});
    measurement = window.FloeMeasure.bind({document: document, window: window, protocol: P, query: window.FloeQuery, rulers: window.FloeRulers, painted:dumpChanged,
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
    function displayedScene(){
        if(!snapshotReady()){return null;}
        // Flush only annotation canvases. Neither native pixels nor world
        // coordinates are recomputed, and there is no HTTP/WS request.
        inspector.flush();measurement.flush();drcPanel.flush();
        const p=lastPlacement,layers=[];
        if(p.margin&&!marginCanvas.hidden){layers.push({canvas:marginCanvas,offset:p.margin.map(function(n){return -n;})});}
        if(!canvas.hidden){layers.push({canvas:canvas,offset:p.foreground.map(function(n){return -n;})});}
        ['query-canvas','drc-canvas','ruler-canvas'].forEach(function(id){const c=el(id);if(!c.hidden){layers.push({canvas:c,offset:[0,0]});}});
        return {pixels:p.pixels.slice(),layers:layers};
    }
    snapshots=window.FloeSnapshot.bind({document:document,window:window,protocol:P,ready:snapshotReady,
        setTimeout:setTimeout.bind(window),clearTimeout:clearTimeout.bind(window),scene:displayedScene});
    dumps=window.FloeDisplayDump.bind({document:document,window:window,protocol:P,compose:window.FloeSnapshot.compose,scene:displayedScene});
    function settingsContext(){return state?{id:currentId,epoch:epoch,rev:state.state_rev,ready:!stopped&&!document.hidden&&live()&&socket&&socket.readyState===WebSocket.OPEN,
        idle:!indexBlocked()&&!inflight&&!accepted&&!queue.length&&!submitting&&!ownerBusy}:null;}
    defaults=window.FloeDefaults.bind({el:el,protocol:P,query:window.FloeQuery,http:http,context:settingsContext,
        session:function(){return auth?auth.session_id:'';},now:function(){return Date.now();},setTimeout:setTimeout.bind(window),clearTimeout:clearTimeout.bind(window),
        loadPending:function(){return sessionStorage.getItem('floe-default-pending');},
        savePending:function(value){if(value===null){sessionStorage.removeItem('floe-default-pending');}else{sessionStorage.setItem('floe-default-pending',value);}}});
    settings=window.FloeSettings.bind({el:el,window:window,document:document,XHR:XMLHttpRequest,Blob:Blob,Encoder:TextEncoder,Decoder:TextDecoder,
        csrf:function(){return auth?auth.csrf:'';},message:message,edit:edit,setTimeout:setTimeout.bind(window),clearTimeout:clearTimeout.bind(window),
        context:settingsContext});
    palette=window.FloePalette.bind({el:el,document:document,window:window,http:http,edit:edit,styles:paletteStyle,presets:window.FloePresets,slotEditor:window.FloeFillEditor,wholeList:true,
        editSlot:function(c,body,done){
            const now=settingsContext();
            if(!fillEditSupported||!now||!now.ready||!now.idle||now.id!==c.id||now.epoch!==c.epoch||now.rev!==c.rev||state.fill_slots_key!==c.slotKey){done('View changed; the bitmap was not replayed.');return null;}
            return edit({slot_request:{context:c,body:body}},done);
        },
        closeRowStyle:function(){selectedStyle=null;el('style-editor').hidden=true;},
        painted:function(){highlightPicked(pickedPairs);},
        context:function(){return !stopped&&state&&currentId?{id:currentId,key:state.render_key,epoch:epoch,rev:state.state_rev,slotKey:state.fill_slots_key,fillEdit:fillEditSupported,
            connected:!document.hidden&&live()&&!!epoch&&!!socket&&socket.readyState===WebSocket.OPEN&&!ownerBusy&&!submitting&&!indexBlocked(),
            editable:!inflight&&!accepted&&!queue.length}:null;}});
    const displayTest=window.FloeDisplayTest.bind({el:el,document:document,window:window,XHR:XMLHttpRequest,bundle:bundle,
        csrf:function(){return auth?auth.csrf:'';},decode:decodeImage});
    about=window.FloeAbout.bind({el:el,document:document,http:http,bundle:bundle,displayTest:displayTest});
    sessionExit=window.FloeSessionExit.bind({el:el,document:document,confirm:endSession});
    sharing=window.FloeSharing.bind({el:el,document:document,http:http,origin:location.origin,
        context:function(){return !stopped&&!document.hidden&&live()&&!inflight&&!accepted&&!queue.length?
            {view_id:currentId,state_rev:state.state_rev,source_id:currentSource}:null;}});
    window.addEventListener('pagehide',function(){sharing.suspend();});
    minimap=window.FloeMinimap.bind({el:el,document:document,http:http,state:function(){return !stopped&&!document.hidden&&live()&&epoch?state:null;},
        ready:function(){return !!epoch&&live()&&!inflight&&!accepted&&queue.length===0&&!(gesture&&gesture.active())&&!document.hidden;},
        navigate:nav,focus:function(){viewport.focus();}});
    function launchReady() {
        // A trusted CLI adds a proposal, not consent to discard current edits.
        // Keep review/approval panels intact even after their editor lost focus.
        const editing = viewControls.dirty() || ['browse-dialog','index-open-dialog','about-dialog','session-exit-dialog','share-dialog',
            'notes-editor','notes-review','notes-uncertain','notes-cancel',
            'waives-editor','waives-review','waives-uncertain','waives-cancel',
            'default-review','default-uncertain','default-cancel','recovery-preview','recovery-resolve','recovery-check'].some(function(id){return !el(id).hidden;});
        return !editing && !(drcPanel&&drcPanel.recoveryBusy()) && !indexBlocked() && !stopped && !document.hidden && (!picker || !picker.blocked()) && !startupWaiting && !submitting && !ownerBusy && !inflight && !accepted && !queue.length &&
            !(gesture && gesture.active()) && (!live() || ['idle','rendering'].includes(state.status));
    }
    launcher=window.FloeLauncher.bind({el:el,http:http,protocol:P,ready:launchReady,changed:controls,
        setTimeout:function(fn,ms){return setTimeout(fn,ms);},clearTimeout:function(id){clearTimeout(id);},
        loadPending:function(){return sessionStorage.getItem('floe-launch-pending:'+auth.session_id);},
        savePending:function(value){const key='floe-launch-pending:'+auth.session_id;if(value===null){sessionStorage.removeItem(key);}else{sessionStorage.setItem(key,value);}},
        present:function(){window.focus();},completed:operationState,
        prepare:async function(item){const run=pageRun;await refreshCatalog();if(!currentPage(run)){return;}prepareStartup(item.request);if(item.confirm_levels){el('level-options').open=true;}controls();},
        open:async function(item,send){
            if(!launchReady()){throw new Error('Wait for pending view inputs before opening the CLI request.');}
            if(el('source').value!==item.request.source_id){prepareStartup(item.request);throw new Error('Requested source restored. Review its levels before opening.');}
            const run=pageRun;submitting=true;controls();
            try{
                const all=await operationState();if(!currentPage(run)){return;}if(all.active!==null){throw new Error('An operation is already running.');}
                const current=await http('GET','/api/v1/view',undefined,true);
                if(!currentPage(run)){return;}
                const input={action:'open',seq:P.next(all.last_seq),pixels:dims().pixels,levels:levels()};
                if(current&&['idle','rendering'].includes(current.view.status)){input.view_id=current.view.view_id;input.state_rev=current.view.state_rev;}
                ownerBusy=true;controls();notice('');await send(input);
            }finally{try{if(currentPage(run)){await operationState();}}finally{submitting=false;controls();pump();}}
        }});
    picker=window.FloeBrowse.bind({el:el,document:document,http:http,protocol:P,changed:controls,
        available:function(){return !indexBlocked()&&!stopped&&!submitting&&!ownerBusy&&(!launcher||!launcher.blocked());},
        drcContext:function(){return drcPanel.openContext();},
        reviewGrant:function(){return drcPanel.reviewGrant();},
        drcSelected:async function(kind){await drcPanel.refresh();notice(kind==='load_drc_rules'?'SVRF metadata replaced; layout and reviewer unchanged. Filters, selection and unapproved previews reset.':kind==='reconnect_drc_review'?'Launcher reviewer reconnected; layout unchanged. Automatic saving remains off until enabled again.':'DRC opened read-only; layout unchanged. Reviewer registration and automatic saving do not transfer.');},
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
    function menuModel() {
        function proxy(label, id, key, extra) { return Object.assign({label: label, proxy: id, key: key}, extra || {}); }
        function dialogItem(label, name, key, prepare) { return {label: label, key: key, action: function () { panes.show(name, prepare); }}; }
        const deckHidden = function () { return el('live-mode-row').hidden; };
        return [
            {name: 'File', items: [
                proxy('Load layout…', 'browse-open'), proxy('Load jobdeck…', 'browse-open'),
                dialogItem('Registered sources…', 'source'), dialogItem('Index this source…', 'index'),
                {sep: true},
                dialogItem('Clip region…', 'clip', null, function () { if (!el('clip-open').disabled && el('clip-form').hidden) { el('clip-open').click(); } }),
                proxy('Copy view to clipboard', 'snapshot-copy', 'Ctrl+C'), proxy('Save view PNG…', 'snapshot-save'),
                {sep: true},
                proxy('Load layer settings…', 'settings-load'), proxy('Save layer settings…', 'settings-save'),
                dialogItem('Shared design default…', 'settings'),
                {sep: true},
                proxy('End session', 'logout', 'q')
            ]},
            {name: 'View', items: [
                proxy('Fit (zoom all)', 'fit', 'Ctrl+A'),
                {label: 'Zoom in 50%', key: 'Ctrl+Z', enabled: function () { return !el('fit').disabled; }, action: function () { nav({kind: 'zoom', factor: 0.5}); }},
                {label: 'Zoom out 50%', key: 'Shift+Z', enabled: function () { return !el('fit').disabled; }, action: function () { nav({kind: 'zoom', factor: 2}); }},
                {label: 'Goto position…', key: 'g', enabled: function () { return !el('goto').disabled; }, action: function () { el('goto-x').focus(); el('goto-x').select(); }},
                {sep: true},
                {label: 'Detail', sub: [{label: 'Low · 5 px', radio: {id: 'detail', value: 'low'}}, {label: 'Medium · 3 px', radio: {id: 'detail', value: 'medium'}},
                    {label: 'High · 1 px', radio: {id: 'detail', value: 'high'}}, {label: 'Exact · no cut', radio: {id: 'detail', value: 'exact'}}], key: 'd'},
                {label: 'Label size…', enabled: function () { return !el('font-px').disabled; }, action: function () { panes.show('display'); el('font-px').focus(); el('font-px').select(); }},
                {label: 'Depth +1', key: '>', enabled: function () { return !el('depth').disabled; }, action: function () { edit({depth_step: 1}); }},
                {label: 'Depth −1', key: '<', enabled: function () { return !el('depth').disabled; }, action: function () { edit({depth_step: -1}); }},
                {label: 'Depth full', key: '9 9', enabled: function () { return !el('depth').disabled; }, action: function () { edit({depth: 'full'}); }},
                {sep: true},
                {label: 'Hierarchy frames', key: 'f', toggle: 'frames'}, {label: 'Labels', toggle: 'labels'}, {label: 'Grayscale layers', key: 'b', toggle: 'mono'},
                {label: 'Thin shapes at wide views', sub: [{label: 'Auto (keep)', radio: {id: 'thin', value: 'auto'}}, {label: 'Keep (thin shapes as hairlines)', radio: {id: 'thin', value: 'keep'}}, {label: 'Cull (drop all-thin pages, faster)', radio: {id: 'thin', value: 'cull'}}]},
                {label: 'Overlays', key: 'Tab', sub: [{label: 'All', radio: {id: 'overlays', value: 'all'}}, {label: 'Hide other errors', radio: {id: 'overlays', value: 'focus'}}, {label: 'Hide all', radio: {id: 'overlays', value: 'none'}}]},
                {sep: true},
                {label: 'Layers', sub: [proxy('Show all', 'layers-all'), proxy('Hide all', 'layers-none'), {sep: true}, proxy('Show selected', 'layers-show'), proxy('Hide selected', 'layers-hide'),
                    proxy('Toggle selected', 'layers-toggle'), proxy('Style selected…', 'layers-style'), proxy('Clear selection', 'layers-clear'), {sep: true}, proxy('Expand all groups', 'layers-expand'), proxy('Collapse all groups', 'layers-collapse')]},
                {sep: true},
                dialogItem('Display options…', 'display')
            ]},
            {name: 'Cell', items: [
                {label: 'Cell tree / find cell…', key: 't', enabled: function () { return !el('cells-search').disabled; }, action: function () { panes.raise('left', 'cells-page'); el('cells-search').focus(); }},
                proxy('Zoom to selected cell', 'cells-zoom', 'Enter'),
                {label: 'Highlight instances', toggle: 'cells-highlight'},
                {label: 'Clear highlight', key: 'Esc', enabled: function () { return cells && cells.hasSelection(); }, action: function () { cells.clearHighlight(); }},
                {sep: true},
                proxy('Selected cell as view root', 'cells-root', 'Ctrl+T'), proxy('View root: back to the top cell', 'cells-top', 'Ctrl+Shift+T'),
                {sep: true},
                proxy('Build cell index (design.ovh)…', 'cells-build')
            ]},
            {name: 'Ruler', items: [
                {label: 'Ruler mode', key: 'r', enabled: function () { return !el('ruler-mode').disabled; }, check: function () { return el('ruler-mode').getAttribute('aria-pressed') === 'true'; }, action: function () { el('ruler-mode').click(); }},
                {label: 'Edge/vertex snap', key: 'm', toggle: 'ruler-snap'},
                {sep: true},
                proxy('Delete last ruler', 'ruler-pop', 'k'), proxy('Clear rulers', 'ruler-clear', 'Shift+K'),
                {sep: true},
                {label: 'Snap probe', key: 'm', toggle: 'snap-probe'}, proxy('Clear shape selection', 'pick-clear', 'Esc'),
                {label: 'Inspect pane', action: function () { panes.raise('left', 'inspect-page'); }}
            ]},
            {name: 'DRC', items: [
                proxy('Open results .db…', 'drc-open'), proxy('Load SVRF rules…', 'drc-rules-load'), proxy('Reconnect launcher reviewer…', 'drc-reconnect'),
                {sep: true},
                proxy('Next error', 'drc-step-next', '.'), proxy('Previous error', 'drc-step-prev', ','),
                {label: 'Waive/unwaive current error', key: 'w', enabled: function () { return !el('drc-step-next').disabled; }, action: function () { drcPanel.key('w'); }},
                {sep: true},
                {label: 'Note (add/edit)', key: 'n', enabled: function () { return !el('drc-step-next').disabled; }, action: function () { drcPanel.key('n'); }},
                {sep: true},
                {label: 'Error box-select mode', key: 'e', enabled: function () { return !el('drc-box').disabled; }, check: function () { return el('drc-box').getAttribute('aria-pressed') === 'true'; }, action: function () { el('drc-box').click(); }},
                proxy('Build pack…', 'drc-build-open'), proxy('Restore layers', 'drc-restore-layers'),
                {sep: true},
                {label: 'Show DRC pane', hidden: function () { return el('drc-toggle').hidden; }, check: function () { return !el('drc-panel').hidden; }, action: function () { el('drc-toggle').click(); if (!el('drc-panel').hidden) { panes.raise('left', 'drc-page'); } }}
            ]},
            {name: 'Jobdeck', items: [
                {label: 'Level view (mask levels)', hidden: deckHidden, radio: {id: 'live-mode', value: 'level'}},
                {label: 'Chip view (CHIP blocks)', hidden: deckHidden, radio: {id: 'live-mode', value: 'chip'}},
                {label: 'Source layer view (LY/DT)', hidden: deckHidden, radio: {id: 'live-mode', value: 'layer'}},
                {sep: true},
                {label: 'Toggle level view / chip view', key: 'Ctrl+,', hidden: deckHidden, enabled: function () { return !el('live-mode').disabled; }, action: function () { changeDeckMode(currentMode === 'level' ? 'chip' : 'level').catch(report); }},
                {sep: true},
                {label: 'Select levels to load…', action: function () { panes.show('source', function () { el('level-options').open = true; }); }},
                dialogItem('Open mode…', 'source')
            ]},
            {name: 'Help', items: [
                proxy('About floe2', 'about-open'), proxy('Open source licenses', 'about-open'),
                {sep: true},
                proxy('Share locally…', 'share-open')
            ]}
        ];
    }
    menubar = window.FloeMenubar.bind({el: el, document: document, window: window, model: menuModel(), focus: function () { viewport.focus(); }, report: report});
    document.addEventListener('visibilitychange', function () { settings.changed(); defaults.changed(); minimap.changed(); palette.changed(); if (document.hidden) { indexOpen.stop(); finishDecode(); inspector.changed(); measurement.changed(); clipper.changed(); } else if (!stopped && !pageSuspended && startupComplete) { indexOpen.resume().catch(report); if(live()){connect();} } });
    window.addEventListener('blur', function () { inspector.move(NaN, NaN); measurement.interrupt(); });
    setInterval(function () { if (socket && socket.readyState === WebSocket.OPEN && epoch) { try { send({type: 'ping'}); } catch (e) { report(e); } } }, 10000);
    window.addEventListener('pagehide', function () { pageSuspended = true; ++pageRun; dumps.stop(); palette.suspend(); indexOpen.stop(); picker.stop(); launcher.stop(); minimap.suspend(); about.stop(); sessionExit.stop(); disconnect(); inspector.stop(); measurement.stop(); clipper.stop(); snapshots.stop(); settings.stop(); defaults.stop(); clearTimeout(operationTimer); clearTimeout(resizeTimer); if (sizeObserver) { sizeObserver.disconnect(); } drcPanel.stop(); });
    async function resumePage(run) {
        // A later pagehide/pageshow owns a different restore chain. Check each
        // boundary: stop() alone cannot fence a controller resumed afterwards.
        try {
            await drcPanel.resume();
            if (!currentPage(run)) { return; }
            await indexOpen.resume();
            if (!currentPage(run)) { return; }
            await operationState();
            if (!currentPage(run)) { return; }
            await restore();
            if (!currentPage(run)) { return; }
            resized();
            await picker.resume();
            if (!currentPage(run)) { return; }
            await launcher.resume();
        } catch (e) { if (currentPage(run)) { report(e); } }
    }
    window.addEventListener('pageshow', function (event) {
        if (event.persisted && !stopped) {
            pageSuspended = false; const run = ++pageRun;
            dumps.resume(); palette.resume(); minimap.resume(); about.init(); sessionExit.init(); inspector.resume(); measurement.resume(); snapshots.resume(); settings.resume();
            if (sizeObserver) { sizeObserver.observe(viewport); }
            if (startupComplete) { clipper.resume(); defaults.resume(); resumePage(run); }
            else { continueStartup(); }
        }
    });
    continueStartup();
}());
