use super::*;
use std::{fs, sync::atomic::AtomicU64};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "floe-svrf-unit-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn file(&self, name: &str, text: impl AsRef<[u8]>) -> PathBuf {
        let p = self.0.join(name);
        fs::write(&p, text).unwrap();
        p
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn parsed(s: &str) -> Deck {
    let tmp = Temp::new();
    let path = tmp.file("deck.svrf", s);
    parse_deck(
        &path,
        &Options {
            env_switches: false,
            ..Options::default()
        },
        &AtomicUsize::new(0),
    )
    .unwrap()
}

#[test]
fn lex_bounds_are_contiguous_and_ascii_safe() {
    assert_eq!(
        chain("m1 > 0<.05 ABUT>0<90", 3),
        vec![(">", "0"), ("<", ".05")]
    );
    assert_eq!(
        first_op("가나 ABUT<90 m1 <= -1e-3"),
        Some("가나 ABUT<90 m1 ".len())
    );
    assert_eq!(check("\"A.$1-2\" {x}"), Some(("A.$1-2", "x}")));
    assert!(check("\"잘못\" {}").is_none());
    assert_eq!(assign("a_b.x-1 = COPY M"), Some(("a_b.x-1", "COPY M")));
    for s in ["nan", "inf", "1_0", "1e", ".", "+", "1+2"] {
        assert_eq!(numeric(s).unwrap(), None, "{s}");
    }
    assert!(numeric("1e309").is_err());
    assert_eq!(numeric("-.25e+2").unwrap(), Some(-25.));
    assert!(signed("9223372036854775808").is_err());
}
#[test]
fn define_boundaries_longest_first_and_nonrecursive() {
    let mut sub = Substitution::default();
    sub.rebuild(&BTreeMap::from([
        ("A".into(), Some("B".into())),
        ("A-B".into(), Some("long".into())),
        ("B".into(), Some("C".into())),
        ("$A".into(), Some("cash".into())),
    ]))
    .unwrap();
    assert_eq!(
        sub.apply("A A-B B 한A A한 A_x x$A /$A/").unwrap(),
        "B long C 한A A한 A_x xcash /$B/"
    );
    sub.rebuild(&BTreeMap::from([("A".into(), Some("x".repeat(MAX_TEXT)))]))
        .unwrap();
    assert!(sub.apply("A A").is_err());
}
#[test]
fn graph_cycles_maps_redefinitions_and_wrapped_bounds() {
    let d = parsed("LAYER M 100\nLAYER MAP 7 DATATYPE 0 2 100\nVARIABLE V .031\nA = M OR B\nB = A OR unknown\nR { @ \"first\"\n x = INT A >0< V ABUT>0<90\n <= .1\n}\n");
    assert_eq!(d.checks["R"].source_gds, vec![(7, Some(0)), (7, Some(2))]);
    assert_eq!(d.checks["R"].unresolved, vec!["unknown"]);
    let c = &d.checks["R"].constraints;
    assert_eq!(
        c.iter().map(|c| c.value).collect::<Vec<_>>(),
        vec![Some(0.), Some(0.031), Some(0.1)]
    );
    assert_eq!(d.checks["R"].desc, "first");
    assert_eq!(d.stat("meas_no_bound"), 0);
}
#[test]
fn empty_descriptions_universal_newlines_and_invalid_utf8() {
    let tmp = Temp::new();
    let path = tmp.file("mixed", b"LAYER M 7\rR {\r\n@\n@\xff\rINT M < .5\n}");
    let d = parse_deck(&path, &Options::default(), &AtomicUsize::new(0)).unwrap();
    assert_eq!(d.stat("lines"), 6);
    assert_eq!(d.checks["R"].desc, "\n\u{fffd}");
    assert_eq!(d.checks["R"].source_gds, vec![(7, None)]);
}
#[test]
fn same_line_close_does_not_recurse() {
    let text = "A { } ".repeat(7000);
    let d = parsed(&text);
    assert_eq!(d.check_count(), 1);
    assert_eq!(d.warnings.len(), 6999);
}
#[test]
fn lazy_environment_precedence_scan_and_nonexecution() {
    let options = Options {
        defines: BTreeMap::from([("A".into(), Some("cli".into()))]),
        ..Options::default()
    };
    let stop = AtomicUsize::new(0);
    let env = |n: &str| match n {
        "A" => Some("env".into()),
        "B" => Some("yes".into()),
        _ => panic!("unexpected env lookup {n}"),
    };
    let mut p = Parser::new(&options, &stop, &env, Limits::default()).unwrap();
    for s in [
        "#IFDEF A cli",
        "#ENDIF",
        "#IFDEF B yes",
        "#ENDIF",
        "#IFDEF $B yes",
        "#ENDIF",
    ] {
        p.directive(s).unwrap();
    }
    assert_eq!(
        p.d.env_used,
        vec![("B".into(), "yes".into()), ("$B".into(), "yes".into())]
    );
    assert_eq!(p.d.defines["A"], Some("cli".into()));
    assert!(!p.d.defines.contains_key("$B"));
    let d = parsed("DMACRO x\n{\nFAKE { INT M < 2 }\n}\nVERBATIM {\nexec evil\n}\nREAL { INT M < 2 }\nCMACRO x\n");
    assert_eq!(d.check_count(), 1);
    assert!(d.checks.contains_key("REAL"));
    assert_eq!(d.stat("dmacro"), 1);
    assert_eq!(d.stat("cmacro"), 1);
}
#[test]
fn reduced_limits_fail_not_truncate() {
    let tmp = Temp::new();
    let path = tmp.file("deck", "LAYER M 1\nR { INT M < 2 }\n");
    let options = Options::default();
    let stop = AtomicUsize::new(0);
    let env = |_: &str| None;
    for limits in [
        Limits {
            input: 8,
            ..Limits::default()
        },
        Limits {
            model: 1,
            ..Limits::default()
        },
        Limits {
            graph: 1,
            ..Limits::default()
        },
    ] {
        let mut p = Parser::new(&options, &stop, &env, limits).unwrap();
        p.d.path = path.clone();
        let r = p.feed_file(&path, &mut Vec::new()).and_then(|_| p.finish());
        assert!(r.is_err());
    }
    let second = tmp.file("second", "LAYER M 1\n");
    let path = tmp.file("root", format!("INCLUDE {}\n", second.display()));
    for limits in [
        Limits {
            depth: 1,
            ..Limits::default()
        },
        Limits {
            files: 1,
            ..Limits::default()
        },
    ] {
        let mut p = Parser::new(&options, &stop, &env, limits).unwrap();
        assert!(p.feed_file(&path, &mut Vec::new()).is_err());
    }
}
#[test]
fn old_output_preserved_on_changed_input_cancel_or_input_alias() {
    let tmp = Temp::new();
    let path = tmp.file("deck", "R { INT M < 2 }\n");
    let out = tmp.file("out.json", "old output");
    let stop = AtomicUsize::new(0);
    let d = parse_deck(&path, &Options::default(), &stop).unwrap();
    assert!(d.write_json(&path, &stop).is_err());
    stop.store(2, Ordering::Relaxed);
    assert!(d.write_json(&out, &stop).is_err());
    stop.store(0, Ordering::Relaxed);
    fs::write(&path, "changed").unwrap();
    assert!(d.write_json(&out, &stop).is_err());
    assert_eq!(fs::read_to_string(&out).unwrap(), "old output");
    assert_eq!(fs::read_dir(&tmp.0).unwrap().count(), 2);
}
#[test]
fn graph_export_and_reader_contract() {
    let d = parsed("LAYER M 7\nD = M OR Missing\nR { @ test\n INT D < x\n}\n");
    let bytes = d.json_bytes(&AtomicUsize::new(0)).unwrap();
    let rules = super::super::Rules::parse(&bytes, &AtomicUsize::new(0)).unwrap();
    assert!(rules.rule("R").unwrap().includes_layer(7, 0));
    assert_eq!(rules.rule("R").unwrap().unresolved, vec!["Missing"]);
}
