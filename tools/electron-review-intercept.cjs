'use strict';
// Synthetic QA only: observe the product guard's decision, never replace it.
class ReviewIntercept {
  constructor(origin, webId) {
    this.origin = origin; this.webId = webId;
    this.counts = { notes: 0, waives: 0, roots: 0, exchanges: 0 };
    this.ids = new Map(); this.armed = null; this.held = null; this.failed = false;
    this.readStatus = { notes: null, waives: null };
  }
  kind(d) {
    if (!this.origin() || d.webContentsId !== this.webId() || d.method !== 'POST') return null;
    for (const kind of ['notes', 'waives']) {
      if (d.url === this.origin() + '/api/v1/drc/review/' + kind) return kind;
    }
    return null;
  }
  before(d, response, callback) {
    if (!response.cancel && !response.redirectURL && d.webContentsId === this.webId() && this.origin()) {
      if (d.method === 'GET' && d.url === this.origin() + '/') this.counts.roots++;
      if (d.method === 'POST' && d.url === this.origin() + '/api/v1/session/exchange') this.counts.exchanges++;
      const kind = this.kind(d);
      if (kind) {
        if (this.ids.has(d.id) || ++this.counts[kind] > 2) this.failed = true;
        this.ids.set(d.id, kind);
      }
    }
    callback(response);
  }
  headers(d, response, callback) {
    if (this.origin() && d.webContentsId === this.webId() && d.method === 'POST') {
      for (const name of ['notes', 'waives']) {
        if (d.url === this.origin() + '/api/v1/drc/review/' + name + '/read') this.readStatus[name] = d.statusCode;
      }
    }
    const kind = this.kind(d);
    if (kind && kind === this.armed && !response.cancel && !response.redirectURL) {
      this.armed = null;
      if (d.statusCode !== 202 || this.ids.get(d.id) !== kind || this.held) {
        this.failed = true; callback(response); return;
      }
      // No request body, headers, approval token or response body is inspected.
      const timer = setTimeout(() => { this.failed = true; this.release(); }, 10000);
      this.held = { kind, callback, timer };
      return;
    }
    callback(response);
  }
  arm(kind) {
    if (!Object.hasOwn(this.counts, kind) || !['notes', 'waives'].includes(kind) ||
        this.counts[kind] !== 0 || this.armed || this.held || this.failed) throw Error('Invalid synthetic arm');
    this.armed = kind;
  }
  release() {
    const held = this.held; this.held = null;
    if (held) { clearTimeout(held.timer); held.callback({ cancel: true }); }
  }
  snapshot() { return { ...this.counts }; }
}
module.exports = { ReviewIntercept };
