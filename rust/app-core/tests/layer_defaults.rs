//! Executed with private synthetic sources and GTK-generated path/text oracles.
use floe_app_core::{
    jobdeck::color::Mode,
    layer_defaults::Publisher,
    registered::{AccessScope, RegisteredSource},
};
use std::{
    fs,
    path::Path,
    sync::{atomic::AtomicUsize, Arc},
};

#[test]
#[ignore = "run tools/validate_layer_defaults.py for the GTK publication oracle"]
fn gtk_shared_default_targets_and_bytes_match() {
    let oracle: serde_json::Value = serde_json::from_slice(
        &fs::read(std::env::var_os("FLOE_DEFAULTS_ORACLE").unwrap()).unwrap(),
    )
    .unwrap();
    let cases = oracle.as_array().unwrap();
    assert_eq!(cases.len(), 20);
    let stop = AtomicUsize::new(0);
    for case in cases {
        let source = Path::new(case["source"].as_str().unwrap());
        let scope = AccessScope::new(&[source.parent().unwrap().to_owned()]).unwrap();
        let source = RegisteredSource::register(scope, source, &stop).unwrap();
        let publisher = Publisher::new(vec![Arc::clone(&source)]).unwrap();
        let draft = publisher
            .prepare(
                source,
                Mode::parse(case["mode"].as_str().unwrap()).unwrap(),
                case["text"].as_str().unwrap(),
                &stop,
            )
            .unwrap();
        let target = Path::new(case["target"].as_str().unwrap());
        assert_eq!(draft.target(), target);
        assert_eq!(
            fs::read(target).ok(),
            case["before"].as_str().map(|s| s.as_bytes().to_vec())
        );
        draft.publish(&stop).unwrap();
        assert_eq!(
            fs::read_to_string(target).unwrap(),
            case["text"].as_str().unwrap()
        );
    }
    println!("LAYER DEFAULTS: ALL OK (20 GTK targets/bytes, native publication)");
}
