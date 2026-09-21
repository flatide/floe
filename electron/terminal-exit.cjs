'use strict';
// A failed session may leave a diagnostic window, never a live Rust session.
// Closing it acknowledges the error but cannot bypass download/worker cleanup.
class TerminalExit {
  constructor(exit) {
    this.exit = exit; this.started = false; this.hold = false;
    this.requested = false; this.cleaned = false; this.exited = false;
  }
  begin(hold) { this.started = true; this.hold = hold; }
  request() { this.requested = true; this.flush(); }
  complete(code) { this.code = code; this.cleaned = true; this.flush(); }
  flush() {
    if (this.started && this.cleaned && (!this.hold || this.requested) && !this.exited) {
      this.exited = true; this.exit(this.code);
    }
  }
}
module.exports = { TerminalExit };
