//! This test executable doubles as its own fake daemon. No Python, shell,
//! installed renderd or GUI is needed for lifecycle/error-path coverage.
use floe_worker_client::*;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, Write};
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--fixture") {
        fixture(&args[2], args.get(3));
        return;
    }
    let tests: &[(&str, fn())] = &[
        ("raw_png_styles_paths_and_cleanup", roundtrip),
        ("partial_and_final_are_distinct", partial),
        ("stale_frames_and_cancel_are_cleaned", stale),
        ("malformed_handshake_and_timeouts", startup_errors),
        ("frame_corruption_and_path_escape", frame_errors),
        ("daemon_error_is_not_cancelled", worker_error),
        ("bounded_stderr_and_blocked_stdin_shutdown", blocked_io),
        ("deck_open_and_capability", deck),
        ("shutdown_interrupts_startup_waits", interrupt_startup),
    ];
    for (name, test) in tests {
        test();
        println!("test {name} ... ok");
    }
    println!("worker lifecycle: {} passed", tests.len());
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "floe-client-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("cache 한 글")).unwrap();
        fs::write(path.join("cache 한 글/source-marker"), b"not changed").unwrap();
        Self(path)
    }
    fn config(&self, mode: &str) -> Config {
        let mut c = Config::new(std::env::current_exe().unwrap());
        c.args = vec![
            "--fixture".into(),
            mode.into(),
            self.0.as_os_str().to_owned(),
        ];
        c.temp_root = self.0.clone();
        c.ready_timeout = Duration::from_secs(2);
        c.open_timeout = Duration::from_millis(300);
        c.style_timeout = Duration::from_millis(300);
        c.render_timeout = Duration::from_millis(500);
        c.shutdown_grace = Duration::from_millis(50);
        c
    }
    fn worker(&self, mode: &str) -> WorkerClient {
        let mut w = WorkerClient::spawn(self.config(mode)).unwrap();
        let opened = w
            .open(Source::Layout(self.0.join("cache 한 글")), 32, 2)
            .unwrap();
        assert_eq!(opened.unit, 1000.);
        w.set_styles(&[style()]).unwrap();
        w
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn style() -> Style {
    Style {
        layer: (1, 0),
        color: [0, 255, 0, 255],
        fill: Fill::Solid,
        width: 1,
    }
}
fn request(format: FrameFormat) -> RenderRequest {
    RenderRequest {
        width: 2,
        height: 2,
        format,
        ..Default::default()
    }
}
fn frame(w: &mut WorkerClient) -> Frame {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        assert!(Instant::now() < deadline, "frame deadline");
        match w.poll(Duration::from_millis(50)).unwrap() {
            Some(Event::Frame(f)) => return f,
            Some(Event::Failed { code, message, .. }) => panic!("{code}: {message}"),
            _ => {}
        }
    }
}
fn no_outputs(w: &WorkerClient) {
    let names: Vec<_> = fs::read_dir(w.work_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("source")]);
}
fn roundtrip() {
    let tmp = Temp::new();
    let mut w = tmp.worker("normal");
    let work = w.work_dir().to_owned();
    no_outputs(&w);
    for format in [FrameFormat::Raw, FrameFormat::Png] {
        let gen = w.render(request(format)).unwrap();
        let f = frame(&mut w);
        assert_eq!(f.generation, gen);
        assert!(f.complete());
        if format == FrameFormat::Raw {
            assert_eq!(f.bytes.len(), 32);
        } else {
            assert!(f.bytes.starts_with(b"\x89PNG"));
        }
        no_outputs(&w);
    }
    assert_eq!(w.set_styles(&[style()]).unwrap(), 2);
    w.close().unwrap();
    w.close().unwrap();
    assert_eq!(w.set_styles(&[style()]).unwrap_err().kind, ErrorKind::State);
    assert_eq!(
        w.render(request(FrameFormat::Raw)).unwrap_err().kind,
        ErrorKind::State
    );
    assert_eq!(
        w.open(Source::Layout(tmp.0.join("cache 한 글")), 32, 1)
            .unwrap_err()
            .kind,
        ErrorKind::State
    );
    assert!(!work.exists());
    assert_eq!(
        fs::read(tmp.0.join("cache 한 글/source-marker")).unwrap(),
        b"not changed"
    );
}
fn partial() {
    let tmp = Temp::new();
    let mut w = tmp.worker("partial");
    w.render(request(FrameFormat::Raw)).unwrap();
    let first = frame(&mut w);
    assert!(!first.final_frame && first.partial && first.deferred == 0 && !first.complete());
    let final_frame = frame(&mut w);
    assert!(final_frame.complete());
    assert_eq!(final_frame.round, 2);
    no_outputs(&w);
    let mut w = tmp.worker("partial_final");
    w.render(request(FrameFormat::Raw)).unwrap();
    let f = frame(&mut w);
    assert!(f.final_frame && !f.complete());
}
fn stale() {
    let tmp = Temp::new();
    let mut w = tmp.worker("normal");
    w.render(request(FrameFormat::Raw)).unwrap();
    let gen = w.render(request(FrameFormat::Raw)).unwrap();
    assert_eq!(frame(&mut w).generation, gen);
    no_outputs(&w);
    w.render(request(FrameFormat::Raw)).unwrap();
    let frontier = w.cancel().unwrap();
    loop {
        match w.poll(Duration::from_millis(50)).unwrap() {
            Some(Event::CancelAcknowledged { before_generation }) => {
                assert_eq!(before_generation, frontier);
                break;
            }
            Some(Event::Frame(_)) => panic!("cancelled frame escaped"),
            _ => {}
        }
    }
    no_outputs(&w);
    w.render(request(FrameFormat::Raw)).unwrap();
    assert!(frame(&mut w).complete());
}
fn startup_errors() {
    let tmp = Temp::new();
    for (mode, expected) in [
        ("version", ErrorKind::Version),
        ("utf8", ErrorKind::Protocol),
        ("oversize", ErrorKind::Protocol),
        ("duplicate", ErrorKind::Protocol),
        ("eof", ErrorKind::Exited),
        ("ready_timeout", ErrorKind::Timeout),
    ] {
        let mut c = tmp.config(mode);
        c.ready_timeout = Duration::from_millis(300);
        let e = WorkerClient::spawn(c).err().expect("startup must fail");
        assert_eq!(e.kind, expected, "{mode}: {e}");
        assert_eq!(
            fs::read_dir(&tmp.0).unwrap().count(),
            1,
            "startup directory leak"
        );
    }
    for (mode, expected) in [
        ("open_error", ErrorKind::Worker),
        ("open_timeout", ErrorKind::Timeout),
    ] {
        let mut w = WorkerClient::spawn(tmp.config(mode)).unwrap();
        let work = w.work_dir().to_owned();
        assert_eq!(
            w.open(Source::Layout(tmp.0.join("cache 한 글")), 32, 2)
                .unwrap_err()
                .kind,
            expected
        );
        assert!(!work.exists());
    }
    let mut w = WorkerClient::spawn(tmp.config("style_error")).unwrap();
    w.open(Source::Layout(tmp.0.join("cache 한 글")), 32, 1)
        .unwrap();
    assert_eq!(
        w.set_styles(&[style()]).unwrap_err().kind,
        ErrorKind::Worker
    );
}
fn frame_errors() {
    let tmp = Temp::new();
    let outside = tmp.0.join("outside");
    fs::write(&outside, b"must stay").unwrap();
    for mode in [
        "escape",
        "symlink",
        "short",
        "dimensions",
        "oversize_frame",
        "crc",
        "render_timeout",
        "render_eof",
        "bad_round",
    ] {
        let mut w = tmp.worker(mode);
        let work = w.work_dir().to_owned();
        let format = if mode == "crc" {
            FrameFormat::Png
        } else {
            FrameFormat::Raw
        };
        w.render(request(format)).unwrap();
        let mut failure = None;
        for _ in 0..50 {
            match w.poll(Duration::from_millis(50)) {
                Err(e) => {
                    failure = Some(e);
                    break;
                }
                Ok(Some(Event::Frame(_))) => panic!("bad frame accepted: {mode}"),
                _ => {}
            }
        }
        let error = failure.expect(mode);
        if mode == "render_timeout" {
            assert_eq!(error.kind, ErrorKind::Timeout);
        }
        if mode == "render_eof" {
            assert_eq!(error.kind, ErrorKind::Exited);
        }
        assert!(!work.exists(), "{mode}: cleanup");
        assert_eq!(fs::read(&outside).unwrap(), b"must stay", "{mode}");
    }
}
fn worker_error() {
    let tmp = Temp::new();
    let mut w = tmp.worker("render_error");
    let first = w.render(request(FrameFormat::Raw)).unwrap();
    w.render(request(FrameFormat::Raw)).unwrap();
    match w.poll(Duration::from_secs(1)).unwrap().unwrap() {
        Event::Failed {
            generation,
            code,
            message,
        } => {
            assert_eq!(generation, Some(first));
            assert_eq!(code, "render");
            assert_eq!(message, "ENOSPC");
        }
        other => panic!("error was disguised: {other:?}"),
    }
}
fn blocked_io() {
    let tmp = Temp::new();
    let mut w = tmp.worker("stderr");
    w.render(request(FrameFormat::Raw)).unwrap();
    frame(&mut w);
    assert!(!w.stderr_tail().is_empty());
    assert!(w.stderr_tail().len() <= 8192);
    w.close().unwrap();
    let mut w = tmp.worker("stall_writer");
    let work = w.work_dir().to_owned();
    let mut r = request(FrameFormat::Raw);
    r.layers = Layers::Only((0..4096).map(|i| (i, 0)).collect());
    let start = Instant::now();
    let mut busy = false;
    for _ in 0..100 {
        if let Err(e) = w.render(r.clone()) {
            assert_eq!(e.kind, ErrorKind::Busy);
            busy = true;
            break;
        }
    }
    assert!(busy);
    w.close().unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!work.exists());
}
fn deck() {
    let tmp = Temp::new();
    let path = tmp.0.join("deck 한 글.spec");
    fs::write(&path, b"fixture").unwrap();
    let mut w = WorkerClient::spawn(tmp.config("normal")).unwrap();
    assert!(w.open(Source::Deck(path), 32, 1).unwrap().is_deck);
    w.set_styles(&[style()]).unwrap();
    let mut r = request(FrameFormat::Raw);
    r.labels = true;
    assert_eq!(w.render(r).unwrap_err().kind, ErrorKind::InvalidInput);
}

