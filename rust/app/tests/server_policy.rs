//! No TLS, source parsing, renderer or native indexer is involved here.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "floe-policy-cli-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // Exclusive creation: never adopt or erase a preexisting directory.
        fs::create_dir(&path).unwrap();
        let path = fs::canonicalize(path).unwrap();
        for name in ["data", "work", "runtime"] {
            fs::create_dir(path.join(name)).unwrap();
        }
        fs::write(
            path.join("data/synthetic.oas"),
            b"path-only synthetic fixture",
        )
        .unwrap();
        Self(path)
    }
    fn config(&self) -> Value {
        json!({"version":1,"public_origin":"https://example.test","runtime_root":self.0.join("runtime"),"max_sessions":4,
            "deployment":{"mode":"teebox","client_id":"teebox-service","user_namespace":"teebox",
                "shared_root":self.0.join("data"),"work_root":self.0.join("work"),
                "index":{"max_running":1,"max_entries":32,"jobs":8}}})
    }
    fn run(&self, config: &Value) -> Output {
        let path = self.0.join("policy.json");
        fs::write(&path, serde_json::to_vec(config).unwrap()).unwrap();
        Command::new(env!("CARGO_BIN_EXE_floe2-web"))
            .args(["server", "--check-config"])
            .arg(path)
            .current_dir(&self.0)
            .env("PATH", "")
            .env("FLOE_INDEX_BIN", "/nonexistent/preflight-must-not-spawn")
            .env("FLOE_RENDERD_BIN", "/nonexistent/preflight-must-not-spawn")
            .output()
            .unwrap()
    }
    fn assert_untouched(&self) {
        assert_eq!(fs::read_dir(self.0.join("work")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(self.0.join("runtime")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(self.0.join("data")).unwrap().count(), 1);
        assert_eq!(
            fs::read(self.0.join("data/synthetic.oas")).unwrap(),
            b"path-only synthetic fixture"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn check_config_is_read_only_and_never_claims_a_ready_service() {
    let f = Fixture::new();
    let mut config = f.config();
    for mode in ["teebox", "public_demo"] {
        if mode == "public_demo" {
            config["deployment"] = json!({"mode":"public_demo","data_root":f.0.join("data"),"samples":[{"id":"sample1","source":"synthetic.oas"}]});
        }
        let out = f.run(&config);
        assert!(out.status.success(), "{:?}", out);
        assert!(out.stderr.is_empty());
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["mode"], mode);
        assert_eq!(report["valid"], true);
        assert_eq!(report["runtime_ready"], false);
        assert_eq!(report["insecure_http_test"], false);
        assert!(!String::from_utf8(out.stdout)
            .unwrap()
            .contains(f.0.to_str().unwrap()));
        if mode == "public_demo" {
            assert_eq!(report["allowed_actions"], json!(["view"]));
            assert_eq!(report["demo_samples"], 1);
            assert!(report["index_policy"].is_null());
        }
        f.assert_untouched();
    }
}
#[test]
fn http_demo_preflight_is_opt_in_and_read_only() {
    let f = Fixture::new();
    let mut config = f.config();
    config["public_origin"] = json!("http://10.0.0.10:8080");
    config["deployment"] = json!({"mode":"public_demo","data_root":f.0.join("data"),"samples":[{"id":"sample1","source":"synthetic.oas"}]});
    assert!(!f.run(&config).status.success());
    config["deployment"]["allow_insecure_http"] = json!(true);
    let out = f.run(&config);
    assert!(out.status.success(), "{out:?}");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["insecure_http_test"], true);
    assert_eq!(report["allowed_actions"], json!(["view"]));
    assert_eq!(report["runtime_ready"], false);
    f.assert_untouched();
}
#[test]
fn invalid_config_fails_without_output_or_side_effects() {
    let f = Fixture::new();
    for (field, bad) in [
        ("public_origin", json!("http://example.test")),
        ("max_sessions", json!(0)),
        ("runtime_root", json!(f.0.join("data"))),
        ("password", json!("synthetic-must-not-echo")),
    ] {
        let mut config = f.config();
        config[field] = bad;
        let out = f.run(&config);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert!(!String::from_utf8(out.stderr)
            .unwrap()
            .contains("synthetic-must-not-echo"));
        f.assert_untouched();
    }
}
