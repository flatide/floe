Floe2 Electron OFFLINE DEVELOPMENT COMPARISON

Extract into a new directory. Run from a terminal:
  sh /absolute/path/floe2-electron-comparison/verify.sh
  /absolute/path/floe2-electron-comparison/floe2-electron view ./design.oas

The caller's working directory is preserved. Run the top-level launcher, NOT
runtime/Electron.app with Finder/open, and do not move that app out of the bundle.
This directory includes the pinned, fuse-hardened Electron runtime, five matched
release Rust executables (including the CLI), and the explicit JS/shared-UI file closure. No Python,
separate Node install, repository, Cargo, KLayout or external browser is needed
to launch. verify.sh uses the OS sha256sum or shasum utility; it is not an automatic
per-launch scan. System graphics/window/audio/security libraries are still needed.

Linux x64 + TeeBox/ETX standalone (NOT the deferred public server):
  /absolute/path/floe2-electron-comparison/floe2-teebox --check
  /absolute/path/floe2-electron-comparison/floe2-teebox view /shared/chip.oas --root /shared
  /absolute/path/floe2-electron-comparison/floe2-cli index /shared/mask.jb --jobs 4
The TeeBox entry inherits DISPLAY/XAUTHORITY and caller cwd, explicitly selects
X11, requires non-root and uses only this bundle's binaries. It does not detach,
create a login identity, index implicitly or change security settings. --check
validates files/native versions and Chromium ldd dependencies but does not prove
X11 access or sandbox/ETX execution. All binary overrides must be unset.
Same UID+DISPLAY reuses the default window; --multi and explicit process options
start independently. DISPLAY/reviewer is NOT authentication or OS isolation.
Headless floe2-cli is the matching Rust floe2-web CLI; help retains that name.
Indexing shared sources is explicit; coordinate rebuilds before using --force.

FLOE_ELECTRON_BIN, FLOE_ELECTRON_SERVICE_BIN, FLOE_ELECTRON_DOWNLOAD_BIN,
FLOE_INDEX_BIN and FLOE_RENDERD_BIN are explicit developer overrides. Unset them
to test the bundled binaries. Invalid overrides are errors, not fallback requests.
Unset ELECTRON_RUN_AS_NODE, NODE_OPTIONS, NODE_PATH and NODE_EXTRA_CA_CERTS.
The private runtime copy disables RunAsNode, NodeOptions, NodeCliInspect and
FileProtocolExtraPrivileges fuses. verify.sh checks the actual fuse wire, not only
the manifest. macOS staging copies are ad-hoc re-signed (no signing identity or
notarization). This is still a development bundle: app.asar-only loading and ASAR
integrity are NOT enabled; application source remains a separate directory.
This is not a security boundary against a local user replacing the app or verifier.

BUNDLE.json records source revision, native build target, runtime archive SHA-256,
regular-file hashes, permissions and internal relative symlinks. Verification
detects accidental changes, not hostile replacement of both data and verifier.
Archive SHA-256 is a change/provenance record, not a publisher signature. BUILD.txt
separates static checks from GUI acceptance. Build does not open a window or access
designs, clipboard, reviews, defaults or external servers. No auto-update is added.

NOTICES/ contains resolved Rust/build/font/toolchain notice inventories. The FULL
official runtime file set is retained, including runtime/LICENSE and
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
