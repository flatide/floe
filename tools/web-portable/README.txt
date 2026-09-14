floe2-web portable — opt-in web preview, not the GTK package
==========================================================

Unpack this archive into a NEW directory, then:

  sh verify.sh
  ./floe2-web selfcheck --adjacent
  ./floe2-web view /path/to/design.oas --no-open
  ./floe2-web view /path/to/deck.jb --level 1,3

The runtime consists of three Rust executables and embedded UI/font assets.
No Python, GTK, KLayout, Node, browser or package manager is bundled or started
by verification/selfcheck. Firefox must already be installed to open a window;
--no-open keeps browser launch separate. The existing GTK launcher is unchanged.

BUILD.txt records the target, compiler, source stamp, and whether native runtime
checks actually ran. ELF.txt records interpreter, needed libraries and GLIBC
symbol-version requirements. Cross-assembly is NOT Linux execution acceptance.
On GNU builds, the system must provide the listed libraries and GLIBC versions.
On musl builds, absence of interpreter/DT_NEEDED is checked, but this does not
prove compatibility with every Linux kernel or machine. Check on your host.

Selfcheck tests indexer version and renderd startup/shutdown, not pixels or
Firefox/ETX/NFS behavior. It opens no design, listener or browser. --adjacent
ignores native tool overrides/dev/PATH and tests only this directory's binaries.
It also validates the compiled NOTICE-INDEX.json identity/structure, not every
notice chunk or the complete package hashes. --metadata-only reads no files.
Normal commands preserve explicit FLOE_INDEX_BIN/FLOE_RENDERD_BIN overrides;
an invalid override remains an error. Remove stale overrides before normal use.
Installation paths may contain spaces; native worker TMPDIR may NOT contain
whitespace/control characters. Use a private, writable, local temp location.

  ./floe2-web selfcheck --metadata-only
  ./floe2-web --version
  ./floe2-web index --help

verify.sh requires sha256sum (or shasum) and checks every shipped regular file
listed in SHA256SUMS. Checksums detect corruption, not malicious replacement or
publisher identity: verify the archive hash through a trusted channel first.
Do not add/change files inside this bundle; keep datasets and outputs elsewhere.

NOTICES contains original resolved native/build dependency manifests/notices
(build dependencies are included, not all are runtime-linked), Cargo.lock, the font
notice and installed Rust toolchain copyright/license documents. Optional extra
notices are identified separately. This inventory does not grant rights to Floe
or certify all downstream distribution obligations; its source manifests retain
LicenseRef-Flatide-Proprietary. Review distribution terms for the actual build.

About can read this package's original notices: 64 catalogue entries and one
UTF-8-aligned chunk of at most 64 KiB at a time, with page navigation/retry.
HTML is displayed as source text; non-UTF-8 material is explicitly shown as hex.
NOTICE-INDEX.json is pinned into this executable; each requested chunk is checked.
Missing/changed files are not displayed. Repair the installation and restart if
the catalogue is unavailable. The SHA-1 content ID is NOT publisher authentication
and does not replace verify.sh or trusted distribution checks. Older/development
executables without this compiled catalogue show only the embedded font notice.

Do not expose this loopback preview as an unauthenticated remote service.
Field Firefox/ETX validation, the remaining UI parity and GTK retirement remain
separate acceptance gates. See docs/WEBUI_PLAN.ko.md in the source repository.
