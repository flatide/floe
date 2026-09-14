//! Actual GTK release math + production JS gesture inputs, supplied by the gate.
use floe_app_core::view::{Patch, Viewport};
use floe_web::view::PatchDto;
use serde_json::Value;

#[test]
#[ignore = "run tools/validate_zoom_band.py with the GTK/JS fixture"]
fn gtk_release_coordinates_match_web_gesture_and_rust_navigation() {
    let path = std::env::var_os("FLOE_BAND_CASES").expect("private cases required");
    let cases: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(cases.len() >= 300);
    for (i, c) in cases.iter().enumerate() {
        let a = |v: &Value| -> [f64; 4] { std::array::from_fn(|j| v[j].as_f64().unwrap()) };
        let before = Viewport::new(
            a(&c["bbox"]),
            c["pixels"][0].as_u64().unwrap() as u32,
            c["pixels"][1].as_u64().unwrap() as u32,
        )
        .unwrap();
        let patch: Patch = if c["navigation"].is_null() {
            Patch::default()
        } else {
            serde_json::from_value::<PatchDto>(serde_json::json!({"navigation":c["navigation"]}))
                .unwrap()
                .core()
                .unwrap()
        };
        let after = patch
            .navigation
            .map_or(Ok(before), |nav| before.navigate(nav, [0.; 4], 1.))
            .unwrap();
        let want = a(&c["expected"]);
        for (j, (&got, &expected)) in after.bbox.iter().zip(want.iter()).enumerate() {
            let tolerance = 32.
                * f64::EPSILON
                * before
                    .bbox
                    .iter()
                    .chain(want.iter())
                    .fold(1_f64, |n, x| n.max(x.abs()));
            assert!(
                (got - expected).abs() <= tolerance,
                "case {i} axis {j}: {got} != {expected}; {c}"
            );
        }
    }
    println!("ZOOM BAND GTK/JS/RUST: ALL OK ({} cases)", cases.len());
}
