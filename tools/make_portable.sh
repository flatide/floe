#!/bin/bash
# Build a self-contained floe2/floe runtime bundle for hosts without PyGObject.
# Adapted from flateyes' make_portable.sh - the GTK3 stack is pulled from
# conda-forge (relocatable) exactly as flateyes does; floe additionally
# needs NumPy + Pillow wheels. The default product is Rust-only floe2;
# FLOE_PORTABLE_KLAYOUT=1 builds the KLayout floe bundle with both `floe`
# (stable/KLayout) and `floe2` (Rust) launchers.
#
#   ./make_portable.sh [output-dir]     # -> floe2-portable-<ver>-<date>.tar.gz
#   ./make_portable.sh --print-name     # print the artifact name and stop
#
# The bundle holds python + PyGObject + GTK3 (conda-forge) + NumPy/Pillow +
# the shared floe implementation, floe2 shell, and matched Rust binaries,
# plus a launcher that builds
# the machine-local GTK caches on first run. Target: x86_64 Linux; verify
# computes the exact glibc floor and enforces the configured ceiling.
set -euo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)          # floe repo root
PRINT_NAME=0
if [ "${1:-}" = "--print-name" ]; then
    PRINT_NAME=1
    shift
fi
OUT_DIR=${1:-$REPO}
WORK=${FLOE_PORTABLE_WORK:-${TMPDIR:-/tmp}/floe-portable-build}
PY_SPEC=${PY_SPEC:-python=3.11}                 # conda-forge python line
# Two glibc knobs: keep the conda solver at the supported deployment
# baseline while independently checking every bundled ELF (including the
# wheels and Rust daemons). The measured floor is baked into selfcheck/README.
CONDA_GLIBC=${CONDA_GLIBC:-2.27}                # conda solver target
GLIBC_CEILING=${GLIBC_CEILING:-28}              # verify guard (RHEL8=2.28)
WHEELS=${WHEELS:-}                              # local wheel dir (closed net)
FLOE_PORTABLE_KLAYOUT=${FLOE_PORTABLE_KLAYOUT:-0}
case "$FLOE_PORTABLE_KLAYOUT" in
    0 | 1) ;;
    *) echo "FLOE_PORTABLE_KLAYOUT must be 0 or 1"; exit 1 ;;
esac
FLOE_PORTABLE_PRODUCT=${FLOE_PORTABLE_PRODUCT:-}
if [ -z "$FLOE_PORTABLE_PRODUCT" ]; then
    if [ "$FLOE_PORTABLE_KLAYOUT" = 1 ]; then
        FLOE_PORTABLE_PRODUCT=floe
    else
        FLOE_PORTABLE_PRODUCT=floe2
    fi
fi
case "$FLOE_PORTABLE_PRODUCT" in
    floe)
        # The stable product owns the KLayout renderer. Keep the historical
        # FLOE_PORTABLE_KLAYOUT=1 spelling as a compatible product selector.
        FLOE_PORTABLE_KLAYOUT=1
        ;;
    floe2)
        if [ "$FLOE_PORTABLE_KLAYOUT" = 1 ]; then
            echo "floe2 is Rust-only; do not combine FLOE_PORTABLE_PRODUCT=floe2 with KLayout"
            exit 1
        fi
        ;;
    *) echo "FLOE_PORTABLE_PRODUCT must be floe or floe2"; exit 1 ;;
esac
VERSION=$(sed -n \
    's/^__version__[[:space:]]*=[[:space:]]*"\([^"]*\)".*$/\1/p' \
    "$REPO/floe/__init__.py")
[ -n "$VERSION" ] || {
    echo "could not read floe version from floe/__init__.py"; exit 1; }
STAMP=$(date +%Y%m%d)
FLAVOR=""
if [ "$FLOE_PORTABLE_KLAYOUT" = 1 ]; then
    FLAVOR="-klayout"
fi
NAME="${FLOE_PORTABLE_PRODUCT}-portable-${VERSION}-${STAMP}${FLAVOR}"
if [ "$PRINT_NAME" = 1 ]; then
    printf '%s\n' "$NAME"
    exit 0
