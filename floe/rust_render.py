"""`floe.service.RenderWorker` compatible adapter for `floe-renderd`.

The adapter translates the existing queue-shaped Python job/result contract
to the renderer's strict line protocol. KLayout remains an independently
selectable rollback backend while the Rust renderer is stabilized.
"""

import os
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path


_DEFAULT_JOBS = max(1, min(8, os.cpu_count() or 1))
_DEFAULT_BUDGET_MB = 1024
_DEFAULT_ROUND_PAGES = 128
_DEFAULT_TILE_PX = 128
_PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


class _PopenCompat:
    """Expose the one multiprocessing.Process method used by floe probe."""

    def __init__(self, process):
        self._process = process

    def __getattr__(self, name):
        return getattr(self._process, name)

    def is_alive(self):
        return self._process.poll() is None


def _env_int(name, default, minimum, maximum):
    value = os.environ.get(name)
    if value is None:
        return default
    try:
        parsed = int(value)
    except ValueError as exc:
        raise RuntimeError("%s must be an integer, got %r" %
                           (name, value)) from exc
    if parsed < minimum or parsed > maximum:
        raise RuntimeError("%s must be in %d..%d, got %d" %
                           (name, minimum, maximum, parsed))
    return parsed


def _bool_wire(value):
    return "1" if value else "0"


def _parse_wire_line(line):
    fields = {}
    tokens = line.strip().split()
    if not tokens:
        return "", fields
    for token in tokens[1:]:
        if "=" in token:
            key, value = token.split("=", 1)
            fields[key] = value
    return tokens[0], fields


def _wire_int(fields, name, default=0):
    try:
        return int(fields.get(name, default))
    except (TypeError, ValueError):
        return default


def _pattern_fill(rows):
    """Convert floe's 16x16 `*`/`.` bitmap to renderd style syntax."""
    if not isinstance(rows, str):
        raise ValueError("fill bitmap must be a string")
    lines = rows.splitlines()
    if len(lines) != 16 or any(len(line) != 16 for line in lines):
        raise ValueError("fill bitmap must contain 16 rows of 16 pixels")
    if any(ch not in ".*" for line in lines for ch in line):
        raise ValueError("fill bitmap pixels must be '.' or '*'")
    words = []
    for line in lines:
        word = 0
        for ch in line:
            word = (word << 1) | (ch == "*")
        words.append(word)
    if all(word == 0xFFFF for word in words):
        return "solid"
    if all(word == 0 for word in words):
        return "clear"
    if words == [0xAAAA if row % 2 == 0 else 0x5555
                 for row in range(16)]:
        return "speckle"
    return "pat:" + "".join("%04X" % word for word in words)


def _layer_key(value):
    if not isinstance(value, (list, tuple)) or len(value) != 2:
        raise ValueError("layer key must be [layer, datatype]")
    return int(value[0]), int(value[1])


def _find_binary():
    configured = os.environ.get("FLOE_RENDERD_BIN")
    candidates = []
    if configured:
        candidates.append(configured)
    root = Path(__file__).resolve().parents[1]
    candidates.extend((
        str(root / "rust" / "target" / "release" / "floe-renderd"),
        str(root / "rust" / "dist" / "floe-renderd-linux-gnu"),
        str(root / "rust" / "dist" / "floe-renderd-linux-x86_64"),
    ))
    adjacent = Path(sys.executable).resolve().parent / "floe-renderd"
    candidates.append(str(adjacent))
    on_path = shutil.which("floe-renderd")
    if on_path:
        candidates.append(on_path)
    for candidate in candidates:
        if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return os.path.abspath(candidate)
    raise RuntimeError(
        "floe-renderd not found; set FLOE_RENDERD_BIN (checked: %s)" %
        ", ".join(candidates))


