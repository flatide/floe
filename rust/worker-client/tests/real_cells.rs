//! The cell tree against a real daemon over a real cache (design.ovh built by
//! the indexer). Ignored by default: FLOE_CELLS_RENDERD names the renderd
//! binary, FLOE_CELLS_CACHE the `.<src>.ice` directory. Prints the answers.
use floe_worker_client::*;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn cell(w: &mut WorkerClient, request: CellRequest) -> CellReply {
    let seq = w.cell_query(request.clone()).unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < end, "{request:?} timed out");
        match w.poll(Duration::from_millis(20)).unwrap() {
            Some(Event::Cell { sequence, reply }) => {
                assert_eq!(sequence, seq);
                return reply.unwrap_or_else(|f| panic!("{request:?}: {f:?}"));
            }
            Some(Event::Failed { message, .. }) => panic!("{message}"),
            _ => (),
        }
    }
}
fn frame(w: &mut WorkerClient, request: RenderRequest) -> Frame {
    let gen = w.render(request).unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < end);
        match w.poll(Duration::from_millis(20)).unwrap() {
            Some(Event::Frame(f)) if f.final_frame => {
                assert_eq!(f.generation, gen);
                return f;
            }
            Some(Event::Failed { message, .. }) => panic!("{message}"),
            _ => (),
        }
    }
}

#[test]
#[ignore = "needs FLOE_CELLS_RENDERD and FLOE_CELLS_CACHE (a cache with design.ovh)"]
fn cell_tree_and_root_render_against_the_real_daemon() {
    let binary = PathBuf::from(std::env::var_os("FLOE_CELLS_RENDERD").expect("renderd binary"));
    let cache = PathBuf::from(std::env::var_os("FLOE_CELLS_CACHE").expect("cache directory"));
    let mut w = WorkerClient::spawn(Config::new(binary)).unwrap();
    let opened = w.open(Source::Layout(cache), 64, 2).unwrap();
    println!("opened unit={} max_depth={}", opened.unit, opened.max_depth);
    // Every layer of the cache, read off meta.json without a JSON crate:
    // `"layer": N, "datatype": M` pairs in order.
    let meta = std::fs::read_to_string(
        PathBuf::from(std::env::var_os("FLOE_CELLS_CACHE").unwrap()).join("meta.json"),
    )
    .unwrap();
    let number = |text: &str, key: &str| -> Option<u32> {
        let rest = text.split_once(&format!("\"{key}\":"))?.1.trim_start();
        rest.split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    };
    let styles: Vec<Style> = meta
        .split("\"layer\":")
        .skip(1)
        .filter_map(|chunk| {
            let layer = chunk
                .trim_start()
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()?;
            let layer = (layer, number(chunk, "datatype")?);
            Some((
                layer,
                Style {
                    layer,
                    color: [64, 192, 128, 255],
                    fill: Fill::Solid,
                    width: 1,
                },
            ))
        })
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .collect();
    println!(
        "styles: {:?}",
        styles.iter().map(|s| s.layer).collect::<Vec<_>>()
    );
    w.set_styles(&styles).unwrap();

    let sources = cell(&mut w, CellRequest::Sources);
    println!("cell_sources: {sources:?}");
    let CellReply::Sources(sources) = sources else {
        panic!()
    };
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source, 0);

    let top = cell(
        &mut w,
        CellRequest::Children {
            source: 0,
            cell: None,
        },
    );
    println!("cells (top): {top:?}");
    let CellReply::Children {
        cell: top_cell,
        name: top_name,
        unit,
        bbox: top_bbox,
        children,
        total,
        ..
    } = top
    else {
        panic!()
    };
    assert_eq!(children.len() as u64, total.min(CELLS_CHILD_CAP as u64));
    assert!(unit > 0.);
    let top_bbox = top_bbox.expect("the top holds shapes");

    let found = cell(
        &mut w,
        CellRequest::Find {
            source: None,
            pattern: "*".into(),
            limit: 10,
        },
    );
    println!("cell_find *: {found:?}");
    let CellReply::Find { total, matches } = found else {
        panic!()
    };
    assert!(total >= 1);
    assert!(matches.iter().any(|m| m.name == top_name));
    let empty = cell(
        &mut w,
        CellRequest::Find {
            source: None,
            pattern: String::new(),
            limit: 3,
        },
    );
    println!("cell_find (empty pattern): {empty:?}");

    // The first child, or the top itself when the layout is flat.
    let child = children.first().map_or(top_cell, |c| c.cell);
    let bbox = cell(
        &mut w,
        CellRequest::Bbox {
            source: 0,
            cell: child,
            root: None,
        },
    );
    println!("cell_bbox {child}: {bbox:?}");
    let CellReply::Bbox { insts, .. } = bbox else {
        panic!()
    };
    assert!(insts >= 1);

    let insts_reply = cell(
        &mut w,
        CellRequest::Insts {
            source: 0,
            cell: child,
            view: top_bbox,
            cap: 16,
            root: None,
        },
    );
    println!("cell_insts {child} over the top: {insts_reply:?}");
    let CellReply::Insts { boxes, .. } = insts_reply else {
        panic!()
    };
    assert!(!boxes.is_empty());

    // The child's own coordinates under it as the root.
    let rooted = cell(
        &mut w,
        CellRequest::Children {
            source: 0,
            cell: Some(child),
        },
    );
    println!("cells {child}: {rooted:?}");
    let CellReply::Children {
        bbox: Some(root_bbox),
        ..
    } = rooted
    else {
        panic!("the root cell holds shapes")
    };
    let under_root = cell(
        &mut w,
        CellRequest::Bbox {
            source: 0,
            cell: child,
            root: Some(child),
        },
    );
    println!("cell_bbox {child} root={child}: {under_root:?}");

    let request = RenderRequest {
        view: root_bbox,
        width: 96,
        height: 96,
        raster_jobs: 2,
        decode_jobs: 2,
        format: FrameFormat::Raw,
        root: Some(child),
        ..Default::default()
    };
    let f = frame(&mut w, request.clone());
    println!(
        "render root={child}: gen={} round={} complete={} bytes={} query_scene={:?}",
        f.generation,
        f.round,
        f.complete(),
        f.bytes.len(),
        f.query_scene().unwrap()
    );
    assert!(f.complete());
    assert_eq!(f.request.root, Some(child));
    assert_eq!(f.bytes.len(), 16 + 96 * 96 * 4);
    let painted = f.bytes[16..]
        .chunks(4)
        .filter(|p| p[..3] != [0, 0, 0])
        .count();
    println!("painted pixels under the root: {painted}");
    assert!(painted > 0, "the root's own shapes must draw");
    // The same request without a root draws the top: another picture.
    let plain = frame(
        &mut w,
        RenderRequest {
            root: None,
            ..request
        },
    );
    println!(
        "render top over the root's bbox: bytes equal={}",
        plain.bytes == f.bytes
    );
    w.close().unwrap();
}
