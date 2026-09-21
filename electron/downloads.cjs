'use strict';
const { DownloadSlot } = require('./download-slot.cjs');
const P = require('./policy.cjs');
const fs = require('node:fs'), path = require('node:path');
const MAX_BYTES = 512 * 1024 * 1024;
function blobAllowed(origin, url) {
  return P.validOrigin(origin) && typeof url === 'string' &&
    new RegExp('^blob:' + origin.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '/[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$').test(url);
}
function postAllowed(origin, url) {
  if (!P.validOrigin(origin) || typeof url !== 'string' || !url.startsWith(origin)) return false;
  const m = /^\/api\/v1\/(?:artifacts|drc\/review\/(?:notes|waives)\/artifacts)\/([1-9][0-9]*)\/download$/.exec(url.slice(origin.length));
  return !!m && m[1].length <= 20 && BigInt(m[1]) <= 18446744073709551615n;
}
function nameSuggestion(name) {
  const safe = Array.from(String(name)).filter(c => !/[\p{Cc}/\\:]/u.test(c)).slice(0, 180).join('');
  return !safe || safe.startsWith('.') ? 'floe-export' : safe;
}
function mimeAllowed(mime) {
  return ['image/png', 'application/json', 'application/xml', 'text/xml', 'text/plain', 'application/octet-stream'].includes(mime);
}
function outsideProfile(directory, destination) {
  if (typeof destination !== 'string' || !path.isAbsolute(destination)) return false;
  try {
    const relative = path.relative(fs.realpathSync(directory), fs.realpathSync(path.dirname(destination)));
    return relative === '..' || relative.startsWith('..' + path.sep) || path.isAbsolute(relative);
  } catch (_) { return false; }
}
class Downloads {
  constructor({ binary, directory, origin, owns, choose, notify, stopWaitMs = 30000, createSlot = (b, d) => new DownloadSlot(b, d) }) {
    Object.assign(this, { binary, directory, origin, owns, choose, notify, createSlot });
    this.active = null; this.stopped = false; this.cleanupFailed = false; this.results = [];
    this.stopWaitMs = stopWaitMs;
    this.slots = new Set(); this.prepare();
  }
  prepare() {
    if (this.stopped) return;
    this.slot = this.createSlot(this.binary, this.directory); this.slots.add(this.slot);
    this.ready = this.slot.ready;
    this.ready.catch(() => { this.cleanupFailed = true; });
    const slot = this.slot;
    slot.finished.then(result => { if (!result.cleanup) this.cleanupFailed = true; this.slots.delete(slot); });
  }
  receive(event, item, contents) {
    let valid = false;
    try {
      const origin = this.origin(), url = item.getURL(), chain = item.getURLChain();
      valid = !this.stopped && !this.active && this.slot.phase === 'ready' && this.owns(contents, url) &&
        item.getInitiatorOrigin() === origin && (blobAllowed(origin, url) || postAllowed(origin, url)) &&
        chain.length === 1 && chain[0] === url && mimeAllowed(item.getMimeType()) &&
        item.getTotalBytes() >= 0 && item.getTotalBytes() <= MAX_BYTES;
    } catch (_) {}
    if (!valid) {
      event.preventDefault(); this.report('rejected'); return;
    }
    const slot = this.slot;
    const active = { item, slot, ended: false }; this.active = active;
    active.terminal = new Promise(resolve => { active.resolveTerminal = resolve; });
    const cancel = () => { try { if (!active.ended) item.cancel(); } catch (_) {} };
    const timer = setTimeout(cancel, 300000);
    slot.once('failure', cancel); slot.once('limit', cancel);
    item.on('updated', (_event, state) => {
      try { if (state === 'interrupted' || item.getReceivedBytes() > MAX_BYTES || this.stopped) cancel(); }
      catch (_) { cancel(); }
    });
    item.once('done', (_event, state) => {
      clearTimeout(timer);
      active.ended = true;
      active.resolveTerminal();
      this.complete(active, state).catch(() => { this.cleanupFailed = true; slot.close(); });
    });
    try { item.setSavePath(slot.path); slot.watch(); }
    catch (_) { cancel(); } // done owns cleanup; never unlink under a live Chromium writer
  }
  async complete(active, state) {
    const { slot, item } = active;
    let result;
    try {
      if (!this.stopped && state === 'completed' && item.getReceivedBytes() <= MAX_BYTES) {
        const destination = await this.choose(nameSuggestion(item.getFilename()));
        if (destination && !this.stopped) result = await slot.publish(destination);
      }
    } catch (_) {
      // A cancelled/failed native chooser is not evidence of filesystem
      // cleanup failure. The helper's terminal result remains authoritative.
    } finally {
      if (!result) { slot.close(); result = await slot.finished; }
      this.results.push(result);
      if (this.results.length > 32) this.results.shift(); // bounded diagnostic history
      if (!result.cleanup) this.cleanupFailed = true;
      if (!this.stopped) await this.report(!result.cleanup ? 'cleanup_failed' : result.publication);
      if (this.active === active) this.active = null;
      if (!this.stopped) this.prepare();
    }
  }
  report(code) { return Promise.resolve().then(() => this.notify(code)).catch(() => {}); }
  async shutdown() {
    // A DownloadItem can outlive its WebContents (notably the hidden POST
    // window). Explicitly cancel and await its terminal event BEFORE EOF lets
    // Rust unlink staging. Destroying a window alone is not a producer fence.
    this.stopped = true;
    const active = this.active;
    let retained = null;
    if (active && !active.ended) {
      try { active.item.cancel(); } catch (_) { this.cleanupFailed = true; }
      let timer;
      const stopped = await Promise.race([active.terminal.then(() => true), new Promise(resolve => {
        timer = setTimeout(() => resolve(false), this.stopWaitMs);
      })]).finally(() => clearTimeout(timer));
      if (!stopped) {
        this.cleanupFailed = true; retained = active.slot;
        retained.retain(); // no unproven unlink if Chromium failed to report done
      }
    }
    const slots = Array.from(this.slots);
    for (const slot of slots) if (slot !== retained) slot.close();
    const results = await Promise.all(slots.map(slot => slot.finished));
    if (results.some(r => !r.cleanup)) this.cleanupFailed = true;
    return !this.cleanupFailed;
  }
  pids() { return Array.from(this.slots).map(s => s.child.pid).filter(Number.isSafeInteger); }
}
module.exports = { Downloads, blobAllowed, postAllowed, nameSuggestion, mimeAllowed, outsideProfile, MAX_BYTES };