class RustRenderWorker:
    """Queue-compatible worker backed by one persistent Rust daemon."""

    def __init__(self, cache, stream_kb=None, stream_target_ms=500,
                 debug=False):
        del stream_target_ms  # Rust rounds are page-budgeted, not time-tuned.
        self.cache = cache
        self.stream_kb = stream_kb
        self.debug = bool(debug)
        self.res = queue.Queue()
        self._proc = None
        self._reader = None
        self._stderr_reader = None
        self._write_lock = threading.Lock()
        self._condition = threading.Condition()
        self._ready = False
        self._opened = False
        self._styled_epoch = None
        self._startup_error = None
        self._stopping = False
        self._jobs = {}
        self._jobs_lock = threading.Lock()
        self._mono = False
        self._style_epoch = 0
        self._colors = {}
        self._fills = {}
        self._widths = {}
        self._stderr_tail = []
        self._work_dir = None
        self._cache_path = None
        self._binary = _find_binary()
        self._jobs_count = _env_int(
            "FLOE_RUST_JOBS", _DEFAULT_JOBS, 1, 256)
        self._budget_mb = _env_int(
            "FLOE_RUST_BUDGET_MB", _DEFAULT_BUDGET_MB, 1, 1 << 20)
        self._round_pages = _env_int(
            "FLOE_RUST_ROUND_PAGES", _DEFAULT_ROUND_PAGES, 1, 1 << 30)
        self._tile_px = _env_int(
            "FLOE_RUST_TILE_PX", _DEFAULT_TILE_PX, 1, 4096)
        self._init_styles()

    def _init_styles(self):
        layers = self.cache.meta.get("layers", [])
        for layer in layers:
            key = int(layer["layer"]), int(layer["datatype"])
            self._colors[key] = str(layer.get("color") or "#ffffff")
        try:
            from floe import cache as cache_mod
            from floe import fillpat
            rows, _path = cache_mod.load_layer_props(self.cache.src)
            patterns = fillpat.default_patterns()
            for key, _color, fill, _name, _f1, width in rows:
                key = _layer_key(key)
                index = fillpat.fill_index(fill)
                if index is not None:
                    self._fills[key] = _pattern_fill(patterns[index])
                try:
                    parsed_width = int(width)
                    if parsed_width > 1:
                        self._widths[key] = min(8, parsed_width)
                except ValueError:
                    pass
        except (ImportError, OSError, ValueError):
            # Personalization must not prevent the renderer from starting.
            pass

    def start(self):
        if self._proc is not None:
            raise RuntimeError("RustRenderWorker is already started")
        self._work_dir = tempfile.mkdtemp(prefix="floe-rust-worker-")
        cache_path = os.path.abspath(self.cache.dir)
        if any(ch.isspace() for ch in cache_path):
            alias = os.path.join(self._work_dir, "cache")
            os.symlink(cache_path, alias)
            cache_path = alias
        self._cache_path = cache_path
        self._proc = _PopenCompat(subprocess.Popen(
            [self._binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, bufsize=1))
        self._reader = threading.Thread(
            target=self._read_stdout, name="floe-renderd-stdout", daemon=True)
        self._stderr_reader = threading.Thread(
            target=self._read_stderr, name="floe-renderd-stderr", daemon=True)
        try:
            self._reader.start()
            self._stderr_reader.start()
            self._wait_for(lambda: self._ready, "ready")
            self._send("open cache=%s budget_mb=%d jobs=%d" %
                       (self._cache_path, self._budget_mb,
                        self._jobs_count))
            self._wait_for(lambda: self._opened, "open")
            self._publish_style(wait=True)
        except Exception:
            self.stop()
            raise

    def alive(self):
        return self._proc is not None and self._proc.poll() is None

    def exitcode(self):
        return None if self._proc is None else self._proc.poll()

    def submit(self, job):
        if not self.alive():
            self.res.put({"kind": "error", "msg":
                          "Rust render service is not running"})
            return
        kind = job.get("kind")
        try:
            if kind == "render":
                self._submit_render(job)
            elif kind == "recolor":
                self._submit_recolor(job)
            elif kind == "repattern":
                self._submit_repattern(job)
            elif kind == "mono":
                self._mono = bool(job.get("on"))
            elif kind in ("snap", "pick", "clip"):
                self.res.put({
                    "kind": "error",
                    "msg": "Rust backend does not implement %s yet" % kind})
            else:
                self.res.put({"kind": "error", "msg":
                              "unknown Rust worker job: %r" % kind})
        except Exception as exc:
            self.res.put({"kind": "error", "msg": str(exc)})

    def stop(self):
        if self._proc is None:
            return
        self._stopping = True
        if self.alive():
            try:
                self._send("quit")
            except (BrokenPipeError, OSError):
                pass
        try:
            self._proc.wait(timeout=1.5)
        except subprocess.TimeoutExpired:
            self._proc.terminate()
            try:
                self._proc.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                self._proc.kill()
                self._proc.wait(timeout=1.0)
        for stream in (self._proc.stdin, self._proc.stdout,
                       self._proc.stderr):
            try:
                stream.close()
            except (AttributeError, OSError):
                pass
        for thread in (self._reader, self._stderr_reader):
            if thread is not None:
                thread.join(timeout=0.5)
        if self._work_dir:
            shutil.rmtree(self._work_dir, ignore_errors=True)

    def _wait_for(self, predicate, phase):
        deadline = time.monotonic() + 10.0
        with self._condition:
            while not predicate():
                if self._startup_error:
                    raise RuntimeError(self._startup_error)
                if self._proc.poll() is not None:
                    raise RuntimeError(self._exit_message(phase))
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeError("floe-renderd %s timeout" % phase)
                self._condition.wait(min(remaining, 0.1))

    def _send(self, command):
        if any(ch in command for ch in "\r\n"):
            raise ValueError("render command contains a newline")
        with self._write_lock:
            if self._proc.stdin is None:
                raise RuntimeError("floe-renderd stdin is closed")
            self._proc.stdin.write(command + "\n")
            self._proc.stdin.flush()
        if self.debug:
            print("[rust-render] > " + command, file=sys.stderr, flush=True)

    def _submit_render(self, job):
        if job.get("abstract"):
            raise RuntimeError(
                "abstract mode is intentionally unsupported by the Rust "
                "backend (KLayout-specific)")
        if job.get("coverage"):
            raise RuntimeError("Rust backend does not implement coverage mode yet")
        generation = int(job["gen"])
        bbox = tuple(float(value) for value in job["bbox"])
        if len(bbox) != 4:
            raise ValueError("render bbox must have four coordinates")
        visible = job.get("visible")
        if visible is None:
            layers = "all"
        elif not visible:
            layers = "none"
        else:
            layers = ",".join("%d/%d" % layer for layer in sorted(
                _layer_key(layer) for layer in visible))
        depth_value = job.get("depth")
        depth = "full" if depth_value is None or int(depth_value) >= 999 \
            else str(max(0, int(depth_value)))
        output = os.path.join(self._work_dir, "frame-%d.png" % generation)
        state = {
            "job": dict(job), "output": output,
            "started": time.monotonic(), "new": 0,
            "read_us": 0, "decode_us": 0, "scene_us": 0,
            "draw_us": 0, "plan_us": 0,
        }
        with self._jobs_lock:
            self._jobs[generation] = state
        command = (
            "render gen=%d view=%s w=%d h=%d depth=%s cut=%s exact=0 "
            "layers=%s frames=%s mono=%s jobs=%d tile_px=%d "
            "round_pages=%d round_paths=1 style_epoch=%d out=%s" % (
                generation, ",".join(repr(value) for value in bbox),
                int(job["w"]), int(job["h"]), depth,
                repr(max(0.0, float(job.get("cut_px") or 0.0))), layers,
                _bool_wire(job.get("frames", True)),
                _bool_wire(self._mono), self._jobs_count, self._tile_px,
                self._round_pages, self._style_epoch, output))
        self._send(command)

    def _submit_recolor(self, job):
        for key, color in job.get("colors", []):
            self._colors[_layer_key(key)] = str(color)
        self._publish_style(wait=False)

    def _submit_repattern(self, job):
        fills = {}
        for key, rows in job.get("fills", []):
            fills[_layer_key(key)] = _pattern_fill(rows)
        widths = {}
        for key, width in job.get("widths", []):
            width = int(width)
            if width < 1 or width > 8:
                raise ValueError("line width must be in 1..8")
            widths[_layer_key(key)] = width
        self._fills = fills
        self._widths = widths
        self._publish_style(wait=False)

    def _publish_style(self, wait):
        self._style_epoch += 1
        epoch = self._style_epoch
        path = os.path.join(self._work_dir, "style-%d.tsv" % epoch)
        rows = []
        for key in sorted(self._colors):
            color = self._colors[key]
            fill = self._fills.get(key, "speckle")
            width = self._widths.get(key, 1)
            rows.append("%d/%d %s %s %d\n" %
                        (key[0], key[1], color, fill, width))
        if not rows:
            raise RuntimeError("cache contains no renderable layers")
        with open(path, "x", encoding="ascii") as style_file:
            style_file.writelines(rows)
            style_file.flush()
            os.fsync(style_file.fileno())
        self._send("style epoch=%d path=%s" % (epoch, path))
        if wait:
            self._wait_for(lambda: self._styled_epoch == epoch, "style")

    def _read_stdout(self):
        try:
            for raw_line in self._proc.stdout:
                line = raw_line.rstrip("\r\n")
                if self.debug:
                    print("[rust-render] < " + line,
                          file=sys.stderr, flush=True)
                kind, fields = _parse_wire_line(line)
                self._handle_line(kind, fields, line)
        except (OSError, ValueError) as exc:
            self._set_startup_error("floe-renderd stdout: %s" % exc)
        finally:
            with self._condition:
                self._condition.notify_all()
            if not self._stopping and not self._startup_error:
                self.res.put({"kind": "error", "msg":
                              self._exit_message("stdout")})

    def _read_stderr(self):
        try:
            for raw_line in self._proc.stderr:
                line = raw_line.rstrip("\r\n")
                self._stderr_tail.append(line)
                del self._stderr_tail[:-20]
                if self.debug:
                    print("[rust-render][stderr] " + line,
                          file=sys.stderr, flush=True)
        except OSError:
            pass

    def _handle_line(self, kind, fields, line):
        if kind == "ready":
            with self._condition:
                self._ready = True
                self._condition.notify_all()
        elif kind == "opened":
            with self._condition:
                self._opened = True
                self._condition.notify_all()
        elif kind == "styled":
            with self._condition:
                self._styled_epoch = _wire_int(fields, "epoch", -1)
                self._condition.notify_all()
        elif kind == "frame":
            self._emit_frame(fields)
        elif kind in ("cancelled", "dropped"):
            generation = _wire_int(fields, "gen", -1)
            with self._jobs_lock:
                self._jobs.pop(generation, None)
        elif kind == "error":
            message = fields.get("message", line).replace("_", " ")
            generation = _wire_int(fields, "gen", -1)
            with self._jobs_lock:
                known = self._jobs.pop(generation, None)
            if not self._opened or (self._style_epoch and
                                    self._styled_epoch is None):
                self._set_startup_error(message)
            else:
                self.res.put({"kind": "error", "msg": message,
                              "gen": generation if known else -1})

    def _emit_frame(self, fields):
        generation = _wire_int(fields, "gen", -1)
        with self._jobs_lock:
            state = self._jobs.get(generation)
        if state is None:
            return
        path = fields.get("png")
        try:
            with open(path, "rb") as frame_file:
                png = frame_file.read()
        except OSError as exc:
            self.res.put({"kind": "error", "gen": generation,
                          "msg": "read Rust frame: %s" % exc})
            return
        if not png.startswith(_PNG_SIGNATURE):
            self.res.put({"kind": "error", "gen": generation,
                          "msg": "Rust frame is not a PNG"})
            return
        if path != state["output"]:
            try:
                os.unlink(path)
            except OSError:
                pass

        state["plan_us"] = _wire_int(fields, "plan_us")
        state["read_us"] += _wire_int(fields, "read_us")
        state["decode_us"] += _wire_int(fields, "decode_us")
        state["scene_us"] += _wire_int(fields, "scene_us")
        state["draw_us"] += _wire_int(fields, "raster_us")
        state["new"] += _wire_int(fields, "cache_miss")
        job = state["job"]
        deferred = _wire_int(fields, "deferred")
        partial = _wire_int(fields, "partial") != 0
        output = {
            "kind": "frame", "png": png, "bbox": job["bbox"],
            "gen": generation, "tiles": _wire_int(
                fields, "plan_pages",
                _wire_int(fields, "pages") + deferred),
            "new": state["new"], "scope": job.get("scope", "live"),
            "bg": False,
            "load_ms": round((state["plan_us"] + state["read_us"] +
                              state["decode_us"] + state["scene_us"]) /
                             1000),
            "phase_plan": round(state["plan_us"] / 1000),
            "phase_delta": round(state["read_us"] / 1000),
            "phase_apply": round((state["decode_us"] +
                                  state["scene_us"]) / 1000),
            "draw_ms": round(state["draw_us"] / 1000),
            "wait_ms": 0,
            "ms": round((time.monotonic() - state["started"]) * 1000),
            "plan_ms": state["plan_us"] / 1000.0,
            "wc_cells": _wire_int(fields, "wc_cells"),
            "inst_edges": _wire_int(fields, "inst_edges"),
            "frame_rects": _wire_int(fields, "frame_rects"),
        }
        if partial:
            output["refining"] = deferred
        if job.get("labels", True):
            output["labels_truncated"] = True
        cut_px = max(0.0, float(job.get("cut_px") or 0.0))
        if cut_px:
            span_dbu = float(job["bbox"][2]) - float(job["bbox"][0])
            px_per_um = float(job["w"]) / max(
                1e-12, span_dbu * float(self.cache.meta["dbu"]))
            output["cut_um"] = round(cut_px / px_per_um, 3)
        self.res.put(output)
        if _wire_int(fields, "final") != 0:
            with self._jobs_lock:
                self._jobs.pop(generation, None)

    def _set_startup_error(self, message):
        with self._condition:
            if self._startup_error is None:
                self._startup_error = message
            self._condition.notify_all()

    def _exit_message(self, phase):
        code = None if self._proc is None else self._proc.poll()
        tail = " | ".join(self._stderr_tail[-3:])
        message = "floe-renderd exited during %s (code %s)" % (phase, code)
        return message + ((": " + tail) if tail else "")
