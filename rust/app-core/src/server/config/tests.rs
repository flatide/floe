use super::*;
use std::os::unix::fs::symlink;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut random = [0; 12];
        getrandom::fill(&mut random).unwrap();
        let id: String = random.iter().map(|n| format!("{n:02x}")).collect();
        let path = std::env::temp_dir().join(format!("floe-server-policy-{id}"));
        fs::create_dir(&path).unwrap();
        for name in ["data", "work", "runtime", "outside"] {
            fs::create_dir(path.join(name)).unwrap();
        }
        fs::write(
            path.join("data/synthetic.oas"),
            b"synthetic path-only fixture",
        )
        .unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn config(&self) -> Config {
        Config {
            version: 1,
            public_origin: "https://192.0.2.10".into(),
            runtime_root: self.0.join("runtime"),
            max_sessions: 4,
            deployment: Deployment::TeeBox {
                client_id: "teebox-service".into(),
                user_namespace: "teebox".into(),
                shared_root: self.0.join("data"),
                work_root: self.0.join("work"),
                index: IndexPolicy {
                    max_running: 1,
                    max_entries: 32,
                    jobs: 8,
                },
            },
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn shared_sources_personal_work_and_machine_identity_are_separate() {
    let f = Fixture::new();
    let policy = f.config().validate().unwrap();
    let a = policy.principal("teebox-service", "alice").unwrap();
    let b = policy.principal("teebox-service", "bob").unwrap();
    assert_ne!(a.subject(), "teebox-service");
    assert!(policy.principal("alice", "bob").is_err());
    assert_eq!(
        policy.resolve_source("synthetic.oas").unwrap(),
        f.0.join("data/synthetic.oas")
    );
    assert_ne!(
        policy.personal_directory(&a, "layout1").unwrap(),
        policy.personal_directory(&b, "layout1").unwrap()
    );
    assert!(policy
        .personal_directory(&a, "layout1")
        .unwrap()
        .starts_with(f.0.join("work")));
    assert!(!f.0.join("work/users").exists()); // preflight never creates storage
    assert!(policy.personal_directory(&a, "../layout1").is_err());
    assert!(policy
        .personal_directory(&Principal::delegated("other", "alice").unwrap(), "layout1")
        .is_err());
    assert!(policy.allows(Action::RequestIndex) && policy.allows(Action::WriteReview));
    assert!(!policy.allows(Action::PublishDefaults));
}
#[test]
fn root_escapes_aliases_and_implicit_scope_are_rejected() {
    let f = Fixture::new();
    fs::write(f.0.join("outside/secret.oas"), b"synthetic outside").unwrap();
    symlink(f.0.join("outside"), f.0.join("data/link")).unwrap();
    let p = f.config().validate().unwrap();
    for bad in [
        "../outside/secret.oas",
        "/synthetic.oas",
        "./synthetic.oas",
        "a//b",
        "a/../b",
        "link/secret.oas",
        "a\\b",
        "",
        "\0",
    ] {
        assert!(p.resolve_source(bad).is_err(), "{bad:?}");
    }
    let mut c = f.config();
    c.runtime_root = f.0.join("data");
    assert!(c.validate().is_err());
    let mut c = f.config();
    c.runtime_root = "relative".into();
    assert!(c.validate().is_err());
    let mut c = f.config();
    c.runtime_root = "/".into();
    assert!(c.validate().is_err());
    symlink(f.0.join("data"), f.0.join("alias")).unwrap();
    let mut c = f.config();
    c.runtime_root = f.0.join("alias");
    assert!(c.validate().is_err());
    let mut c = f.config();
    c.version = 2;
    assert!(c.validate().is_err());
    let mut c = f.config();
    c.max_sessions = 0;
    assert!(c.validate().is_err());
    let mut c = f.config();
    c.public_origin = "http://example.test".into();
    assert!(c.validate().is_err());
}
#[test]
fn anonymous_demo_is_a_sample_allowlist_not_a_browser_path_or_writer() {
    let f = Fixture::new();
    let mut c = f.config();
    c.deployment = Deployment::PublicDemo {
        allow_insecure_http: false,
        data_root: f.0.join("data"),
        samples: vec![Sample {
            id: "demo1".into(),
            source: "synthetic.oas".into(),
        }],
    };
    let policy = c.clone().validate().unwrap();
    assert_eq!(
        policy.resolve_source("demo1").unwrap(),
        f.0.join("data/synthetic.oas")
    );
    assert!(policy.resolve_source("synthetic.oas").is_err());
    assert!(policy.principal("teebox-service", "alice").is_err());
    assert!(policy
        .personal_directory(&Principal::delegated("teebox", "alice").unwrap(), "layout1")
        .is_err());
    assert!(policy.allows(Action::View));
    for denied in [
        Action::Browse,
        Action::RequestIndex,
        Action::ReadOwnReview,
        Action::WriteReview,
        Action::Share,
        Action::ExportGeometry,
        Action::PublishDefaults,
    ] {
        assert!(!policy.allows(denied));
    }
    if let Deployment::PublicDemo { samples, .. } = &mut c.deployment {
        samples.push(samples[0].clone());
    }
    assert!(c.validate().is_err());
}
#[test]
fn http_demo_requires_opt_in_and_does_not_grant_teebox_or_write_authority() {
    let f = Fixture::new();
    let mut c = f.config();
    c.public_origin = "http://10.0.0.10:8080".into();
    assert!(c.clone().validate().is_err());
    c.deployment = Deployment::PublicDemo {
        data_root: f.0.join("data"),
        samples: vec![Sample {
            id: "demo1".into(),
            source: "synthetic.oas".into(),
        }],
        allow_insecure_http: false,
    };
    assert!(c.clone().validate().is_err());
    if let Deployment::PublicDemo {
        allow_insecure_http,
        ..
    } = &mut c.deployment
    {
        *allow_insecure_http = true;
    }
    let p = c.clone().validate().unwrap();
    assert!(p.http_test() && p.allows(Action::View));
    assert!(!p.allows(Action::RequestIndex) && !p.allows(Action::WriteReview));
    assert!(p.principal("teebox", "alice").is_err());
    c.public_origin = "https://example.test".into();
    assert!(
        c.validate().is_err(),
        "HTTP opt-in must not silently change HTTPS policy"
    );
    let mut v = serde_json::to_value(f.config()).unwrap();
    v["deployment"]["allow_insecure_http"] = serde_json::json!(true);
    assert!(
        serde_json::from_value::<Config>(v).is_err(),
        "TeeBox cannot opt into HTTP"
    );
}
#[test]
fn strict_bounded_config_reader_has_no_password_or_unknown_options() {
    let f = Fixture::new();
    let path = f.0.join("config.json");
    let value = serde_json::to_value(f.config()).unwrap();
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    Config::read(&path).unwrap().validate().unwrap();
    for key in ["password", "user_id", "automatic_gc", "listen"] {
        let mut v = value.clone();
        v[key] = serde_json::json!("not accepted");
        fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(Config::read(&path).is_err());
    }
    fs::write(&path, vec![b' '; CONFIG_BYTES as usize + 1]).unwrap();
    assert!(Config::read(&path).is_err());
    symlink(&path, f.0.join("config-link")).unwrap();
    assert!(Config::read(&f.0.join("config-link")).is_err());
    assert!(Config::read(&f.0.join("data")).is_err());
}

#[test]
fn personal_namespaces_cannot_alias_other_users_through_links() {
    let f = Fixture::new();
    let policy = f.config().validate().unwrap();
    let alice = policy.principal("teebox-service", "alice").unwrap();
    let bob = policy.principal("teebox-service", "bob").unwrap();
    let a = policy.personal_directory(&alice, "layout1").unwrap();
    let b = policy.personal_directory(&bob, "layout1").unwrap();
    fs::create_dir_all(b.parent().unwrap()).unwrap();
    symlink(b.parent().unwrap(), a.parent().unwrap()).unwrap();
    assert!(policy.personal_directory(&alice, "layout1").is_err());
    assert!(policy.personal_directory(&bob, "layout1").is_ok());
}
