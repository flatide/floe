'use strict';
// Electron installs native shutdown handlers after evaluating main.cjs. Re-arm
// our libuv watchers at ready; merely adding a second JS listener does not do it.
// Only our listeners are removed. This never changes the menu/window confirmation.
function terminationSignals(target, callback) {
  const handlers = new Map(['SIGINT', 'SIGTERM'].map(signal => [signal, () => callback(signal)]));
  const rearm = () => {
    for (const [signal, handler] of handlers) {
      target.removeListener(signal, handler);
      target.on(signal, handler);
    }
  };
  rearm();
  return rearm;
}
module.exports = { terminationSignals };
