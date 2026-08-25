#!/bin/sh
# Shared floe/floe2 portable launcher. The installed basename selects the
# product; make_portable.sh only installs floe where KLayout is bundled.
PRODUCT=${0##*/}
case "$PRODUCT" in
    floe | floe2) ;;
    *)
        echo "portable launcher must be named floe or floe2" >&2
        exit 2
        ;;
esac

# Resolve an optional convenience symlink before locating the adjacent
# runtime. Relative links remain relative to the directory containing them.
SELF=$0
while [ -L "$SELF" ]; do
    LINK=$(readlink "$SELF") || exit 2
    case "$LINK" in
        /*) SELF=$LINK ;;
        *) SELF=$(dirname "$SELF")/$LINK ;;
    esac
done
HERE=$(CDPATH= cd -- "$(dirname -- "$SELF")" && pwd)
RT="$HERE/runtime"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/${PRODUCT}-rt"
mkdir -p "$CACHE/schemas" 2>/dev/null || \
    CACHE="${TMPDIR:-/tmp}/${PRODUCT}-rt.$(id -u)"
mkdir -p "$CACHE/schemas" 2>/dev/null

# First run (or after the bundle moved/updated): build the machine-local GTK
# caches a normal package install would have produced.
if [ ! -f "$CACHE/schemas/gschemas.compiled" ] \
   || [ "$RT/share/glib-2.0/schemas" -nt \
        "$CACHE/schemas/gschemas.compiled" ]; then
    "$RT/bin/glib-compile-schemas" --targetdir="$CACHE/schemas" \
        "$RT/share/glib-2.0/schemas" 2>/dev/null || true
fi
if [ ! -f "$CACHE/pixbuf-loaders.cache" ] \
   || [ "$RT/lib/gdk-pixbuf-2.0/2.10.0/loaders" -nt \
        "$CACHE/pixbuf-loaders.cache" ]; then
    LD_LIBRARY_PATH="$RT/lib" \
    GDK_PIXBUF_MODULEDIR="$RT/lib/gdk-pixbuf-2.0/2.10.0/loaders" \
        "$RT/bin/gdk-pixbuf-query-loaders" \
        > "$CACHE/pixbuf-loaders.cache" 2>/dev/null || true
fi

GDK_PIXBUF_MODULE_FILE="$CACHE/pixbuf-loaders.cache"
GI_TYPELIB_PATH="$RT/lib/girepository-1.0"
GSETTINGS_SCHEMA_DIR="$CACHE/schemas"
XDG_DATA_DIRS="$RT/share:/usr/local/share:/usr/share"
XDG_CONFIG_DIRS="$HERE/etc:/etc/xdg"
FONTCONFIG_FILE="$HERE/fonts.conf"
LD_LIBRARY_PATH="$RT/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
GDK_BACKEND=x11
NO_AT_BRIDGE=1
PYTHONHOME="$RT"
PYTHONNOUSERSITE=1
export GDK_PIXBUF_MODULE_FILE GI_TYPELIB_PATH GSETTINGS_SCHEMA_DIR \
    XDG_DATA_DIRS XDG_CONFIG_DIRS FONTCONFIG_FILE LD_LIBRARY_PATH \
    GDK_BACKEND NO_AT_BRIDGE PYTHONHOME PYTHONNOUSERSITE

# XQuartz's XRender implementation can composite images to black. Opt into
# cairo's core-protocol fallback when viewing through XQuartz.
if [ -n "${FLOE_XQUARTZ:-}" ]; then
    CAIRO_DEBUG=xrender-version=-1
    export CAIRO_DEBUG
fi
exec "$RT/bin/python3" -m "$PRODUCT" "$@"
