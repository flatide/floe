'use strict';
// Native UI lifecycle only. This controller never creates a session, replays
// authentication, or issues a save/index request. load is one approved root GET.
class RecoveryController {
  constructor({ allowed, reveal, confirm, load, probe, result,
    now = () => performance.now(), setTimer = setTimeout, clearTimer = clearTimeout,
    timeout = 30000, interval = 500 }) {
    Object.assign(this, { allowed, reveal, confirm, load, probe, result, now, setTimer, clearTimer, timeout, interval });
    this.epoch = 0; this.phase = 'idle'; this.ended = false;
    this.deadlineTimer = null; this.probeTimer = null;
    this.lastResult = null; this.expectedNavigation = false;
  }
  get busy() { return this.phase !== 'idle'; }
  current(id) { return !this.ended && this.busy && this.epoch === id; }
  invalidate() {
    this.epoch++; this.phase = 'idle'; this.expectedNavigation = false;
    this.clearTimer(this.deadlineTimer); this.clearTimer(this.probeTimer);
    this.deadlineTimer = this.probeTimer = null;
  }
  end() { this.ended = true; this.invalidate(); }
  navigation() {
    if (this.phase === 'loading' && this.expectedNavigation) this.expectedNavigation = false;
    else this.invalidate();
  }
  finish(id, marker) {
    if (!this.current(id) || this.phase === 'prompting') return;
    if (this.now() >= this.until) marker = 'timeout';
    this.invalidate(); this.lastResult = marker;
    const noticeEpoch = this.epoch;
    // A closed UI must not reject an unobserved asynchronous notification.
    Promise.resolve().then(() => {
      if (!this.ended && this.epoch === noticeEpoch) return this.result(marker);
    }).catch(() => {});
  }
  expired(id) {
    if (!this.current(id)) return true;
    if (this.now() >= this.until) { this.finish(id, 'timeout'); return true; }
    return false;
  }
  async request() {
    if (this.ended || this.busy || !this.allowed()) return false;
    this.reveal(); this.phase = 'prompting'; const id = ++this.epoch;
    let accepted = false;
    try { accepted = await this.confirm(); } catch (_) {}
    if (!this.current(id)) return false;
    if (accepted !== true || !this.allowed()) { this.invalidate(); return false; }
    this.phase = 'loading'; this.lastResult = null; this.until = this.now() + this.timeout;
    this.expectedNavigation = true;
    this.deadlineTimer = this.setTimer(() => this.finish(id, 'timeout'), this.timeout);
    Promise.resolve().then(() => {
      if (!this.expired(id)) return this.load();
    }).then(() => {
      if (this.expired(id)) return;
      this.expectedNavigation = false; this.phase = 'checking'; this.poll(id);
    }, () => this.finish(id, 'load-failed'));
    return true;
  }
  poll(id) {
    if (this.expired(id) || this.phase !== 'checking') return;
    // One probe at a time per attempt. The independent deadline covers a promise that
    // never resolves, not merely a sequence of completed-but-unready probes.
    Promise.resolve().then(() => {
      if (!this.expired(id) && this.phase === 'checking') return this.probe();
    }).catch(() => 'waiting').then(marker => {
      if (this.expired(id)) return;
      if (['ready', 'ready-hidden', 'restart-required'].includes(marker)) { this.finish(id, marker); return; }
      this.probeTimer = this.setTimer(() => this.poll(id), this.interval);
    });
  }
}
module.exports = { RecoveryController };
