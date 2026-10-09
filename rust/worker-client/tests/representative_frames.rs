//! Real OVR2 frames, supplied by validate_representatives.py. No mock daemon.
use floe_worker_client::*;
use std::time::{Duration, Instant};

#[test]
#[ignore = "run tools/validate_representatives.py"]
fn representative_cursor_preserves_scene_completion_and_nonpickable_samples() {
    std::env::set_var("FLOE_RUST_REPRESENTATIVES", "on");
    std::env::set_var("FLOE_RUST_REPRESENTATIVES_BATCH", "3");
    std::env::set_var("FLOE_RUST_REPRESENTATIVES_DIRECT", "off");
    std::env::set_var("FLOE_RUST_OCCUPANCY", "off");
    std::env::set_var("FLOE_RUST_RETAINED_MB", "0");
    std::env::remove_var("FLOE_RUST_REPRESENTATIVES_MERGE");
    std::env::remove_var("FLOE_RUST_PAGE_REPS");
    std::env::remove_var("FLOE_RUST_SUB_CUT_WASH");
    let binary = std::env::var_os("FLOE_REP_QUERY_RENDERD").expect("synthetic daemon");
    let cache = std::env::var_os("FLOE_REP_QUERY_CACHE").expect("synthetic OVR2 cache");
    let mut worker = WorkerClient::spawn(Config::new(std::path::PathBuf::from(binary))).unwrap();
    let work = worker.work_dir().to_owned();
    worker.open(Source::Layout(cache.into()), 64, 2).unwrap();
    worker
        .set_styles(&[Style {
            layer: (1, 0),
            color: [255; 4],
            fill: Fill::Solid,
            width: 1,
        }])
        .unwrap();
    let generation = worker
        .render(RenderRequest {
            view: [0., 0., 512000., 512000.],
            width: 512,
            height: 512,
            cut_px: 3.,
            thin: ThinPolicy::Cull,
            format: FrameFormat::Raw,
            raster_jobs: 2,
            decode_jobs: 2,
            frame_cache: false,
            ..Default::default()
        })
        .unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    let mut previews = 0;
    let final_scene = loop {
        assert!(Instant::now() < end, "representative frame timeout");
        match worker.poll(Duration::from_millis(20)).unwrap() {
            Some(Event::Frame(frame)) => {
                assert_eq!(frame.generation, generation);
                let context = frame.query_scene().unwrap();
                assert_eq!(
                    context.id.unwrap(),
                    SceneId {
                        generation,
                        round: frame.round
                    }
                );
                assert_eq!(frame.fields.get("pages"), Some("0"));
                assert!(frame.fields.get("wall_us").unwrap().parse::<u64>().unwrap() > 0);
                if frame.final_frame {
                    assert!(frame.complete());
                    assert!(context.complete);
                    assert_eq!(frame.round, 2);
                    assert_eq!(previews, 1);
                    assert!(frame.bytes[16..].chunks_exact(4).any(|p| p[0] != 0));
                    break context.id.unwrap();
                }
                previews += 1;
                assert!(frame.partial);
                assert!(
                    !context.complete,
                    "pending OVR cursor was advertised as complete"
                );
                assert!(!context.queryable());
            }
            Some(Event::Failed { message, .. }) => panic!("{message}"),
            _ => (),
        }
    };
    let sequence = worker
        .query(QueryRequest {
            scene: final_scene,
            operation: QueryOperation::Pick { nth: 0 },
            x: 1000,
            y: 1000,
            radius: 1000,
            layers: Layers::All,
        })
        .unwrap();
    loop {
        assert!(Instant::now() < end, "representative query timeout");
        match worker.poll(Duration::from_millis(20)).unwrap() {
            Some(Event::Query(reply)) => {
                assert_eq!(reply.sequence, sequence);
                assert_eq!(reply.status, QueryStatus::Ok);
                assert!(reply.hit.is_none(), "display samples became query geometry");
                break;
            }
            Some(Event::Failed { message, .. }) => panic!("{message}"),
            _ => (),
        }
    }
    worker.close().unwrap();
    assert!(!work.exists());
    println!("RUST OVR QUERY COMPLETION: ALL OK");
}
