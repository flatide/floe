Floe2 Electron OFFLINE DEVELOPMENT COMPARISON

Extract into a new directory. Run from a terminal:
  sh /absolute/path/floe2-electron-comparison/verify.sh
  /absolute/path/floe2-electron-comparison/floe2-electron view ./design.oas

The caller's working directory is preserved. Run the top-level launcher, NOT
runtime/Electron.app with Finder/open, and do not move that app out of the bundle.
This directory includes the pinned, unmodified Electron runtime, four matched
release Rust workers, and the explicit JS/shared-UI file closure. No Python,
separate Node install, repository, Cargo, KLayout or external browser is needed
to launch. verify.sh uses the OS sha256sum or shasum utility; it is not an automatic
per-launch scan. System graphics/window/audio/security libraries are still needed.

FLOE_ELECTRON_BIN, FLOE_ELECTRON_SERVICE_BIN, FLOE_ELECTRON_DOWNLOAD_BIN,
FLOE_INDEX_BIN and FLOE_RENDERD_BIN are explicit developer overrides. Unset them
to test the bundled binaries. Invalid overrides are errors, not fallback requests.
Unset ELECTRON_RUN_AS_NODE, NODE_OPTIONS and NODE_PATH before using the launcher.
The bundled runtime is unchanged: its upstream development fuses are NOT hardened
or a security boundary against a local user modifying/launching its files.

BUNDLE.json records source revision, native build target, runtime archive SHA-256,
regular-file hashes, permissions and internal relative symlinks. Verification
detects accidental changes, not hostile replacement of both data and verifier.
Archive SHA-256 is a change/provenance record, not a publisher signature. BUILD.txt
separates static checks from GUI acceptance. Build does not open a window or access
designs, clipboard, reviews, defaults or external servers. No auto-update is added.

NOTICES/ contains resolved Rust/build/font/toolchain notice inventories. The FULL
official runtime is retained, including runtime/LICENSE and
runtime/LICENSES.chromium.html. This inventory is not legal distribution clearance.
No rebranding, Floe signing/notarization, installer or Finder association is done.

RHEL 8.6/8.10 + ETX: candidate only. Verify actual OS dependencies, sandbox/user
namespace policy, X11/DPI/input/IME, process cleanup and per-user memory on site.
Never use --no-sandbox, change setuid modes, or disable security as a workaround.
The prebuilt Linux runtime retains its ordinary archive permissions; the packager
does not configure chrome-sandbox. Rust GNU executables must pass GLIBC <= 2.28;
this does not prove the Chromium dependency closure or Linux/ETX execution.

Only explicitly requested --smoke-* flags run synthetic UI QA. Clipboard QA
overwrites the OS clipboard and requires separate user consent. Ordinary launch,
packaging and verify.sh never run those tests.