fn interrupt_startup() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    for mode in [
        "ready_timeout",
        "open_timeout",
        "style_timeout",
        "render_timeout",
    ] {
        let tmp = Temp::new();
        let flag = Arc::new(AtomicUsize::new(0));
        let signal = Arc::clone(&flag);
        let mut config = tmp.config(mode);
        config.shutdown_requested = Some(flag);
        config.ready_timeout = Duration::from_secs(30);
        config.open_timeout = Duration::from_secs(30);
        config.style_timeout = Duration::from_secs(30);
        config.render_timeout = Duration::from_secs(30);
        let start = Instant::now();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            signal.store(2, Ordering::Relaxed);
        });
        let result = (|| -> Result<()> {
            let mut w = WorkerClient::spawn(config)?;
            w.open(Source::Layout(tmp.0.join("cache 한 글")), 32, 1)?;
            w.set_styles(&[style()])?;
            w.render(request(FrameFormat::Raw))?;
            loop {
                w.poll(Duration::from_millis(20))?;
            }
        })();
        trigger.join().unwrap();
        assert_eq!(result.unwrap_err().kind, ErrorKind::Cancelled, "{mode}");
        assert!(start.elapsed() < Duration::from_secs(2), "{mode}");
        assert!(fs::read_dir(&tmp.0).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("floe-worker-")));
    }
}

