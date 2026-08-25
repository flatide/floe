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

from . import __version__


_DEFAULT_JOBS = max(1, min(8, os.cpu_count() or 1))
_DEFAULT_BUDGET_MB = 1024
_DEFAULT_ROUND_PAGES = 1024
_NO_REFINEMENT_ROUND_PAGES = 1 << 30
_DEFAULT_TILE_PX = 384
_DEFAULT_LABEL_FONT_PX = 14
_DEFAULT_OPEN_TIMEOUT_S = 300
_DEFAULT_CLIP_TIMEOUT_S = 300
_PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
_OASIS_SIGNATURE = b"%SEMI-OASIS\r\n"


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


def _wire_hex(fields, name):
    value = fields.get(name)
    if value is None:
        raise ValueError("missing wire field: %s" % name)
    try:
        return bytes.fromhex(value).decode("utf-8")
    except (ValueError, UnicodeDecodeError) as exc:
        raise ValueError("invalid UTF-8 hex field: %s" % name) from exc


def _wire_pair_list(value, field):
    if not value:
        return []
    points = []
    try:
        for pair in value.split(";"):
            x, y = pair.split(",")
            points.append((int(x), int(y)))
    except (TypeError, ValueError) as exc:
        raise ValueError("invalid %s field" % field) from exc
    return points


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

    supports_label_font_px = True
    supports_abstract = False

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
        self._lifecycle_lock = threading.Lock()
        self._write_lock = threading.Lock()
        self._condition = threading.Condition()
        self._ready = False
        self._renderd_version = None
        self._opened = False
        self._styled_epoch = None
        self._style_paths = {}
        self._startup_error = None
        self._stopping = False
        self._expected_exit = False
        self._jobs = {}
        self._jobs_lock = threading.Lock()
        self._clip_seq = 0
        self._clip_jobs = {}
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
        self._raster_jobs_count = _env_int(
            "FLOE_RUST_RASTER_JOBS", min(4, self._jobs_count), 1, 256)
        self._budget_mb = _env_int(
            "FLOE_RUST_BUDGET_MB", _DEFAULT_BUDGET_MB, 1, 1 << 20)
        # The shared viewer spells no intermediate frames as stream_kb=0:
        # stable floe passes that to vfsd, while Rust makes every practical
        # miss set one batch.  This intentionally overrides the tuning env so
        # identical floe/floe2 `--refinement off` command lines mean the same.
        self._round_pages = (
            _NO_REFINEMENT_ROUND_PAGES if stream_kb == 0 else
            _env_int("FLOE_RUST_ROUND_PAGES", _DEFAULT_ROUND_PAGES,
                     1, _NO_REFINEMENT_ROUND_PAGES))
        self._tile_px = _env_int(
            "FLOE_RUST_TILE_PX", _DEFAULT_TILE_PX, 1, 4096)
        self._label_font_px = _env_int(
            "FLOE_RUST_LABEL_PX", _DEFAULT_LABEL_FONT_PX, 6, 96)
        self._open_timeout_s = _env_int(
            "FLOE_RUST_OPEN_TIMEOUT_S", _DEFAULT_OPEN_TIMEOUT_S,
            1, 24 * 60 * 60)
        self._clip_timeout_s = _env_int(
            "FLOE_RUST_CLIP_TIMEOUT_S", _DEFAULT_CLIP_TIMEOUT_S,
            1, 24 * 60 * 60)
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
        try:
            with self._lifecycle_lock:
                if self._stopping:
                    raise RuntimeError(
                        "RustRenderWorker was stopped before startup")
                if self._proc is not None:
                    raise RuntimeError("RustRenderWorker is already started")
                self._work_dir = tempfile.mkdtemp(
                    prefix="floe-rust-worker-")
                cache_path = os.path.abspath(self.cache.dir)
                if any(ch.isspace() for ch in cache_path):
                    alias = os.path.join(self._work_dir, "cache")
                    os.symlink(cache_path, alias)
                    cache_path = alias
                self._cache_path = cache_path
                self._proc = _PopenCompat(subprocess.Popen(
                    [self._binary], stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                    text=True, bufsize=1,
                    start_new_session=(os.name == "posix")))
                self._reader = threading.Thread(
                    target=self._read_stdout, name="floe-renderd-stdout",
                    daemon=True)
                self._stderr_reader = threading.Thread(
                    target=self._read_stderr, name="floe-renderd-stderr",
                    daemon=True)
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

    def start_async(self, callback):
        """Run the potentially cold cache open without blocking GTK."""
        def run():
            error = None
            try:
                self.start()
            except Exception as exc:
                error = exc
            try:
                callback(error)
            except Exception:
                if self.debug:
                    import traceback
                    traceback.print_exc()

        thread = threading.Thread(
            target=run, name="floe-renderd-start", daemon=True)
        thread.start()
        return thread

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
            elif kind == "snap":
                self._submit_snap(job)
            elif kind == "pick":
                self._submit_pick(job)
            elif kind == "clip":
                self._submit_clip(job)
            else:
                self.res.put({"kind": "error", "msg":
                              "unknown Rust worker job: %r" % kind})
        except Exception as exc:
            self.res.put({"kind": "error", "msg": str(exc)})

    def stop(self):
        with self._lifecycle_lock:
            if self._stopping:
                return
            self._stopping = True
            proc = self._proc
        if proc is None:
            if self._work_dir:
                shutil.rmtree(self._work_dir, ignore_errors=True)
            return
        with self._jobs_lock:
            clip_states = list(self._clip_jobs.values())
            self._clip_jobs.clear()
        for state in clip_states:
            timer = state.get("timer")
            if timer is not None:
                timer.cancel()
        if self.alive():
            try:
                self._send("quit")
            except (BrokenPipeError, OSError):
                pass
        try:
            proc.wait(timeout=1.5)
        except subprocess.TimeoutExpired:
            proc.terminate()
            try:
                proc.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=1.0)
        for stream in (proc.stdin, proc.stdout, proc.stderr):
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
        timeout = self._open_timeout_s if phase == "open" else 10.0
        deadline = time.monotonic() + timeout
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
        generation = int(job["gen"])
        try:
            label_font_px = int(job.get(
                "label_font_px", self._label_font_px))
        except (TypeError, ValueError) as exc:
            raise ValueError("label_font_px must be an integer") from exc
        if label_font_px < 6 or label_font_px > 96:
            raise ValueError("label_font_px must be in 6..96")
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
            "draw_us": 0, "png_us": 0, "plan_us": 0,
            "publish_write_us": 0, "publish_sync_us": 0,
            "publish_rename_us": 0, "adapter_read_us": 0,
            "cache_hit": 0, "render_tiles": 0,
            "frame_cache_hit": 0,
            "resident_bytes": 0, "decode_workers": 0,
        }
        with self._jobs_lock:
            self._jobs[generation] = state
        command = (
            "render gen=%d view=%s w=%d h=%d depth=%s cut=%s exact=0 "
            "layers=%s frames=%s labels=%s font_px=%d mono=%s "
            "frame_cache=%s "
            "jobs=%d decode_jobs=%d tile_px=%d "
            "round_pages=%d round_paths=1 style_epoch=%d out=%s" % (
                generation, ",".join(repr(value) for value in bbox),
                int(job["w"]), int(job["h"]), depth,
                repr(max(0.0, float(job.get("cut_px") or 0.0))), layers,
                _bool_wire(job.get("frames", True)),
                _bool_wire(job.get("labels", True)), label_font_px,
                _bool_wire(self._mono),
                _bool_wire(job.get("frame_cache", True)),
                self._raster_jobs_count,
                self._jobs_count, self._tile_px,
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

    @staticmethod
    def _query_layers(job):
        layers = job.get("layers")
        # Match the parent service: an absent or empty selection means all
        # layers for pick/snap (render jobs intentionally use "none").
        if not layers:
            return "all"
        return ",".join("%d/%d" % layer for layer in sorted(
            set(_layer_key(layer) for layer in layers)))

    def _submit_snap(self, job):
        self._send("snap seq=%d x=%d y=%d r=%d layers=%s" % (
            int(job.get("seq", -1)), int(job["x"]), int(job["y"]),
            max(1, int(job["r"])), self._query_layers(job)))

    def _submit_pick(self, job):
        self._send("pick seq=%d x=%d y=%d r=%d nth=%d layers=%s" % (
            int(job.get("seq", -1)), int(job["x"]), int(job["y"]),
            max(1, int(job["r"])), int(job.get("nth", 0)),
            self._query_layers(job)))

    def _submit_clip(self, job):
        bbox = tuple(int(value) for value in job["bbox"])
        if len(bbox) != 4:
            raise ValueError("clip bbox must have four coordinates")
        if bbox[0] > bbox[2] or bbox[1] > bbox[3]:
            raise ValueError("clip bbox coordinates are reversed")
        destination = os.path.abspath(os.fspath(job["out"]))
        with self._jobs_lock:
            self._clip_seq += 1
            sequence = self._clip_seq
            daemon_output = os.path.join(
                self._work_dir, "clip-%d.oas" % sequence)
            self._clip_jobs[sequence] = {
                "out": destination,
                "daemon": daemon_output,
                "started": time.monotonic(),
            }
            timer = threading.Timer(
                self._clip_timeout_s, self._clip_timed_out,
                args=(sequence,))
            timer.daemon = True
            self._clip_jobs[sequence]["timer"] = timer
        try:
            cell_name = str(job.get("cell_name", "FLOE_CLIP"))
            if not cell_name:
                raise ValueError("clip cell name must not be empty")
            self._send(
                "clip seq=%d box=%s layers=%s jobs=%d cell_hex=%s out=%s" % (
                    sequence, ",".join(str(value) for value in bbox),
                    self._query_layers(job), self._jobs_count,
                    cell_name.encode("utf-8").hex(),
                    daemon_output))
            timer.start()
        except Exception:
            with self._jobs_lock:
                state = self._clip_jobs.pop(sequence, None)
            if state is not None:
                state["timer"].cancel()
            raise

    def _clip_timed_out(self, sequence):
        with self._jobs_lock:
            state = self._clip_jobs.pop(sequence, None)
        if state is None:
            return
        state["timer"].cancel()
        try:
            os.unlink(state["daemon"])
        except OSError:
            pass
        self.res.put({
            "kind": "error",
            "msg": "Rust clip timed out after %d seconds" %
                   self._clip_timeout_s,
        })
        # clip runs inline in renderd's worker. Once it has exceeded the hard
        # deadline the daemon cannot serve renders or even process `quit`.
        if self.alive():
            try:
                self._expected_exit = True
                self._proc.terminate()
            except OSError:
                pass

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
        with self._condition:
            self._style_paths[epoch] = path
        try:
            self._send("style epoch=%d path=%s" % (epoch, path))
        except Exception:
            with self._condition:
                self._style_paths.pop(epoch, None)
            try:
                os.unlink(path)
            except OSError:
                pass
            raise
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
            if (not self._stopping and not self._expected_exit and
                    not self._startup_error):
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
            renderd_version = fields.get("version")
            if renderd_version != __version__:
                self._set_startup_error(
                    "floe-renderd version mismatch: expected %s, got %s "
                    "(%s); rebuild or replace the Rust binaries" % (
                        __version__, renderd_version or "missing",
                        self._binary))
                return
            with self._condition:
                self._renderd_version = renderd_version
                self._ready = True
                self._condition.notify_all()
        elif kind == "opened":
            with self._condition:
                self._opened = True
                self._condition.notify_all()
        elif kind == "styled":
            with self._condition:
                self._styled_epoch = _wire_int(fields, "epoch", -1)
                style_path = self._style_paths.pop(
                    self._styled_epoch, None)
                self._condition.notify_all()
            if style_path is not None:
                try:
                    os.unlink(style_path)
                except OSError:
                    pass
        elif kind == "frame":
            self._emit_frame(fields)
        elif kind == "snap":
            self._emit_snap(fields)
        elif kind == "pick":
            self._emit_pick(fields)
        elif kind == "clip":
            self._emit_clip(fields)
        elif kind in ("cancelled", "dropped"):
            generation = _wire_int(fields, "gen", -1)
            with self._jobs_lock:
                self._jobs.pop(generation, None)
        elif kind == "error":
            message = fields.get("message", line).replace("_", " ")
            if fields.get("code") == "clip":
                sequence = _wire_int(fields, "seq", -1)
                with self._jobs_lock:
                    state = self._clip_jobs.pop(sequence, None)
                if state is not None:
                    state["timer"].cancel()
                    try:
                        os.unlink(state["daemon"])
                    except OSError:
                        pass
                self.res.put({"kind": "error", "msg": message})
                return
            generation = _wire_int(fields, "gen", -1)
            with self._jobs_lock:
                known = self._jobs.pop(generation, None)
            if not self._opened or (self._style_epoch and
                                    self._styled_epoch is None):
                self._set_startup_error(message)
            else:
                self.res.put({"kind": "error", "msg": message,
                              "gen": generation if known else -1})

    def _emit_snap(self, fields):
        output = {
            "kind": "snap",
            "seq": _wire_int(fields, "seq", -1),
            "found": _wire_int(fields, "found") != 0,
            "x": _wire_int(fields, "x"),
            "y": _wire_int(fields, "y"),
            "snap": "" if fields.get("snap") == "-" else
                    fields.get("snap", ""),
        }
        try:
            if "err_hex" in fields:
                output["err"] = _wire_hex(fields, "err_hex")
        except ValueError as exc:
            self.res.put({"kind": "error", "msg": str(exc)})
            return
        self.res.put(output)

    def _emit_pick(self, fields):
        output = {
            "kind": "pick",
            "seq": _wire_int(fields, "seq", -1),
            "found": _wire_int(fields, "found") != 0,
            "count": _wire_int(fields, "count"),
        }
        try:
            if "err_hex" in fields:
                output["err"] = _wire_hex(fields, "err_hex")
            if output["found"]:
                bbox = [int(value) for value in
                        fields["bbox"].split(",")]
                if len(bbox) != 4:
                    raise ValueError("invalid bbox field")
                output.update({
                    "index": _wire_int(fields, "index"),
                    "layer": _wire_int(fields, "layer"),
                    "datatype": _wire_int(fields, "datatype"),
                    "lname": _wire_hex(fields, "lname_hex"),
                    "cell": _wire_hex(fields, "cell_hex"),
                    "area": float(fields["area"]),
                    "bbox": bbox,
                    "points": _wire_pair_list(
                        fields.get("points", ""), "points"),
                })
        except (KeyError, TypeError, ValueError) as exc:
            self.res.put({"kind": "error", "msg":
                          "invalid Rust pick response: %s" % exc})
            return
        self.res.put(output)

    def _emit_clip(self, fields):
        sequence = _wire_int(fields, "seq", -1)
        with self._jobs_lock:
            state = self._clip_jobs.pop(sequence, None)
        if state is None:
            return
        state["timer"].cancel()
        daemon_output = state["daemon"]
        destination = state["out"]
        staged = None
        try:
            parent = os.path.dirname(destination) or "."
            prefix = ".%s.floe-clip-" % (
                os.path.basename(destination) or "clip")
            descriptor, staged = tempfile.mkstemp(prefix=prefix, dir=parent)
            with open(daemon_output, "rb") as source, \
                    os.fdopen(descriptor, "wb") as target:
                signature = source.read(len(_OASIS_SIGNATURE))
                if signature != _OASIS_SIGNATURE:
                    raise ValueError("Rust clip is not an OASIS file")
                target.write(signature)
                shutil.copyfileobj(source, target, 1024 * 1024)
                target.flush()
                os.fsync(target.fileno())
            os.replace(staged, destination)
            staged = None
            size = os.path.getsize(destination)
            self.res.put({
                "kind": "clip", "path": destination,
                "size_mb": size / 1e6,
                "ms": _wire_int(
                    fields, "ms",
                    round((time.monotonic() - state["started"]) * 1000)),
            })
        except (OSError, ValueError) as exc:
            self.res.put({"kind": "error", "msg":
                          "publish Rust clip: %s" % exc})
        finally:
            if staged is not None:
                try:
                    os.unlink(staged)
                except OSError:
                    pass
            try:
                os.unlink(daemon_output)
            except OSError:
                pass

    def _emit_frame(self, fields):
        generation = _wire_int(fields, "gen", -1)
        with self._jobs_lock:
            state = self._jobs.get(generation)
        if state is None:
            return
        path = fields.get("png")
        read_started = time.monotonic()
        try:
            with open(path, "rb") as frame_file:
                png = frame_file.read()
            if not png.startswith(_PNG_SIGNATURE):
                raise ValueError("Rust frame is not a PNG")
        except (OSError, TypeError, ValueError) as exc:
            self.res.put({"kind": "error", "gen": generation,
                          "msg": "read Rust frame: %s" % exc})
            with self._jobs_lock:
                self._jobs.pop(generation, None)
            return
        finally:
            controlled = path == state["output"] or (
                isinstance(path, str) and
                path.startswith(state["output"] + ".gen-"))
            if controlled:
                try:
                    os.unlink(path)
                except OSError:
                    pass
        adapter_read_us = round((time.monotonic() - read_started) * 1e6)

        final = _wire_int(fields, "final") != 0
        if not final:
            # `partial=1 deferred=0` is still a progressive round. The wire's
            # final bit, not deferred's truthiness, decides settled state.
            refining = max(1, _wire_int(fields, "deferred"))
        else:
            refining = 0

        state["plan_us"] = _wire_int(fields, "plan_us")
        state["read_us"] += _wire_int(fields, "read_us")
        state["decode_us"] += _wire_int(fields, "decode_us")
        state["scene_us"] += _wire_int(fields, "scene_us")
        state["draw_us"] += _wire_int(fields, "raster_us")
        state["png_us"] += _wire_int(fields, "png_us")
        state["publish_write_us"] += _wire_int(
            fields, "publish_write_us")
        state["publish_sync_us"] += _wire_int(
            fields, "publish_sync_us")
        state["publish_rename_us"] += _wire_int(
            fields, "publish_rename_us")
        state["adapter_read_us"] += adapter_read_us
        state["new"] += _wire_int(fields, "cache_miss")
        state["cache_hit"] += _wire_int(fields, "cache_hit")
        state["frame_cache_hit"] += _wire_int(
            fields, "frame_cache_hit")
        state["render_tiles"] += _wire_int(fields, "tiles")
        state["resident_bytes"] = max(
            state["resident_bytes"], _wire_int(fields, "resident_bytes"))
        state["decode_workers"] = max(
            state["decode_workers"], _wire_int(fields, "decode_workers"))
        job = state["job"]
        deferred = _wire_int(fields, "deferred")
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
            # Exact phase telemetry for benchmark/field traces. The rounded
            # legacy fields above remain the GUI status contract.
            "read_ms": state["read_us"] / 1000.0,
            "decode_ms": state["decode_us"] / 1000.0,
            "scene_ms": state["scene_us"] / 1000.0,
            "raster_ms": state["draw_us"] / 1000.0,
            "png_ms": state["png_us"] / 1000.0,
            "publish_write_ms": state["publish_write_us"] / 1000.0,
            "publish_sync_ms": state["publish_sync_us"] / 1000.0,
            "publish_rename_ms": state["publish_rename_us"] / 1000.0,
            "publish_ms": (state["publish_write_us"] +
                           state["publish_sync_us"] +
                           state["publish_rename_us"]) / 1000.0,
            "adapter_read_ms": state["adapter_read_us"] / 1000.0,
            "cache_hit": state["cache_hit"],
            "cache_miss": state["new"],
            "frame_cache_hit": state["frame_cache_hit"],
            "resident_mb": state["resident_bytes"] / (1024.0 * 1024.0),
            "decode_workers": state["decode_workers"],
            "workers": _wire_int(fields, "workers"),
            "raster_jobs": self._raster_jobs_count,
            "render_tiles": state["render_tiles"],
            "tile_px": _wire_int(fields, "tile_px"),
            "frame_width": int(job.get("w", 0)),
            "frame_height": int(job.get("h", 0)),
            "wait_ms": 0,
            "ms": round((time.monotonic() - state["started"]) * 1000),
            "plan_ms": state["plan_us"] / 1000.0,
            "wc_cells": _wire_int(fields, "wc_cells"),
            "inst_edges": _wire_int(fields, "inst_edges"),
            "frame_rects": _wire_int(fields, "frame_rects"),
            "text_plan_ms": _wire_int(fields, "text_plan_us") / 1000.0,
            "text_place_records": _wire_int(fields, "text_place_records"),
            "labels": _wire_int(fields, "labels"),
            "label_tile_paints": _wire_int(fields, "label_tile_paints"),
            "label_pixel_paints": _wire_int(fields, "label_pixel_paints"),
        }
        if refining:
            output["refining"] = refining
        if _wire_int(fields, "labels_truncated") != 0:
            output["labels_truncated"] = True
        cut_px = max(0.0, float(job.get("cut_px") or 0.0))
        if cut_px:
            span_dbu = float(job["bbox"][2]) - float(job["bbox"][0])
            px_per_um = float(job["w"]) / max(
                1e-12, span_dbu * float(self.cache.meta["dbu"]))
            output["cut_um"] = round(cut_px / px_per_um, 3)
        self.res.put(output)
        if final:
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
