'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { randomUUID } = require('node:crypto');
const P = require('./policy.cjs');

// This origin is intercepted in a private, cookie-less session, never fetched.
const ORIGIN = 'https://floe-notices.invalid';
const ROUTES = ['/', '/electron', '/chromium'];
const LIMIT = 32 * 1024 * 1024;
const CSP = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; sandbox";
const STYLE = '<style>body{font:15px system-ui;margin:24px;line-height:1.5}pre{white-space:pre-wrap;overflow-wrap:anywhere}.product{border-top:1px solid #888;padding:18px 0}.title{font-weight:bold}.license{display:block}a{overflow-wrap:anywhere}</style>';
function escape(text) { return text.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c])); }
function route(url) {
  if (typeof url !== 'string') return null;
  // No query, encoded path, user info, alternate host, port, or fragment aliases.
  return ROUTES.find(r => url === ORIGIN + r) ?? null;
}
function runtimeRoot(executable, platform) {
  if (!path.isAbsolute(executable) || !['darwin', 'linux'].includes(platform)) throw new Error('Unsupported runtime');
  return platform === 'darwin' ? path.resolve(path.dirname(executable), '../../..') : path.dirname(executable);
}
async function readNotice(root, name) {
  if (!['LICENSE', 'LICENSES.chromium.html'].includes(name)) throw new Error('Unknown notice');
  const f = await fs.promises.open(path.join(root, name), fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
  try {
    const before = await f.stat();
    if (!before.isFile() || before.nlink !== 1 || before.size < 1 || before.size > LIMIT) throw new Error('Invalid notice');
    const buffer = Buffer.alloc(before.size + 1);
    let size = 0;
    while (size < buffer.length) {
      const { bytesRead } = await f.read(buffer, size, buffer.length - size, null);
      if (!bytesRead) break;
      size += bytesRead;
    }
    const after = await f.stat();
    if (size !== before.size || after.size !== before.size || after.mtimeMs !== before.mtimeMs || after.ctimeMs !== before.ctimeMs) throw new Error('Notice changed');
    return new TextDecoder('utf-8', { fatal: true }).decode(buffer.subarray(0, size));
  } finally { await f.close(); }
}
async function documentFor(root, url) {
  const selected = route(url);
  if (selected === '/') return '<!doctype html><meta charset="utf-8"><title>Open Source Licenses</title>' + STYLE +
    '<h1>Open Source Licenses</h1><p>Notices from the running Electron distribution. This viewer does not establish distribution compliance.</p>' +
    '<ul><li><a href="/electron">Electron license</a></li><li><a href="/chromium">Chromium and bundled third-party licenses</a></li></ul>' +
    '<p>Rust, font and toolchain notices are supplied separately in the comparison bundle’s NOTICES directory. External links, downloads and scripts are disabled here.</p>';
  if (selected === '/electron') return '<!doctype html><meta charset="utf-8"><title>Electron license</title>' + STYLE +
    '<nav><a href="/">Contents</a></nav><h1>Electron license</h1><pre>' + escape(await readNotice(root, 'LICENSE')) + '</pre>';
  if (selected === '/chromium') {
    const html = await readNotice(root, 'LICENSES.chromium.html');
    // Keep upstream notice text intact. Its chrome:// styles cannot be fetched;
    // a local wrapping style keeps the full license bodies readable offline.
    if (!html.includes('</head>')) throw new Error('Unknown credits format');
    return html.replace('</head>', STYLE + '</head>').replace('<body>', '<body><nav><a href="/">Contents</a></nav>');
  }
  throw new Error('Unknown notice route');
}

class Notices {
  constructor(electron, root) { this.electron = electron; this.root = root; this.window = null; }
  async open(parent) {
    if (this.window && !this.window.isDestroyed()) { this.window.show(); this.window.focus(); return; }
    const { BrowserWindow, session, Menu } = this.electron;
    const partition = 'floe-notices-' + randomUUID();
    const ses = session.fromPartition(partition, { cache: false });
    ses.setPermissionCheckHandler(() => false);
    ses.setPermissionRequestHandler((_web, _permission, callback) => callback(false));
    ses.on('will-download', event => event.preventDefault());
    let child;
    ses.webRequest.onBeforeRequest((details, callback) => callback({ cancel:
      !child || child.isDestroyed() || details.webContentsId !== child.webContents.id ||
      details.method !== 'GET' || details.resourceType !== 'mainFrame' || route(details.url) === null }));
    ses.protocol.handle('https', async request => {
      const headers = { 'Content-Type': 'text/html; charset=utf-8', 'Content-Security-Policy': CSP,
        'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', 'Referrer-Policy': 'no-referrer' };
      try {
        if (request.method !== 'GET' || route(request.url) === null || !child || child.isDestroyed()) throw new Error('Rejected');
        return new Response(await documentFor(this.root, request.url), { headers });
      } catch (_) {
        // No path/exception contents, directory listing or fallback file search.
        return new Response('<!doctype html><title>Notice unavailable</title><h1>Notice unavailable</h1><p>The running distribution’s notice is missing, invalid or changed. Check the complete verified runtime archive.</p>', { status: 404, headers });
      }
    });
    child = new BrowserWindow({ parent, modal: !!parent, show: true, width: 900, height: 720,
      title: 'Open Source Licenses', webPreferences: { ...P.webPreferences(partition),
        javascript: false, images: false, webgl: false, disableDialogs: true } });
    this.window = child;
    const closed = new Promise(resolve => child.once('closed', () => {
      if (this.window === child) this.window = null;
      ses.protocol.unhandle('https'); resolve();
    }));
    const web = child.webContents;
    web.setWindowOpenHandler(() => ({ action: 'deny' }));
    web.on('will-attach-webview', event => event.preventDefault());
    web.on('will-redirect', event => event.preventDefault());
    web.on('will-frame-navigate', event => { if (!event.isMainFrame || route(event.url) === null) event.preventDefault(); });
    web.on('will-navigate', event => { if (route(event.url) === null) event.preventDefault(); });
    web.on('before-input-event', (event, input) => {
      if (input.type === 'keyDown' && (input.key === 'Escape' || ((input.control || input.meta) && input.key.toLowerCase() === 'w'))) {
        event.preventDefault(); child.close();
      }
    });
    child.setMenu(Menu.buildFromTemplate([{ label: 'Notices', submenu: ROUTES.map((r, i) => ({
      label: ['Contents', 'Electron', 'Chromium'][i], click: () => { web.loadURL(ORIGIN + r).catch(() => {}); }
    })).concat([{ type: 'separator' }, { label: 'Close notices', click: () => child.close() }]) }]));
    try { await web.loadURL(ORIGIN + '/'); } catch (_) { child.destroy(); throw new Error('Notice view unavailable'); }
    await closed;
  }
  close() { if (this.window && !this.window.isDestroyed()) this.window.destroy(); }
}
module.exports = { Notices, runtimeRoot, readNotice, documentFor, route, ORIGIN, CSP, LIMIT };