fn reply(line: &str) {
    println!("{line}");
    std::io::stdout().flush().unwrap();
}
fn fixture(mode: &str, root: Option<&String>) {
    match mode {
        "version" => {
            reply("ready version=wrong");
            return;
        }
        "utf8" => {
            std::io::stdout()
                .write_all(b"ready version=\xff\n")
                .unwrap();
            return;
        }
        "oversize" => {
            reply(&"a".repeat(65538));
            return;
        }
        "duplicate" => {
            reply("ready version=a version=b");
            return;
        }
        "eof" => return,
        "ready_timeout" => {
            std::thread::sleep(Duration::from_secs(30));
            return;
        }
        _ => {}
    }
    reply(&format!(
        "ready version={} git=fixture flavor=test",
        EXPECTED_RENDERD_VERSION
    ));
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let mut words = line.split_whitespace();
        let command = words.next().unwrap();
        let fields: BTreeMap<_, _> = words.map(|w| w.split_once('=').unwrap()).collect();
        match command {
            "open" => {
                if mode == "open_timeout" {
                    std::thread::sleep(Duration::from_secs(30));
                    continue;
                }
                if mode == "open_error" {
                    reply("error code=open message=bad_marker");
                } else {
                    reply("opened unit=1000 max_depth=6");
                }
            }
            "style" => {
                if mode == "style_timeout" {
                    std::thread::sleep(Duration::from_secs(30));
                    continue;
                }
                assert!(fs::read_to_string(fields["path"]).unwrap().contains("1/0"));
                if mode == "style_error" {
                    reply("error code=style message=bad_style");
                } else {
                    reply(&format!("styled epoch={} layers=1", fields["epoch"]));
                }
                if mode == "stall_writer" {
                    std::thread::sleep(Duration::from_secs(30));
                }
            }
            "render" => {
                if mode == "render_timeout" {
                    std::thread::sleep(Duration::from_secs(30));
                    continue;
                }
                if mode == "render_eof" {
                    return;
                }
                if mode == "render_error" {
                    reply(&format!(
                        "error code=render gen={} message=ENOSPC",
                        fields["gen"]
                    ));
                    continue;
                }
                if mode == "stderr" {
                    std::io::stderr()
                        .write_all(&vec![b'x'; 2 * 1024 * 1024])
                        .unwrap();
                }
                let w: u32 = fields["w"].parse().unwrap();
                let h: u32 = fields["h"].parse().unwrap();
                let mut raw = b"FLOERAW1".to_vec();
                raw.extend(w.to_le_bytes());
                raw.extend(h.to_le_bytes());
                raw.extend(vec![255; (w * h * 4) as usize]);
                let mut bytes = if fields["frame_format"] == "png" {
                    png(w, h)
                } else {
                    raw
                };
                if mode == "short" {
                    bytes.pop();
                }
                if mode == "dimensions" {
                    bytes[8] = 99;
                }
                if mode == "crc" {
                    bytes[29] ^= 1;
                }
                let rounds = if mode == "partial" { 2 } else { 1 };
                for round in 1..=rounds {
                    let final_frame = round == rounds;
                    let mut path = PathBuf::from(fields["out"]);
                    if !final_frame {
                        path = PathBuf::from(format!(
                            "{}.gen-{}.round-{round}.partial.{}",
                            fields["out"], fields["gen"], fields["frame_format"]
                        ));
                    }
                    if mode == "escape" {
                        path = PathBuf::from(root.unwrap()).join("outside");
                    } else if mode == "symlink" {
                        symlink(PathBuf::from(root.unwrap()).join("outside"), &path).unwrap();
                    } else if mode == "oversize_frame" {
                        fs::File::create(&path)
                            .unwrap()
                            .set_len(81 * 1024 * 1024)
                            .unwrap();
                    } else {
                        fs::write(&path, &bytes).unwrap();
                    }
                    let partial = !final_frame || mode == "partial_final";
                    let r = if mode == "bad_round" { 0 } else { round };
                    reply(&format!("frame gen={} round={r} final={} partial={} deferred=0 labels_truncated=0 style_epoch={} format={} png={}", fields["gen"], u8::from(final_frame), u8::from(partial), fields["style_epoch"], fields["frame_format"], path.display()));
                }
            }
            "cancel" => reply(&format!("cancelled before_gen={}", fields["before_gen"])),
            "quit" => return,
            _ => panic!("unexpected command"),
        }
    }
}
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = w.to_be_bytes().to_vec();
    header.extend(h.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    for _ in 0..h {
        encoder.write_all(&[0]).unwrap();
        encoder.write_all(&vec![255; (w * 4) as usize]).unwrap();
    }
    chunk(&mut out, b"IDAT", &encoder.finish().unwrap());
    chunk(&mut out, b"IEND", &[]);
    out
}
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let begin = out.len();
    out.extend(kind);
    out.extend(data);
    out.extend(crc32fast::hash(&out[begin..]).to_be_bytes());
}
