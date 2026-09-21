'use strict';
const { EventEmitter } = require('node:events');
const { spawn } = require('node:child_process');
const path = require('node:path');
const fs = require('node:fs');
class DownloadSlot extends EventEmitter {
  constructor(binary, directory, env = process.env) {
    super();
    this.phase = 'starting'; this.buffer = Buffer.alloc(0); this.result = null; this.stopping = false;
    this.ready = new Promise((resolve, reject) => { this.resolveReady = resolve; this.rejectReady = reject; });
    this.ready.catch(() => {}); // always observed, including shutdown before ready
    this.finished = new Promise(resolve => { this.resolveFinished = resolve; });
    this.directory = fs.realpathSync(directory);
    this.child = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'], env, shell: false });
    this.child.once('error', () => this.fail());
    this.child.stdin.on('error', () => this.fail());
    this.child.stdout.on('error', () => this.fail());
    this.child.stderr.on('error', () => this.fail());
    this.child.stderr.on('data', () => {});
    this.child.stdout.on('data', bytes => this.receive(bytes));
    this.child.once('close', code => {
      this.phase = 'ended'; clearTimeout(this.timer);
      this.rejectReady(new Error('Download slot ended'));
      const result = !this.failed && this.buffer.length === 0 && this.result || { publication: 'unconfirmed', cleanup: false };
      if (code !== 0) result.cleanup = false;
      this.resolveFinished(result); this.emit('ended', result);
    });
    this.timer = setTimeout(() => this.fail(), 30000);
    this.send({ directory: this.directory });
  }
  send(fields) {
    const bytes = Buffer.from(JSON.stringify({ v: 1, ...fields }) + '\n');
    if (bytes.length > 65537) throw new Error('Download request too large');
    this.child.stdin.write(bytes);
  }
  receive(bytes) {
    this.buffer = Buffer.concat([this.buffer, bytes]);
    if (this.buffer.length > 65537) return this.fail();
    let at;
    while ((at = this.buffer.indexOf(10)) >= 0) {
      let v;
      try { v = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(this.buffer.subarray(0, at))); }
      catch (_) { return this.fail(); }
      this.buffer = this.buffer.subarray(at + 1);
      if (!v || v.v !== 1) return this.fail();
      const keys = Object.keys(v).sort().join(',');
      if (v.event === 'ready' && keys === 'event,path,v' && this.phase === 'starting' && typeof v.path === 'string' &&
          path.dirname(path.dirname(v.path)) === this.directory && path.basename(v.path) === 'payload' &&
          /^\.floe-download-[a-f0-9]{32}$/.test(path.basename(path.dirname(v.path)))) {
        clearTimeout(this.timer); this.phase = 'ready'; this.path = v.path; this.resolveReady(this); continue;
      }
      if (v.event === 'limit' && keys === 'event,v' && ['receiving', 'publishing'].includes(this.phase)) { this.emit('limit'); continue; }
      if (v.event === 'done' && keys === 'cleanup,event,publication,v' && !this.result &&
          ['saved','unconfirmed','not_requested'].includes(v.publication) && typeof v.cleanup === 'boolean') {
        if (v.publication === 'saved' && this.phase !== 'publishing') return this.fail();
        this.result = { publication: v.publication, cleanup: v.cleanup }; continue;
      }
      return this.fail();
    }
  }
  fail() {
    if (this.phase === 'ended' || this.failed) return;
    this.failed = true; this.rejectReady(new Error('Download slot failed')); this.close(); this.emit('failure');
  }
  watch() {
    if (this.phase !== 'ready' || this.stopping) throw new Error('Download slot unavailable');
    this.phase = 'receiving'; this.send({ command: 'watch' });
  }
  publish(destination) {
    if (this.phase !== 'receiving' || this.stopping || !path.isAbsolute(destination)) throw new Error('Download publication unavailable');
    this.phase = 'publishing'; this.send({ publish: destination }); return this.finished;
  }
  close() {
    if (this.phase === 'ended' || this.stopping) return;
    this.stopping = true; this.child.stdin.end();
  }
  retain() {
    if (this.phase === 'ended' || this.stopping) return;
    this.send({ command: 'retain' }); this.stopping = true;
    // Keep the pipe open until the explicit retained/unclean acknowledgement.
  }
}
module.exports = { DownloadSlot };
