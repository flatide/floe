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
fn comments_follow_source_order_and_preserve_quoted_literals() {
    for (input, expected) in [
        ("//******** /* never opened", ""),
        ("LAYER M 1 // /* ignored", "LAYER M 1 "),
        ("/* // } */ INT/**/M < .1 // tail", "  INT M < .1 "),
        ("/* done */ //********", "  "),
        (
            r#"INCLUDE "한글//path/*literal*/" // tail"#,
            r#"INCLUDE "한글//path/*literal*/" "#,
        ),
        (
            r#"TEXT "a\" // /*" 'b\' // /*' // real /*"#,
            r#"TEXT "a\" // /*" 'b\' // /*' "#,
        ),
        (r#"TEXT "a\\" // /*"#, r#"TEXT "a\\" "#),
        ("  @ literal // /* }", "  @ literal // /* }"),
        ("/* prefix */ @ literal // /* }", "  @ literal // /* }"),
    ] {
        let mut in_block = false;
        assert_eq!(strip_comments(input, &mut in_block), expected, "{input}");
        assert!(!in_block, "{input}");
    }
    let mut in_block = false;
    assert_eq!(strip_comments("INT M /* opens", &mut in_block), "INT M  ");
    assert!(in_block);
    assert_eq!(strip_comments("// still block }", &mut in_block), "");
    assert!(in_block);
    assert_eq!(
        strip_comments("*/ @ literal /* //", &mut in_block),
        " @ literal /* //"
    );
    assert!(!in_block);
}
#[test]
fn decorative_comments_keep_includes_and_following_rules() {
    let tmp = Temp::new();
    tmp.file("child.cal", "LAYER M 1\nCHILD { INT M < .1 }\n");
    let path = tmp.file(
        "main.cal",
        "//********\nINCLUDE \"child.cal\" //********\nLAYER N 2 // /* ignored\nMAIN { INT N < .2 }\n",
    );
    for scan_all in [false, true] {
        let d = parse_deck(
            &path,
            &Options {
                scan_all,
                env_switches: false,
                ..Options::default()
            },
            &AtomicUsize::new(0),
        )
        .unwrap();
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        assert_eq!(d.stat("files"), 2);
        assert_eq!(d.check_count(), 2);
        assert_eq!(d.checks["CHILD"].constraints[0].value, Some(0.1));
        assert_eq!(d.checks["MAIN"].constraints[0].value, Some(0.2));
    }
}
#[test]
fn directives_keep_quoted_comment_markers() {
    let d = parsed(concat!(
        "#DEFINE PATH \"a//b\" // trailing comment\n",
        "#DEFINE MARK '/* literal */' // trailing comment\n",
        "#DEFINE ESC \"a\\\"//b\" // trailing comment\n",
        "#IFDEF PATH \"a//b\" // matching literal\n",
        "MATCH { INT M < .1 }\n",
        "#ENDIF\n",
    ));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert_eq!(d.defines["PATH"].as_deref(), Some("a//b"));
    assert_eq!(d.defines["MARK"].as_deref(), Some("/* literal */"));
    assert_eq!(d.defines["ESC"].as_deref(), Some("a\\\"//b"));
    assert!(d.checks.contains_key("MATCH"));
}
#[test]
fn block_comments_separate_tokens_and_descriptions_remain_literal() {
    let d = parsed(concat!(
        "/* banner // {\n@ ignored /*\n*/\n",
        "LAYER/**/M/**/7\nR {\n",
        "/* prefix */ @ literal // /* }\n",
        "INT/**/M < .1 /* } */\n}\n",
        "AFTER { INT M < .2 }\n",
    ));
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert_eq!(d.check_count(), 2);
    assert_eq!(d.checks["R"].desc, "literal // /* }");
    assert_eq!(d.checks["R"].source_gds, vec![(7, None)]);
    assert_eq!(d.checks["R"].constraints[0].value, Some(0.1));
    assert_eq!(d.checks["AFTER"].constraints[0].value, Some(0.2));
}
#[test]
fn included_files_have_independent_block_comment_state() {
    let tmp = Temp::new();
    tmp.file(
        "child.cal",
        "LAYER M 1\nCHILD { INT M < .1 }\n/* child never closes\nHIDDEN_CHILD { INT M < 9 }\n",
    );
    let path = tmp.file(
        "parent.cal",
        "INCLUDE \"child.cal\" /* parent banner\nHIDDEN_PARENT { INT M < 9 }\n*/\nPARENT { INT M < .2 }\n",
    );
    let d = parse_deck(&path, &Options::default(), &AtomicUsize::new(0)).unwrap();
    assert_eq!(d.warnings, vec!["unclosed /* comment at end of child.cal"]);
    assert_eq!(d.check_count(), 2);
    assert_eq!(d.checks["CHILD"].constraints[0].value, Some(0.1));
    assert_eq!(d.checks["PARENT"].constraints[0].value, Some(0.2));
}
#[test]
fn commented_braces_preserve_macro_and_verbatim_include_policy() {
    let tmp = Temp::new();
    tmp.file("child.cal", "CHILD { INT M < .1 }\n");
    let path = tmp.file(
        "main.cal",
        concat!(
            "DMACRO SKIP {\n//******** }\nFAKE { INT M < 9 }\n}\n",
            "VERBATIM {\n/* } // */\n//******** }\nINCLUDE child.cal\n}\n",
            "REAL { INT M < .2 }\n",
        ),
    );
    for follow_verbatim in [false, true] {
        let d = parse_deck(
            &path,
            &Options {
                follow_verbatim,
                env_switches: false,
                ..Options::default()
            },
            &AtomicUsize::new(0),
        )
        .unwrap();
        assert!(d.checks.contains_key("REAL"));
        assert!(!d.checks.contains_key("FAKE"));
        assert_eq!(d.checks.contains_key("CHILD"), follow_verbatim);
        assert_eq!(d.verbatim_includes, vec!["child.cal"]);
        assert!(
            !d.warnings.iter().any(|w| w.contains("unclosed")),
            "{:?}",
            d.warnings
        );
    }
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
fn standalone_shape_outputs_resolve_provenance_without_cd_constraints() {
    let d = parsed(concat!(
        "LAYER Base 100\nLAYER MAP 31 DATATYPE 2 100\n",
        "LAYER Aux 200\nLAYER MAP 32 DATATYPE 7 200\n",
        "dGRFILL.C.7_P1_temp1 = Base OR Missing\n",
        "dGRFILL.C.7 { NOT RECTANGLE dGRFILL.C.7_P1_temp1 ORTHOGONAL ONLY }\n",
        "SHAPE { RECTANGLE [Base] >= .1 BY <= .4 ASPECT > 1 ORTHOGONAL ONLY }\n",
        "EXTENTS { RECTANGLE (Aux) MEASURE EXTENTS }\n",
        "COPIED { COPY Aux }\n",
    ));
    let rule = &d.checks["dGRFILL.C.7"];
    assert_eq!(rule.layers, vec!["dGRFILL.C.7_P1_temp1"]);
    assert_eq!(rule.source_gds, vec![(31, Some(2))]);
    assert_eq!(rule.unresolved, vec!["Missing"]);
    assert_eq!(d.checks["SHAPE"].layers, vec!["Base"]);
    assert_eq!(d.checks["SHAPE"].source_gds, vec![(31, Some(2))]);
    for name in ["EXTENTS", "COPIED"] {
        assert_eq!(d.checks[name].layers, vec!["Aux"]);
        assert_eq!(d.checks[name].source_gds, vec![(32, Some(7))]);
    }
    assert!(d.checks.values().all(|r| r.constraints.is_empty()));
    assert_eq!(d.stat("layer_output"), 4);
    assert_eq!(d.stat("unknown_in_block"), 0);
}
#[test]
fn standalone_boolean_outputs_do_not_extend_previous_assignments() {
    let d = parsed(concat!(
        "LAYER a 1\nLAYER b 2\nLAYER c 3\nLAYER d 4\n",
        "R {\n tmp = a OR b\n",
        " NOT RECTANGLE c ORTHOGONAL ONLY\n COPY d\n",
        " AND a c\n OR b d\n XOR a d\n NOT a b\n",
        " a AND b\n b OR c\n c NOT d\n d XOR a\n}\n",
    ));
    assert_eq!(d.derived["tmp"], "a OR b");
    assert_eq!(d.derived_ops["tmp"], vec!["a", "b"]);
    let rule = &d.checks["R"];
    assert_eq!(rule.layers, vec!["a", "b", "c", "d"]);
    assert_eq!(
        rule.source_gds,
        vec![(1, None), (2, None), (3, None), (4, None)]
    );
    assert!(rule.constraints.is_empty());
    assert_eq!(d.stat("layer_output"), 10);
    assert_eq!(d.stat("unknown_in_block"), 0);
}
#[test]
fn standalone_outputs_end_old_measurements_but_keep_existing_wraps() {
    let d = parsed(concat!(
        "LAYER a 1\nLAYER b 2\nLAYER c 3\nLAYER d 4\n",
        "MIX {\n EXT a b < .2\n",
        " NOT RECTANGLE c ORTHOGONAL ONLY\n COPY d\n}\n",
        "WRAP {\n EXT a b\n NOT a\n < .05\n}\n",
        "ASSIGN_WRAP {\n tmp = a AND\n b\n NOT c\n COPY d\n INT tmp < .1\n}\n",
    ));
    let mix = &d.checks["MIX"];
    assert_eq!(mix.constraints.len(), 1);
    assert_eq!(mix.constraints[0].value, Some(0.2));
    assert_eq!(mix.constraints[0].text, "EXT a b < .2");
    assert_eq!(mix.layers, vec!["a", "b", "c", "d"]);
    let wrap = &d.checks["WRAP"];
    assert_eq!(wrap.constraints.len(), 1);
    assert_eq!(wrap.constraints[0].value, Some(0.05));
    assert_eq!(wrap.layers, vec!["a", "b"]);
    assert_eq!(d.derived["tmp"], "a AND b NOT c");
    assert_eq!(d.derived_ops["tmp"], vec!["a", "b", "c"]);
    assert_eq!(d.checks["ASSIGN_WRAP"].constraints[0].value, Some(0.1));
}
#[test]
fn open_assignments_keep_infix_wraps_and_later_layer_provenance() {
    let d = parsed(concat!(
        "LAYER A 1\nLAYER B 2\nLAYER C 3\nLAYER D 4\n",
        "BUILD {\n tmp = A AND\n B OR C\n",
        " closed = A OR B\n C NOT D\n}\n",
        "OPEN_USE { COPY tmp }\nCLOSED_USE { COPY closed }\n",
    ));
    assert_eq!(d.derived["tmp"], "A AND B OR C");
    assert_eq!(d.derived_ops["tmp"], vec!["A", "B", "C"]);
    assert_eq!(
        d.checks["OPEN_USE"].source_gds,
        vec![(1, None), (2, None), (3, None)]
    );
    assert_eq!(d.derived["closed"], "A OR B");
    assert_eq!(d.derived_ops["closed"], vec!["A", "B"]);
    assert_eq!(
        d.checks["CLOSED_USE"].source_gds,
        vec![(1, None), (2, None)]
    );
    assert!(d.checks["OPEN_USE"].unresolved.is_empty());
    assert!(d.checks["CLOSED_USE"].unresolved.is_empty());
}
#[test]
fn standalone_recognizer_keeps_ambiguous_options_and_unknown_text_out() {
    for input in [
        "NOT a",
        "NOT CONNECTED",
        "AND a",
        "OR a CONNECTED",
        "XOR a ASPECT",
        "NOT a NOT_CONNECTED",
        "a OR b OR c",
        "COPY a b",
        "COPY (a)",
        "RECTANGLE a UnknownOption",
        "RECTANGLE ((a))",
        "Mystery a b",
    ] {
        assert_eq!(layer_output_operands(input), None, "{input}");
    }
    let d = parsed(concat!(
        "LAYER Base 1\n",
        "SAFE {\n DFM RDB ignored\n arbitrary Missing\n}\n",
        "UNKNOWN { Unsupported Base Missing }\n",
    ));
    assert!(d.checks.values().all(|r| r.layers.is_empty()));
    assert!(d.checks.values().all(|r| r.constraints.is_empty()));
    assert!(d.unknown.index.contains_key("UNSUPPORTED"));
    assert!(!d.unknown.index.contains_key("ARBITRARY"));
}
#[test]
fn standalone_outputs_keep_check_operand_limits() {
    let tmp = Temp::new();
    let mut text = String::from("R {\n");
    for i in 0..4097 {
        text.push_str(&format!("COPY M{i}\n"));
    }
    text.push_str("}\n");
    let path = tmp.file("bounded.cal", text);
    let error = parse_deck(&path, &Options::default(), &AtomicUsize::new(0))
        .err()
        .expect("4097 standalone inputs must exceed the per-check limit");
    assert!(
        error.message.contains("check operands exceed 4096"),
        "{error}"
    );
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
