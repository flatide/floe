'use strict';
// UI-only confirmation. The Rust session endpoint remains the normal authority.
class CloseController {
  constructor({ reveal, openDialog, confirmForce, cancelService, timeout = 5000 }) {
    Object.assign(this, { reveal, openDialog, confirmForce, cancelService, timeout });
    this.epoch = 0;
    this.pending = false;
    this.ended = false;
  }
  invalidate() { this.epoch++; this.pending = false; }
  end() { this.ended = true; this.invalidate(); }
  async request() {
    if (this.ended) return;
    this.reveal();
    if (this.pending) return;
    const epoch = ++this.epoch;
    this.pending = true;
    let timer;
    const current = () => !this.ended && epoch === this.epoch;
    try {
      const result = await Promise.race([
        Promise.resolve().then(() => this.openDialog()).catch(() => 'unavailable'),
        new Promise(resolve => { timer = setTimeout(() => resolve('timeout'), this.timeout); })
      ]);
      if (current() && result !== 'opened' && await this.confirmForce() && current()) {
        this.cancelService();
      }
    } finally {
      clearTimeout(timer);
      if (current()) this.pending = false;
    }
  }
}
module.exports = { CloseController };
