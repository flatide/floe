use super::*;

fn manifest() -> Manifest {
    Manifest {
        version: 1,
        source: PathBuf::from("/synthetic/deck.jb"),
        source_stamp: Stamp {
            dev: 1,
            ino: 2,
            len: 3,
            mtime: 4,
            mtime_ns: 5,
            ctime: 6,
            ctime_ns: 7,
        },
        revision: "a".repeat(32),
        levels: None,
        members: vec![Member {
            source: PathBuf::from("/synthetic/a.oas"),
            revision: "b".repeat(32),
        }],
    }
}

#[test]
fn manifest_rejects_ambiguous_or_unbounded_members_and_selector() {
    let good = manifest();
    assert_eq!(
        good.validate(&good.source).unwrap(),
        [PathBuf::from("/synthetic/a.oas")].into()
    );
    for kind in 0..8 {
        let mut bad = good.clone();
        match kind {
            0 => bad.version = 4,
            1 => bad.revision = "../escape".into(),
            2 => bad.members.push(bad.members[0].clone()),
            3 => bad.members[0].revision = "B".repeat(32),
            4 => bad.members[0].source = PathBuf::from("relative.oas"),
            5 => bad.members.clear(),
            6 => bad.levels = Some(BTreeSet::new()),
            _ => bad.levels = Some((0..4097).collect()),
        }
        assert!(bad.validate(&good.source).is_err(), "{kind}");
    }
    assert!(good.validate(Path::new("/synthetic/other.jb")).is_err());
}

#[test]
fn manifest_schema_is_closed_and_source_fields_are_required() {
    let good = serde_json::to_value(manifest()).unwrap();
    for key in ["version", "source", "source_stamp", "revision", "members"] {
        let mut missing = good.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<Manifest>(missing).is_err(),
            "{key}"
        );
    }
    let mut extra = good.clone();
    extra["directory"] = serde_json::json!("/outside");
    assert!(serde_json::from_value::<Manifest>(extra).is_err());
    let mut extra_member = good;
    extra_member["members"][0]["path"] = serde_json::json!("/outside");
    assert!(serde_json::from_value::<Manifest>(extra_member).is_err());
}
