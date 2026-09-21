'use strict';
const { validOrigin } = require('./policy.cjs');

// No clipboard contents cross this controller. An isolated-world probe reports
// only current Chromium activation/focus, never creates a synthetic activation.
const activationProbe = 'navigator.userActivation.isActive === true && !document.hidden && document.hasFocus()';
class ClipboardController {
  constructor({ origin, owns, allowed, probe, timeout = 750, now = () => performance.now() }) {
    Object.assign(this, { origin, owns, allowed, probe, timeout, now });
    this.pending = null; this.ended = false; this.probing = false;
  }
  eligible(web, permission, details) {
    try {
      const origin = this.origin();
      return !this.ended && permission === 'clipboard-sanitized-write' &&
        validOrigin(origin) && details?.isMainFrame === true && details.requestingUrl === origin + '/' &&
        this.owns(web) && this.allowed();
    } catch (_) { return false; }
  }
  invalidate() { if (this.pending) this.pending(false); }
  end() { this.ended = true; this.invalidate(); }
  request(web, permission, callback, details) {
    // Never grant reads, ambient permission, non-owner/subframe requests, or a
    // second concurrent probe. Each callback is settled once, including timeout.
    if (this.probing || this.pending || !this.eligible(web, permission, details)) { callback(false); return; }
    let settled = false, timer;
    const until = this.now() + this.timeout;
    const finish = value => {
      if (settled) return;
      settled = true; clearTimeout(timer); this.pending = null;
      callback(value === true && this.now() < until && this.eligible(web, permission, details));
    };
    this.pending = finish;
    this.probing = true;
    timer = setTimeout(() => finish(false), this.timeout);
    Promise.resolve().then(() => settled ? false : this.probe(web)).then(
      value => { this.probing = false; finish(value === true); },
      () => { this.probing = false; finish(false); });
  }
}
module.exports = { ClipboardController, activationProbe };
