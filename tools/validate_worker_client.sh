#!/bin/sh
# M1a real-client gate. Python is the OLD adapter oracle, not the new runtime.
set -eu
cd "$(dirname "$0")/.."
if [ "$#" -ne 2 ]; then
    echo 'Usage: sh tools/validate_worker_client.sh SOURCE.oas CACHE.floe' >&2
    exit 2
fi
worker_source=$1
worker_cache=$2
case "$worker_source" in /*) ;; *) worker_source=$PWD/$worker_source ;; esac
case "$worker_cache" in /*) ;; *) worker_cache=$PWD/$worker_cache ;; esac
worker_python=${FLOE_TEST_PYTHON:-.venv/bin/python}
worker_renderd=$PWD/rust/target/release/floe-renderd
worker_oracle=$(mktemp -d "${TMPDIR:-/tmp}/floe-worker-oracle.XXXXXX")
# All writes are confined to this newly allocated fixture directory.
trap 'rm -rf "$worker_oracle"' EXIT HUP INT TERM
FLOE_WORKER_TEST_SOURCE="$worker_source" FLOE_WORKER_TEST_CACHE="$worker_cache" \
FLOE_WORKER_TEST_ORACLE="$worker_oracle" FLOE_RENDERD_BIN="$worker_renderd" \
PYTHONDONTWRITEBYTECODE=1 "$worker_python" - <<'PY'
import os
from pathlib import Path
from floe.cache import Cache
from floe.rust_render import RustRenderWorker

cache = Cache(os.environ['FLOE_WORKER_TEST_SOURCE'])
cache.dir = os.environ['FLOE_WORKER_TEST_CACHE']
cache.load()
out = Path(os.environ['FLOE_WORKER_TEST_ORACLE'])
view = list(map(float, cache.meta['bbox']))
(out / 'view.txt').write_text(' '.join(repr(v) for v in view))
keys = [(int(l['layer']), int(l['datatype'])) for l in cache.meta['layers']]
worker = RustRenderWorker(cache, stream_kb=0)
worker.start()
try:
    worker.submit({'kind': 'recolor', 'colors': [(k, '#40c080') for k in keys]})
    worker.submit({'kind': 'repattern', 'fills': [(k, '\n'.join(['*' * 16] * 16)) for k in keys],
                   'widths': [(k, 1) for k in keys]})
    for gen, name in [(1, 'frame.png'), (2, 'labels.png')]:
        worker.submit({'kind': 'render', 'gen': gen, 'bbox': view, 'w': 384, 'h': 384,
                       'depth': None if gen == 1 else 1, 'cut_px': 0. if gen == 1 else 1.,
                       'frames': gen == 2, 'labels': gen == 2, 'label_font_px': 19,
                       'thin': 'cull' if gen == 1 else 'keep', 'frame_cache': False,
                       'frame_format': 'png', 'visible': None})
        while True:
            event = worker.res.get(timeout=60)
            if event.get('kind') == 'error':
                raise RuntimeError(event)
            if event.get('kind') == 'frame':
                if event.get('partial') or event.get('refining'):
                    raise RuntimeError('oracle not complete: %r' % event)
                (out / name).write_bytes(event['png'])
                break
finally:
    worker.stop()
PY
(
    cd rust
    FLOE_WORKER_TEST_CACHE="$worker_cache" FLOE_WORKER_TEST_RENDERD="$worker_renderd" \
    FLOE_WORKER_TEST_ORACLE="$worker_oracle" \
    cargo test --offline -p floe-worker-client --test real_worker -- --ignored --nocapture
)