fi

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) : ;;
    *) echo "floe's bundle needs numpy/pillow Linux wheels installed by the"
       echo "runtime's pip, so build on an x86_64 Linux machine (got"
       echo "$(uname -s)-$(uname -m))."; exit 1 ;;
esac

echo "== workdir: $WORK"
rm -rf "$WORK"; mkdir -p "$WORK"; cd "$WORK"

# -- 1. micromamba (static) ---------------------------------------------
curl -fsSL -o micromamba \
    "https://github.com/mamba-org/micromamba-releases/releases/latest/download/micromamba-linux-64"
chmod +x micromamba

# -- 2. resolve the GTK3 + python runtime (conda-forge) ------------------
# CONDA_OVERRIDE_GLIBC keeps the GTK solver at a broadly-compatible
# baseline (the effective floor is raised by the pip wheels anyway).
# librsvg = the SVG gdk-pixbuf loader Adwaita's symbolic icons (checkbox
# check-symbolic.svg etc.) need; a bundled font gives GTK a sans-serif.
#
# (The "images render black" saga was NOT a conda version regression -
# the broken and working bundles had byte-identical lib versions. The
# real culprit was XQuartz's XRender; see the CAIRO_DEBUG fallback the
# launcher sets. GTK_PINS stays as an escape hatch, empty by default.)
GTK_PINS=${GTK_PINS:-}
# shellcheck disable=SC2086  # GTK_PINS is a word list on purpose
CONDA_OVERRIDE_GLIBC=$CONDA_GLIBC ./micromamba create -y \
    -r "$WORK/mmroot" -p "$WORK/runtime" --platform linux-64 \
    -c conda-forge "$PY_SPEC" pygobject gtk3 librsvg \
    font-ttf-ubuntu font-ttf-dejavu-sans-mono $GTK_PINS \
    || true   # post-link failures still exit nonzero on some versions
echo "== core rendering libs in the runtime:"
ls "$WORK"/runtime/lib 2>/dev/null | grep -E \
    "^(libgtk-3|libcairo|libgdk_pixbuf-2.0|libglib-2.0|libpango-1.0|libpixman-1|libX11|libXrender|libxcb)\.so\.[0-9.]+$"
PYBIN="$(ls "$WORK"/runtime/bin/python3.[0-9]* 2>/dev/null | head -1)"
[ -x "$PYBIN" ] || { echo "runtime extraction failed"; exit 1; }
PYVER="$("$PYBIN" -c 'import sys;print("%d.%d"%sys.version_info[:2])')"
echo "== runtime python $PYVER"

# -- 3. add NumPy/Pillow; KLayout only for an opt-in rollback bundle ---
"$PYBIN" -m pip install --upgrade pip >/dev/null
PIP_PACKAGES=(numpy pillow)
if [ "$FLOE_PORTABLE_KLAYOUT" = 1 ]; then
    PIP_PACKAGES+=(klayout)
    echo "== KLayout rollback bundle requested"
fi
if [ -n "$WHEELS" ]; then
    echo "== installing runtime wheels from local directory: $WHEELS"
    "$PYBIN" -m pip install --no-index --find-links "$WHEELS" \
        --only-binary=:all: "${PIP_PACKAGES[@]}"
else
    "$PYBIN" -m pip install --only-binary=:all: "${PIP_PACKAGES[@]}"
fi
"$PYBIN" -c 'import numpy, PIL; print("== numpy", numpy.__version__, "pillow", PIL.__version__)'
if [ "$FLOE_PORTABLE_KLAYOUT" = 1 ]; then
    "$PYBIN" -c 'import klayout; print("== klayout rollback", klayout.__version__)'
fi

# -- 4. drop the shared implementation + product shell into site-packages
SITE="$("$PYBIN" -c 'import site;print(site.getsitepackages()[0])')"
rm -rf "$SITE/floe"; cp -r "$REPO/floe" "$SITE/floe"
rm -rf "$SITE/floe2"; cp -r "$REPO/floe2" "$SITE/floe2"
find "$SITE/floe" -name '__pycache__' -type d -prune -exec rm -rf {} +
find "$SITE/floe2" -name '__pycache__' -type d -prune -exec rm -rf {} +

