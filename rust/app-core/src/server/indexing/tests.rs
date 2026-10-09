use super::*;
use serde_json::json;
use std::sync::Arc;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut bytes = [0; 12];
        getrandom::fill(&mut bytes).unwrap();
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let path = std::env::temp_dir().join(format!("floe-shared-index-{id}"));
        fs::create_dir(&path).unwrap();
        for name in ["a.oas", "b.oas", "c.oas"] {
            fs::write(path.join(name), b"synthetic metadata fixture").unwrap();
        }
        Self(path)
    }
    fn key(&self, name: &str, lod: bool) -> BuildKey {
        BuildKey::capture(
            &self.0.join(name),
            "synthetic-native-v1",
            &json!({"lod":lod,"occupancy":true}),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn queue(running: u16, entries: u16) -> Coordinator {
    Coordinator::new(IndexPolicy {
        max_running: running,
        max_entries: entries,
        jobs: 4,
    })
    .unwrap()
}
#[test]
fn concurrent_users_join_one_job_and_disconnect_does_not_cancel_it() {
    let f = Fixture::new();
    let q = Arc::new(queue(1, 1));
    let key = f.key("a.oas", false);
    let threads: Vec<_> = (0..32)
        .map(|_| {
            let q = Arc::clone(&q);
            let key = key.clone();
            std::thread::spawn(move || q.request(key).unwrap())
        })
        .collect();
    let submissions: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(submissions.iter().filter(|s| !s.joined).count(), 1);
    assert!(submissions.iter().all(|s| s.id == submissions[0].id));
    let id = submissions[0].id.clone();
    drop(submissions);
    let running = q.start_next().unwrap().unwrap();
    assert!(q.start_next().unwrap().is_none());
    assert!(q.request(key.clone()).unwrap().joined); // even when full/running
    assert!(q.request(f.key("b.oas", false)).is_err());
    q.finish(&running, true).unwrap();
    assert_eq!(q.snapshot(&id).unwrap().unwrap().phase, Phase::Succeeded);
    assert!(q.finish(&running, false).is_err());
    let fresh = q.request(key).unwrap(); // not a stale readiness cache
    assert!(!fresh.joined && fresh.id != id);
    assert!(q.snapshot(&id).unwrap().is_none()); // bounded terminal history
}
#[test]
fn source_profiles_serialize_while_other_sources_can_run() {
    let f = Fixture::new();
    let q = queue(2, 4);
    q.request(f.key("a.oas", false)).unwrap();
    let different = q.request(f.key("a.oas", true)).unwrap();
    assert!(!different.joined);
    q.request(f.key("b.oas", false)).unwrap();
    let a = q.start_next().unwrap().unwrap();
    let b = q.start_next().unwrap().unwrap();
    assert_ne!(a.key().source(), b.key().source());
    assert!(q.start_next().unwrap().is_none());
    q.finish(&b, true).unwrap();
    assert!(q.start_next().unwrap().is_none());
    q.finish(&a, false).unwrap();
    let next = q.start_next().unwrap().unwrap();
    assert_eq!(next.id(), &different.id);
    let foreign = queue(1, 2);
    assert!(foreign.finish(&next, true).is_err());
    assert!(foreign.snapshot(next.id()).unwrap().is_none());
}
#[test]
fn dropped_runner_never_frees_a_potentially_live_native_slot() {
    let f = Fixture::new();
    let q = queue(1, 2);
    let s = q.request(f.key("a.oas", false)).unwrap();
    drop(q.start_next().unwrap().unwrap());
    q.request(f.key("b.oas", false)).unwrap();
    assert!(q.start_next().unwrap().is_none());
    assert_eq!(q.snapshot(&s.id).unwrap().unwrap().phase, Phase::Running);
}
#[test]
fn build_identity_includes_source_stamp_native_version_and_full_options() {
    let f = Fixture::new();
    let key = f.key("a.oas", false);
    assert!(key.unchanged().unwrap());
    assert_eq!(
        key,
        BuildKey::capture(
            key.source(),
            "synthetic-native-v1",
            &json!({"occupancy":true,"lod":false})
        )
        .unwrap()
    );
    assert_ne!(key, f.key("a.oas", true));
    assert_ne!(
        key,
        BuildKey::capture(
            key.source(),
            "synthetic-native-v2",
            &json!({"lod":false,"occupancy":true})
        )
        .unwrap()
    );
    fs::write(key.source(), b"changed synthetic source, different length").unwrap();
    assert!(!key.unchanged().unwrap());
    assert_ne!(key, f.key("a.oas", false));
    for limits in [
        IndexPolicy {
            max_running: 2,
            max_entries: 2,
            jobs: 8,
        },
        IndexPolicy {
            max_running: 0,
            max_entries: 2,
            jobs: 4,
        },
        IndexPolicy {
            max_running: 1,
            max_entries: 257,
            jobs: 4,
        },
    ] {
        assert!(Coordinator::new(limits).is_err());
    }
}
