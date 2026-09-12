//! Executed explicitly by tools/validate_worker_client.sh; missing assets are
//! errors, never a silently passing optional integration branch.
use floe_render_core::Cache;
use floe_worker_client::*;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn final_frame(worker: &mut WorkerClient, generation: u64) -> Frame {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(Instant::now() < deadline, "final frame timeout");
        match worker.poll(Duration::from_millis(50)).unwrap() {
            Some(Event::Frame(frame)) => {
                assert_eq!(frame.generation, generation, "stale frame escaped");
                if frame.final_frame {
                    assert!(frame.complete(), "incomplete frame");
                    return frame;
                }
            }
            Some(Event::Failed { code, message, .. }) => panic!("{code}: {message}"),
            _ => {}
        }
    }
}

#[test]
#[ignore = "run tools/validate_worker_client.sh SOURCE CACHE (builds a Python-adapter oracle)"]
fn native_frames_match_python_adapter_and_raw() {
    let cache_path =
        PathBuf::from(std::env::var_os("FLOE_WORKER_TEST_CACHE").expect("cache required"));
    let binary =
        PathBuf::from(std::env::var_os("FLOE_WORKER_TEST_RENDERD").expect("renderd required"));
    let oracle_dir = PathBuf::from(
        std::env::var_os("FLOE_WORKER_TEST_ORACLE").expect("oracle directory required"),
    );
    let cache = Cache::open(&cache_path).unwrap();
    let styles: Vec<_> = cache
        .layers()
        .iter()
        .map(|layer| Style {
            layer: (layer.layer, layer.datatype),
            color: [64, 192, 128, 255],
            fill: Fill::Solid,
            width: 1,
        })
        .collect();
    assert!(!styles.is_empty());
    // The oracle supplies a full-source bbox, not an assumed valmini extent.
    let view: Vec<f64> = std::fs::read_to_string(oracle_dir.join("view.txt"))
        .unwrap()
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    let mut worker = WorkerClient::spawn(Config::new(binary.clone())).unwrap();
    let work = worker.work_dir().to_owned();
    worker
        .open(Source::Layout(cache_path.clone()), 1024, 4)
        .unwrap();
    worker.set_styles(&styles).unwrap();
    let mut request = RenderRequest {
        view: view.try_into().unwrap(),
        width: 384,
        height: 384,
        raster_jobs: 4,
        decode_jobs: 4,
        format: FrameFormat::Png,
        frame_cache: false,
        ..Default::default()
    };
    let gen = worker.render(request.clone()).unwrap();
    let png = final_frame(&mut worker, gen);
    assert_eq!(
        png.bytes,
        std::fs::read(oracle_dir.join("frame.png")).unwrap(),
        "Rust client != Python adapter PNG"
    );
    request.format = FrameFormat::Raw;
    let gen = worker.render(request.clone()).unwrap();
    let raw = final_frame(&mut worker, gen);
    let rgba = png_pixels(&png.bytes, 384, 384);
    assert_eq!(&raw.bytes[16..], rgba);
    assert!(
        rgba.chunks_exact(4).filter(|p| p[..3] != [0, 0, 0]).count() > 100,
        "vacuous empty fixture"
    );
    // Intervening view, return, style changes and labels/frames all travel
    // through the real persistent client, not just an isolated raw handshake.
    request.view[0] += (request.view[2] - request.view[0]) * 0.1;
    let gen = worker.render(request.clone()).unwrap();
    final_frame(&mut worker, gen);
    request.view = raw.request.view;
    let gen = worker.render(request.clone()).unwrap();
    assert_eq!(final_frame(&mut worker, gen).bytes, raw.bytes);
    let recolored: Vec<_> = styles
        .iter()
        .cloned()
        .map(|mut s| {
            s.color = [192, 64, 128, 255];
            s
        })
        .collect();
    worker.set_styles(&recolored).unwrap();
    let gen = worker.render(request.clone()).unwrap();
    assert_ne!(final_frame(&mut worker, gen).bytes, raw.bytes);
    worker.set_styles(&styles).unwrap();
    // M0-D7: all-off is a complete blank frame, not an invalid-plan error.
    // A retained on -> off -> on sequence must not reuse the wrong visibility.
    request.frame_cache = true;
    let gen = worker.render(request.clone()).unwrap();
    assert_eq!(final_frame(&mut worker, gen).bytes, raw.bytes);
    request.layers = Layers::None;
    let gen = worker.render(request.clone()).unwrap();
    let blank = final_frame(&mut worker, gen);
    assert!(blank.bytes[16..]
        .chunks_exact(4)
        .all(|p| p == [0, 0, 0, 255]));
    // Structural depth-frontier frames do not belong to design layers.
    request.depth = Some(0);
    request.frames = true;
    let gen = worker.render(request.clone()).unwrap();
    let frames = final_frame(&mut worker, gen);
    assert!(frames.bytes[16..]
        .chunks_exact(4)
        .any(|p| p[..3] != [0, 0, 0]));
    request.depth = None;
    request.frames = false;
    request.layers = Layers::All;
    let gen = worker.render(request.clone()).unwrap();
    assert_eq!(final_frame(&mut worker, gen).bytes, raw.bytes);
    request.frame_cache = false;
    request.format = FrameFormat::Png;
    request.frames = true;
    request.labels = true;
    request.font_px = 19;
    request.depth = Some(1);
    request.cut_px = 1.;
    request.thin = ThinPolicy::Keep;
    let gen = worker.render(request.clone()).unwrap();
    assert_eq!(
        final_frame(&mut worker, gen).bytes,
        std::fs::read(oracle_dir.join("labels.png")).unwrap()
    );
    // Use a cold worker: cached pages can bypass the small round-page limit.
    worker.close().unwrap();
    assert!(!work.exists());
    let mut worker = WorkerClient::spawn(Config::new(binary)).unwrap();
    let work = worker.work_dir().to_owned();
    worker.open(Source::Layout(cache_path), 1024, 4).unwrap();
    worker.set_styles(&styles).unwrap();
    // Explicit small rounds must end with the same image as single-shot.
    request = raw.request;
    request.round_pages = 1;
    request.frame_cache = false;
    let gen = worker.render(request.clone()).unwrap();
    let streamed = final_frame(&mut worker, gen);
    assert_eq!(streamed.bytes, raw.bytes);
    assert!(
        streamed.round > 1,
        "expected a multi-page fixture to exercise refinement"
    );
    for _ in 0..20 {
        let gen = worker.render(request.clone()).unwrap();
        worker.cancel().unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < end);
            match worker.poll(Duration::from_millis(20)).unwrap() {
                Some(Event::CancelAcknowledged { .. }) => break,
                Some(Event::Frame(f)) => {
                    panic!("cancelled generation {gen} escaped as {}", f.generation)
                }
                _ => {}
            }
        }
    }
    let gen = worker.render(request).unwrap();
    assert_eq!(final_frame(&mut worker, gen).bytes, raw.bytes);
    worker.close().unwrap();
    assert!(!work.exists(), "private output files leaked");
}

fn png_pixels(bytes: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut offset = 8;
    while &bytes[offset + 4..offset + 8] != b"IDAT" {
        offset += 12 + u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
    }
    let len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
    let mut data = Vec::new();
    flate2::read::ZlibDecoder::new(&bytes[offset + 8..offset + 8 + len])
        .read_to_end(&mut data)
        .unwrap();
    assert_eq!(data.len(), (w * 4 + 1) * h);
    data.chunks_exact(w * 4 + 1)
        .flat_map(|row| {
            assert_eq!(row[0], 0);
            row[1..].iter().copied()
        })
        .collect()
}