# -- 5. slim: build-time payloads never touched at runtime --------------
cd "$WORK/runtime"
rm -rf include share/gir-1.0 share/locale share/doc share/man \
       share/gtk-doc share/cups share/terminfo share/zoneinfo \
       share/aclocal share/bash-completion conda-meta compiler_compat \
       x86_64-conda-linux-gnu sbin lib/pkgconfig lib/cmake \
       "lib/python$PYVER/test" "lib/python$PYVER/idlelib" \
       "lib/python$PYVER/ensurepip" "lib/python$PYVER/lib2to3" \
       "lib/python$PYVER/turtledemo" "lib/python$PYVER/tkinter"
rm -rf "lib/python$PYVER/site-packages/pip" \
       "lib/python$PYVER/site-packages/setuptools" \
       "lib/python$PYVER/site-packages/wheel" \
       "lib/python$PYVER"/site-packages/*.dist-info \
       "lib/python$PYVER"/site-packages/*.egg-info
find . -name "__pycache__" -type d -prune -exec rm -rf {} +
find lib -name "*.a" -delete
cd bin
for f in *; do
    case "$f" in
        python3 | python"$PYVER" | glib-compile-schemas \
            | gdk-pixbuf-query-loaders | gtk-update-icon-cache) ;;
        *) rm -rf "$f" ;;
    esac
done
cd "$WORK"

# -- 5b. Rust runtime binaries next to the interpreter ------------------
# vfsclient.find_binary()'s portable slot is NEXT TO sys.executable =
# runtime/bin/floe-index. Without it the viewer still OPENS (the python
# side is complete) but every render waits forever on a vfsd that never
# spawns, and indexing / DRC --pack cannot run at all (field 2026-08-21).
# The Python package and both binaries are a matched set (pack/OVM/protocol
# version discipline): always ship and update them together. Portable defaults
# to static musl binaries. A normal `cargo build --release` inherits the build
# host's glibc floor (for example GLIBC_2.35 on Ubuntu 22.04) and cannot satisfy
# the RHEL8/2.28 deployment ceiling even though the bundled GTK runtime can.
# Explicit overrides remain available for already-built, compatible binaries,
# but must be supplied as a pair.
MUSL_TARGET=x86_64-unknown-linux-musl
if [ -n "${FLOE_INDEX_BIN:-}" ] || [ -n "${FLOE_RENDERD_BIN:-}" ]; then
    if [ -z "${FLOE_INDEX_BIN:-}" ] || [ -z "${FLOE_RENDERD_BIN:-}" ]; then
        echo "FLOE_INDEX_BIN and FLOE_RENDERD_BIN must be specified together"
        exit 1
    fi
else
    FLOE_INDEX_BIN="$REPO/rust/target/$MUSL_TARGET/release/floe-index"
    FLOE_RENDERD_BIN="$REPO/rust/target/$MUSL_TARGET/release/floe-renderd"
    if command -v cargo >/dev/null; then
        if command -v rustup >/dev/null && \
           ! rustup target list --installed | grep -qx "$MUSL_TARGET"; then
            echo "== installing Rust target $MUSL_TARGET"
            rustup target add "$MUSL_TARGET"
        fi
        echo "== building static Rust runtime ($MUSL_TARGET, release)"
        if ! (cd "$REPO/rust" && cargo build --release \
            --target "$MUSL_TARGET" -p floe-index -p floe-renderd); then
            echo "static Rust build failed; install the $MUSL_TARGET target"
            echo "or specify both FLOE_INDEX_BIN and FLOE_RENDERD_BIN"
            exit 1
        fi
    elif [ -x "$REPO/rust/dist/floe-index-linux-x86_64" ] && \
         [ -x "$REPO/rust/dist/floe-renderd-linux-x86_64" ]; then
        FLOE_INDEX_BIN="$REPO/rust/dist/floe-index-linux-x86_64"
        FLOE_RENDERD_BIN="$REPO/rust/dist/floe-renderd-linux-x86_64"
        echo "== using prebuilt static Rust runtime from rust/dist"
    fi
fi
[ -x "$FLOE_INDEX_BIN" ] || {
    echo "floe-index binary not found ($FLOE_INDEX_BIN):"
    echo "install cargo + the $MUSL_TARGET target, or point both"
    echo "FLOE_INDEX_BIN and FLOE_RENDERD_BIN at Linux x86_64 builds"; exit 1; }
[ -x "$FLOE_RENDERD_BIN" ] || {
    echo "floe-renderd binary not found ($FLOE_RENDERD_BIN):"
    echo "install cargo + the $MUSL_TARGET target, or point both"
    echo "FLOE_INDEX_BIN and FLOE_RENDERD_BIN at Linux x86_64 builds"; exit 1; }
cp "$FLOE_INDEX_BIN" "$WORK/runtime/bin/floe-index"
cp "$FLOE_RENDERD_BIN" "$WORK/runtime/bin/floe-renderd"
chmod +x "$WORK/runtime/bin/floe-index" "$WORK/runtime/bin/floe-renderd"

# -- 6. verify: arch, glibc floor <= ceiling, key files -----------------
# The real host requirement is the MAX GLIBC_2.x any bundled ELF needs
# (wheels or Rust binaries); ceiling only guards against accidentally-newer
# deps. The floor is written to $WORK/floor.txt so the
# launcher/README can state the true "glibc >= 2.x" the target must meet.
python3 - "$WORK/runtime" "$GLIBC_CEILING" "$PYVER" "$WORK/floor.txt" \
    "$FLOE_PORTABLE_KLAYOUT" <<'PY'
import os, re, struct, sys
root, ceiling, pyver, floorf = sys.argv[1], int(sys.argv[2]), \
    sys.argv[3], sys.argv[4]
with_klayout = sys.argv[5] == "1"
pat = re.compile(rb"GLIBC_2\.(\d+)")
bad, elves, floor = [], 0, 0
for dp, _, names in os.walk(root):
    for n in names:
        p = os.path.join(dp, n)
        try:
            with open(p, "rb") as f:
                head = f.read(20)
                if head[:4] != b"\x7fELF":
                    continue
                elves += 1
                if struct.unpack("<H", head[18:20])[0] != 0x3E:
                    bad.append(("arch", p)); continue
                f.seek(0)
                vs = [int(m.group(1)) for m in pat.finditer(f.read())]
        except OSError:
            continue
        if vs:
            floor = max(floor, max(vs))
            if max(vs) > ceiling:
                bad.append(("GLIBC_2.%d" % max(vs), p))
open(floorf, "w").write("2.%d" % floor)
for why, p in bad:
    print("FAIL", why, "(> ceiling 2.%d)" % ceiling, os.path.relpath(p, root))
must = ["lib/libgtk-3.so.0", "lib/girepository-1.0/Gtk-3.0.typelib",
        "lib/python%s/site-packages/gi/__init__.py" % pyver,
        "lib/python%s/site-packages/floe/cli.py" % pyver,
        "lib/python%s/site-packages/floe2/cli.py" % pyver,
        "bin/floe-index",   # vfs/index runtime
        "bin/floe-renderd", # default multicore CPU renderer
        "share/glib-2.0/schemas",
        "fonts"]  # bundled sans fallback (fonts.conf lists it first)
klayout_dir = "lib/python%s/site-packages/klayout" % pyver
if with_klayout:
    must.append(klayout_dir)
elif os.path.exists(os.path.join(root, klayout_dir)):
    print("FAIL unexpected KLayout in default bundle", klayout_dir)
    bad.append(("unexpected KLayout", os.path.join(root, klayout_dir)))
missing = [m for m in must if not os.path.exists(os.path.join(root, m))]
for m in missing:
    print("MISSING", m)
# svg pixbuf loader is COSMETIC (Adwaita checkbox icons); render frames
# are png. Warn, don't fail - and dump the loader inventory so we can see
# what's actually there (png is the one that matters for the view).
import glob
loaders = glob.glob(os.path.join(root, "lib/gdk-pixbuf-2.0/*/loaders/*"))
names = sorted(os.path.basename(x) for x in loaders)
has_svg = any("svg" in n for n in names)
has_png = any("png" in n for n in names)
has_librsvg = bool(glob.glob(os.path.join(root, "lib/librsvg-2*.so*")))
print("pixbuf loaders (%d): %s" % (len(names), " ".join(names) or "(none)"))
print("  png-loader=%s  svg-loader=%s (librsvg lib=%s)"
      % (has_png, has_svg, has_librsvg))
if not has_svg:
    print("WARN: no svg loader - checkbox symbolic icons stay blank "
          "(cosmetic; the view renders png)")
if bad or missing:
    sys.exit(1)
print("verified: %d ELF files, all x86_64, needs glibc >= 2.%d "
      "(ceiling 2.%d)" % (elves, floor, ceiling))
PY
FLOOR="$(cat "$WORK/floor.txt" 2>/dev/null || echo 2.28)"
echo "== bundle runs on glibc >= $FLOOR"

# -- 7. assemble the bundle ---------------------------------------------
B="$WORK/${FLOE_PORTABLE_PRODUCT}-portable"
rm -rf "$B"; mkdir -p "$B"
mv "$WORK/runtime" "$B/runtime"

# The KLayout-enabled floe bundle can run both products from the same shared
# packages/runtime. The Rust-only floe2 bundle intentionally omits a `floe`
# launcher because stable floe requires KLayout.
PORTABLE_LAUNCHERS="floe2"
if [ "$FLOE_PORTABLE_KLAYOUT" = 1 ]; then
    PORTABLE_LAUNCHERS="floe floe2"
fi
for PRODUCT in $PORTABLE_LAUNCHERS; do
    cp "$REPO/tools/portable_launcher.sh" "$B/$PRODUCT"
    chmod +x "$B/$PRODUCT"
    sh -n "$B/$PRODUCT"
done

cat > "$B/selfcheck" <<EOF
#!/bin/sh
# Verifies the portable stack on THIS host without opening a window.
HERE=\$(CDPATH= cd -- "\$(dirname -- "\$0")" && pwd)
RT="\$HERE/runtime"
LOADERS="\$RT/lib/gdk-pixbuf-2.0/2.10.0/loaders"
FAILED=0
echo "host glibc:   \$(ldd --version 2>/dev/null | head -1)"
echo "arch:         \$(uname -m)   (needs x86_64, glibc >= ${FLOOR})"
# build the pixbuf loader cache exactly as the launcher does, so the png
# decoder used for render frames is exercised (a broken cache = black view)
CACHE=\$(mktemp)
LD_LIBRARY_PATH="\$RT/lib" GDK_PIXBUF_MODULEDIR="\$LOADERS" \\
    "\$RT/bin/gdk-pixbuf-query-loaders" > "\$CACHE" 2>/dev/null || true
if ! GI_TYPELIB_PATH="\$RT/lib/girepository-1.0" LD_LIBRARY_PATH="\$RT/lib" \\
GDK_PIXBUF_MODULE_FILE="\$CACHE" FONTCONFIG_FILE="\$HERE/fonts.conf" \\
PYTHONHOME="\$RT" PYTHONNOUSERSITE=1 \\
"\$RT/bin/python3" - <<'PY'
import sys
print("python:       %s OK" % sys.version.split()[0])
import gi; gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk, GdkPixbuf, GLib
print("pygobject:    OK  (GTK %d.%d)" % (Gtk.MAJOR_VERSION, Gtk.MINOR_VERSION))
fmts = {f.get_name() for f in GdkPixbuf.Pixbuf.get_formats()}
png = "png OK" if "png" in fmts else "png MISSING (render frames -> black!)"
svg = "svg OK" if "svg" in fmts else "svg MISSING (checkbox icons)"
print("pixbuf:       %s, %s" % (png, svg))
if "png" not in fmts:
    sys.exit(2)
import importlib.util
import numpy, PIL
print("numpy:        %s OK" % numpy.__version__)
print("pillow:       %s OK" % PIL.__version__)
if "$FLOE_PORTABLE_KLAYOUT" == "1":
    klayout = importlib.import_module("klayout")
    importlib.import_module("klayout.db")
    importlib.import_module("klayout.lay")
    print("klayout:      %s OK (rollback bundle)" % klayout.__version__)
elif importlib.util.find_spec("klayout") is not None:
    print("klayout:      UNEXPECTED in default bundle")
    sys.exit(2)
else:
    print("klayout:      not bundled (expected; Rust default)")
import floe, floe2
print("packages:     floe %s, floe2 %s OK" %
      (floe.__version__, floe2.__version__))
PY
then
    FAILED=1
fi
rm -f "\$CACHE"
if [ -x "\$RT/bin/floe-index" ]; then
    "\$RT/bin/floe-index" >/dev/null 2>&1
    INDEX_STATUS=\$?
    if [ \$INDEX_STATUS -eq 2 ]; then
        echo "floe-index:   OK (runtime/bin)"
    else
        echo "floe-index:   present but failed to run (host glibc?)"
        FAILED=1
    fi
else
    echo "floe-index:   MISSING - viewer opens but rendering and"
    echo "              indexing hang (re-deploy runtime/bin/floe-index)"
    FAILED=1
fi
if [ -x "\$RT/bin/floe-renderd" ]; then
    "\$RT/bin/floe-renderd" </dev/null >/dev/null 2>&1
    RENDERD_STATUS=\$?
    if [ \$RENDERD_STATUS -eq 0 ]; then
        echo "floe-renderd: OK (runtime/bin)"
    else
        echo "floe-renderd: present but failed to run (host glibc?)"
        FAILED=1
    fi
else
    echo "floe-renderd: MISSING - FLOE_RENDERER=rust unavailable"
    FAILED=1
fi
for PRODUCT in $PORTABLE_LAUNCHERS; do
    if "\$HERE/\$PRODUCT" --version >/dev/null 2>&1; then
        echo "launcher:     \$PRODUCT OK"
    else
        echo "launcher:     \$PRODUCT FAILED"
        FAILED=1
    fi
done
echo "display:      DISPLAY=\${DISPLAY:-<unset>}  (open test: ./floe2 view <file.oas>)"
exit "\$FAILED"
EOF

cat > "$B/fonts.conf" <<'EOF'
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<!-- floe portable: FONTCONFIG_FILE points here, which REPLACES the
     system /etc/fonts rules entirely - so this file must supply the
     generic-family aliases and rendering defaults itself. Bundled fonts
     first (Ubuntu = proportional UI face, DejaVu Sans Mono), then every
     common system location so Korean glyphs come from the host. -->
<fontconfig>
  <dir prefix="relative">runtime/fonts</dir>
  <dir>/usr/share/fonts</dir>
  <dir>/usr/share/X11/fonts</dir>
  <dir>/usr/local/share/fonts</dir>
  <dir prefix="xdg">fonts</dir>
  <dir>~/.fonts</dir>
  <cachedir>~/.cache/floe-rt/fontcache</cachedir>
  <alias>
    <family>sans-serif</family>
    <prefer>
      <family>Ubuntu</family><family>DejaVu Sans</family>
      <family>Liberation Sans</family><family>Noto Sans</family>
    </prefer>
  </alias>
  <alias>
    <family>serif</family>
    <prefer>
      <family>DejaVu Serif</family><family>Liberation Serif</family>
      <family>Noto Serif</family>
    </prefer>
  </alias>
  <alias>
    <family>monospace</family>
    <prefer>
      <family>Ubuntu Mono</family><family>DejaVu Sans Mono</family>
      <family>Liberation Mono</family>
    </prefer>
  </alias>
  <match target="font">
    <edit name="antialias" mode="assign"><bool>true</bool></edit>
    <edit name="hinting" mode="assign"><bool>true</bool></edit>
    <edit name="hintstyle" mode="assign"><const>hintslight</const></edit>
    <edit name="rgba" mode="assign"><const>none</const></edit>
  </match>
</fontconfig>
EOF

# GTK settings: a deterministic UI font + normalized 96dpi Xft so remote
# X servers (XQuartz etc.) don't produce oddly sized/shaped text.
mkdir -p "$B/etc/gtk-3.0"
cat > "$B/etc/gtk-3.0/settings.ini" <<'EOF'
[Settings]
gtk-font-name = Ubuntu 10
gtk-xft-antialias = 1
gtk-xft-hinting = 1
gtk-xft-hintstyle = hintslight
gtk-xft-dpi = 98304
EOF

cat > "$B/README-PORTABLE.txt" <<EOF
${FLOE_PORTABLE_PRODUCT} 포터블 번들 (${FLOE_PORTABLE_PRODUCT} ${VERSION}, ${STAMP} 빌드)
====================
PyGObject(python3-gobject)가 없는 호스트에서 ${FLOE_PORTABLE_PRODUCT}를 실행하기 위한 자체
포함 런타임. Python + PyGObject + GTK3 + NumPy/Pillow + floe/floe2 패키지 +
floe-index(러스트 인덱서/VFS daemon)와 floe-renderd(기본 CPU renderer)가
runtime/bin/ 안에 들어
있으며 시스템에는 아무것도 설치·변경하지 않는다. 시스템에서
쓰는 것은 X 디스플레이와 (있다면) 시스템 폰트뿐.

기본 floe2 번들은 KLayout을 포함하지 않는다. 안정판 floe/KLayout 번들은
FLOE_PORTABLE_KLAYOUT=1로 별도 빌드하며, 이 번들은 같은 runtime에서
floe(KLayout)와 floe2(Rust) 실행 파일을 모두 제공한다.

요구: x86_64 리눅스, glibc ${FLOOR}+, X 디스플레이.

설치/실행:
    tar xzf ${NAME}.tar.gz -C /opt        # 위치 자유
    /opt/${FLOE_PORTABLE_PRODUCT}-portable/selfcheck
    /opt/${FLOE_PORTABLE_PRODUCT}-portable/floe2 view /path/to/chip.oas
    /opt/${FLOE_PORTABLE_PRODUCT}-portable/floe2 index /path/to/chip.oas
    # KLayout 포함 floe-portable에서만:
    /opt/floe-portable/floe view /path/to/chip.oas

편의상 링크: ln -s /opt/${FLOE_PORTABLE_PRODUCT}-portable/floe2 /usr/local/bin/floe2

코드 업데이트: 새 floe/와 floe2/ 패키지를
    runtime/lib/python*/site-packages/floe
    runtime/lib/python*/site-packages/floe2
