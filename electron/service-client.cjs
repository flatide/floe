'use strict';
const { EventEmitter } = require('node:events');
const { spawn } = require('node:child_process');
const { isAbsolute } = require('node:path');
const MAX_LINE = 65536;

function decodeFrame(bytes) {
  // Never include a wire line or credential in a parser exception.
  let value;
  try { value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)); }
  catch (_) { throw new Error('Invalid service frame'); }
  if (!value || Array.isArray(value) || value.v !== 1) throw new Error('Invalid service frame');
  const keys = Object.keys(value).sort().join(',');
  if (['starting', 'forwarded', 'directory', 'present'].includes(value.event) && keys === 'event,v') return value;
  if (value.event !== 'ready' || keys !== 'event,origin,url,v' ||
      typeof value.origin !== 'string' || typeof value.url !== 'string') {
    throw new Error('Invalid service frame');
  }
  const match = /^http:\/\/127\.0\.0\.1:([1-9][0-9]{0,4})$/.exec(value.origin);
  if (!match || Number(match[1]) > 65535 ||
      !value.url.startsWith(value.origin + '/#bootstrap=') ||
      !/^[a-f0-9]{64}$/.test(value.url.slice((value.origin + '/#bootstrap=').length))) {
    throw new Error('Invalid service frame');
  }
  return value;
}

function encodeFrame(value) {
  const bytes = Buffer.from(JSON.stringify(value) + '\n');
  if (bytes.length > MAX_LINE + 1) throw new Error('Host request too large');
  return bytes;
}

class ServiceClient extends EventEmitter {
  constructor(binary, args, env = process.env) {
    super();
    // Validate before spawning: no orphan child on serialization/size failure.
    const initial = encodeFrame({ v: 1, args });
    this.phase = 'starting';
    this.buffer = Buffer.alloc(0);
    this.closing = false;
    this.failed = false;
    this.child = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'], env, shell: false });
    this.finished = new Promise(resolve => {
      this.child.once('close', (code, signal) => {
        this.phase = 'ended';
        this.buffer = Buffer.alloc(0);
        resolve({ code, signal });
        this.emit('ended', { code, signal });
      });
    });
    this.child.once('error', () => this.fail());
    this.child.stdin.on('error', () => this.fail());
    this.child.stdout.on('error', () => this.fail());
    // Drain, but don't log raw service diagnostics or arbitrary user paths.
    this.child.stderr.on('data', () => {});
    this.child.stdout.on('data', bytes => this.receive(bytes));
    this.child.stdin.write(initial);
  }
  fail() {
    if (this.failed || this.phase === 'ended') return;
    this.failed = true;
    this.close();
    this.emit('failure', new Error('Rust service connection failed'));
  }
  receive(bytes) {
    if (this.closing) return;
    this.buffer = Buffer.concat([this.buffer, bytes]);
    if (this.buffer.length > MAX_LINE + 1) return this.fail();
    let at;
    while ((at = this.buffer.indexOf(10)) !== -1) {
      const line = this.buffer.subarray(0, at);
      this.buffer = this.buffer.subarray(at + 1);
      let frame;
      try { frame = decodeFrame(line); }
      catch (_) { return this.fail(); }
      if (frame.event === 'starting' && this.phase === 'starting') {
        this.phase = 'owner';
        this.emit('starting');
      } else if (frame.event === 'forwarded' && this.phase === 'starting') {
        this.phase = 'forwarded';
        this.emit('forwarded');
      } else if (frame.event === 'present' && this.phase === 'ready') {
        this.emit('present');
      } else if (frame.event === 'directory' && this.phase === 'owner') {
        this.phase = 'directory';
        this.emit('directory');
      } else if (frame.event === 'ready' && ['owner', 'waiting-ready'].includes(this.phase)) {
        this.phase = 'ready';
        this.emit('ready', frame);
      } else {
        return this.fail();
      }
    }
  }
  chooseDirectory(directory) {
    if (this.closing || this.phase !== 'directory' || typeof directory !== 'string' ||
        !isAbsolute(directory) || directory.includes('\0')) throw new Error('Invalid folder choice');
    const message = encodeFrame({ v: 1, directory });
    this.phase = 'waiting-ready';
    this.child.stdin.write(message);
  }
  close() {
    if (this.closing || this.phase === 'ended') return;
    this.closing = true;
    this.buffer = Buffer.alloc(0);
    // EOF is the lifetime authority, including a close before startup/folder choice.
    this.child.stdin.end();
  }
}

module.exports = { decodeFrame, encodeFrame, ServiceClient };
