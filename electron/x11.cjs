'use strict';
// Before app.ready. No sandbox/GPU/security bypass and no DISPLAY rewriting.
function configureX11(commandLine, platform, env) {
  if (platform === 'linux' && env.FLOE_ELECTRON_X11 === '1') {
    commandLine.appendSwitch('ozone-platform', 'x11');
  }
}
module.exports = { configureX11 };