에 각각 덮어쓰고, 같은 체크아웃에서 빌드한 두 바이너리를
    runtime/bin/floe-index
    runtime/bin/floe-renderd
로 함께 교체한다 (chmod +x). 파이썬 패키지와 러스트 바이너리는
포맷 버전이 한 쌍이라 항상 같이 움직여야 한다. 런타임은 재사용.

문제 해결:
- "GLIBC_x.xx not found": 호스트 glibc가 ${FLOOR} 미만 → 사용 불가 (selfcheck 확인)
- 코드 3 종료: DISPLAY 미설정/접속 불가
- 창은 뜨는데 회색/렌더 오류: ~/.cache/floe2-rt 또는 ~/.cache/floe-rt 삭제 후 재실행
- 뷰어는 열리는데 렌더가 멈춤·인덱싱이 안 됨: runtime/bin/floe-index
  누락/실행 불가 (selfcheck의 floe-index 줄 확인)
- floe2 시작 오류: runtime/bin/floe-renderd 누락/실행 불가
EOF

chmod +x "$B/selfcheck"
sh -n "$B/selfcheck"
echo "== assembled ${FLOE_PORTABLE_PRODUCT} runtime selfcheck"
"$B/selfcheck"

# -- 8. pack ------------------------------------------------------------
cd "$WORK"
COPYFILE_DISABLE=1 tar -czf "$NAME.tar.gz" "${FLOE_PORTABLE_PRODUCT}-portable"
mkdir -p "$OUT_DIR"; mv "$NAME.tar.gz" "$OUT_DIR/"
cd "$OUT_DIR"
( command -v sha256sum >/dev/null && sha256sum "$NAME.tar.gz" ) \
    || shasum -a 256 "$NAME.tar.gz"
ls -lh "$NAME.tar.gz"
echo "done: $OUT_DIR/$NAME.tar.gz"
