'use strict';
function validOrigin(origin) {
  if (typeof origin !== 'string') return false;
  const m = /^http:\/\/127\.0\.0\.1:([1-9][0-9]{0,4})$/.exec(origin);
  return !!m && Number(m[1]) <= 65535;
}
function navigationAllowed(origin, url, mainFrame = true) {
  return mainFrame && validOrigin(origin) && typeof url === 'string' &&
    (url === origin + '/' || url.startsWith(origin + '/#'));
}
function requestAllowed(origin, url) {
  if (!validOrigin(origin) || typeof url !== 'string') return false;
  return url.startsWith(origin + '/') || url.startsWith(origin.replace('http:', 'ws:') + '/');
}
function webPreferences(partition) {
  return {
    partition, sandbox: true, contextIsolation: true, nodeIntegration: false,
    nodeIntegrationInWorker: false, nodeIntegrationInSubFrames: false,
    webviewTag: false, webSecurity: true, allowRunningInsecureContent: false,
    navigateOnDragDrop: false, safeDialogs: true, devTools: false, spellcheck: false
  };
}
function serviceBinary(env, fallback) {
  if (!Object.hasOwn(env, 'FLOE_ELECTRON_SERVICE_BIN')) return fallback;
  const value = env.FLOE_ELECTRON_SERVICE_BIN;
  if (typeof value !== 'string' || !value || value.includes('\0')) throw new Error('Invalid service override');
  return value;
}
module.exports = { validOrigin, navigationAllowed, requestAllowed, webPreferences, serviceBinary };
