use super::*;
use crate::drc::analysis::scope::Selection;
use std::sync::atomic::{AtomicU64, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);
const TAG: &str = "abababababababababababababababab,65";

struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "floe-reasons-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn paths(&self) -> [PathBuf; 2] {
        [
            self.0.join(".x.db.reasons.kim"),
            self.0.join("temp").join(".x.db.reasons.kim-0123456789ab"),
        ]
    }
    fn open(&self, sizes: &[u64]) -> Reasons {
        Reasons::open(self.paths(), TAG.into(), sizes.to_vec(), false)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text_of(r: &Reasons, kind: Kind, check: usize, error: u64) -> Option<&str> {
    r.of(kind, check, error).map(|e| e.text.as_str())
}

#[test]
fn runs_are_said_as_gaps_and_lengths() {
    // a whole rule is two numbers, whatever its size
    assert_eq!(encode(&[(0, 5_000_000)]), "0,5000000");
    assert_eq!(encode(&[(5, 10), (20, 21), (21, 30)]), "5,5,10,1,0,9");
    assert_eq!(decode("0,5000000", 5_000_000), Some(vec![(0, 5_000_000)]));
    // runs that touch are one
    assert_eq!(decode("5,5,10,1,0,9", 30), Some(vec![(5, 10), (20, 30)]));
    for (bad, size) in [
        ("", 10),
        ("3", 10),
        ("3,", 10),
        ("3,0", 10),
        ("3,4,5", 10),
        ("3,8", 10),
        ("0,11", 10),
        ("-1,2", 10),
        ("+1,2", 10),
        (" 1,2", 10),
        ("1,2 ", 10),
        ("a,2", 10),
        ("1,2,x,1", 10),
        ("18446744073709551615,2", u64::MAX),
        ("1,18446744073709551615", u64::MAX),
        ("99999999999999999999,1", u64::MAX),
    ] {
        assert_eq!(decode(bad, size), None, "{bad:?}");
    }
    // what was said is what is read, at any spread
    let scattered: Vec<Run> = (0..5000u64).map(|i| (i * 7 + 3, i * 7 + 4 + i % 5)).collect();
    assert_eq!(decode(&encode(&scattered), 40_000), Some(scattered));
    // a list of errors in any order, with repeats
    assert_eq!(runs_of(&[9, 3, 4, 4, 5, 11, 10, 0]), [(0, 1), (3, 6), (9, 12)]);
    assert_eq!(runs_of(&[]), []);
    // a membership read a range at a time: a run across the ranges is one
    let members = Selection::from_indices(200_000, (65_530..65_540).chain([3, 199_999, 131_071, 131_072]));
    assert_eq!(
        member_runs(&members, 200_000),
        [(3, 4), (65_530, 65_540), (131_071, 131_073), (199_999, 200_000)]
    );
    // ... and only what was handled when a job stopped early
    assert_eq!(member_runs(&members, 65_535), [(3, 4), (65_530, 65_535)]);
    assert_eq!(member_runs(&members, 0), []);
    // runs without others: what a reason is said of when some of an
    // action's errors were not the action's to change
    let runs = [(0u64, 10u64), (20, 30), (40, 41), (50, 60)];
    assert_eq!(minus(&runs, &[]), runs);
    assert_eq!(minus(&[], &[(0, 5)]), []);
    assert_eq!(
        minus(&runs, &[(0, 1), (3, 5), (9, 22), (29, 30), (40, 41), (45, 47), (55, 70), (80, 90)]),
        [(1, 3), (5, 9), (22, 29), (50, 55)]
    );
    assert_eq!(minus(&runs, &[(0, 100)]), []);
    assert_eq!(minus(&runs, &[(10, 20), (30, 40), (41, 50)]), runs, "what lies between");
    assert_eq!(minus(&[(5, 8)], &[(0, 6), (7, 9)]), [(6, 7)]);
}

#[test]
fn a_reason_is_a_line_trimmed_and_bounded() {
    assert_eq!(clean("  dummy fill\t(approved)\n ").unwrap(), "dummy fill (approved)");
    assert_eq!(clean("줄 하나\r\n둘").unwrap(), "줄 하나  둘");
    assert_eq!(clean(" \n ").unwrap(), "");
    assert_eq!(clean(&"가".repeat(TEXT_LIMIT)).unwrap().chars().count(), TEXT_LIMIT);
    let e = clean(&"x".repeat(TEXT_LIMIT + 1)).unwrap_err();
    assert!(e.message.contains("at most 1000 characters"), "{}", e.message);
    assert_eq!(Kind::parse("waive"), Some(Kind::Waive));
    assert_eq!(Kind::parse("not-required"), Some(Kind::NotRequired));
    assert_eq!(Kind::parse("not_required"), Some(Kind::NotRequired));
    assert_eq!(Kind::parse("note"), None);
    assert_eq!((Kind::Waive.name(), Kind::NotRequired.name()), ("waive", "not_required"));
}

#[test]
fn an_action_holds_its_errors_until_another_takes_them() {
    let f = Folder::new();
    let mut r = f.open(&[0, 100, 40]);
    assert!(r.is_empty() && !f.paths()[0].exists(), "nothing is made by the open");
    r.set(Kind::Waive, 1, &[(0, 100)], "dummy fill", 10).unwrap();
    r.set(Kind::Waive, 1, &[(10, 20), (30, 31)], "seal ring", 20).unwrap();
    r.set(Kind::NotRequired, 1, &[(15, 18)], "out of scope", 30).unwrap();
    r.set(Kind::Waive, 2, &[(39, 40)], "dummy fill", 40).unwrap();
    for (error, want) in [
        (0, "dummy fill"),
        (9, "dummy fill"),
        (10, "seal ring"),
        (19, "seal ring"),
        (20, "dummy fill"),
        (30, "seal ring"),
        (31, "dummy fill"),
        (99, "dummy fill"),
    ] {
        assert_eq!(text_of(&r, Kind::Waive, 1, error), Some(want), "{error}");
    }
    // the two kinds are apart, and so are the rules
    assert_eq!(text_of(&r, Kind::NotRequired, 1, 16), Some("out of scope"));
    assert_eq!(text_of(&r, Kind::NotRequired, 1, 18), None);
    assert_eq!(text_of(&r, Kind::Waive, 2, 39), Some("dummy fill"));
    assert_eq!(text_of(&r, Kind::Waive, 2, 38), None);
    assert_eq!(text_of(&r, Kind::Waive, 0, 0), None);
    assert_eq!(text_of(&r, Kind::Waive, 9, 0), None);
    assert_eq!(text_of(&r, Kind::Waive, 1, 100), None);
    assert_eq!(r.of(Kind::Waive, 1, 10).unwrap().time, 20);
    // one reason over all of them, or none to start from
    assert_eq!(r.common(Kind::Waive, 1, &[(0, 10)]), Some("dummy fill"));
    assert_eq!(r.common(Kind::Waive, 1, &[(0, 10), (20, 30), (31, 100)]), Some("dummy fill"));
    assert_eq!(r.common(Kind::Waive, 1, &[(5, 15)]), None, "two reasons");
    assert_eq!(r.common(Kind::Waive, 1, &[(10, 20), (30, 31)]), Some("seal ring"));
    assert_eq!(r.common(Kind::Waive, 2, &[(38, 40)]), None, "an error without one");
    assert_eq!(r.common(Kind::Waive, 2, &[(39, 40)]), Some("dummy fill"));
    assert_eq!(r.common(Kind::Waive, 0, &[]), None);
    // what of a set of errors holds a reason
    assert_eq!(r.held(Kind::NotRequired, 1, &[(0, 100)]), [(15, 18)]);
    assert_eq!(r.held(Kind::Waive, 1, &[(5, 12), (40, 50)]), [(5, 12), (40, 50)]);
    assert_eq!(r.held(Kind::Waive, 2, &[(0, 39)]), []);
    assert_eq!(r.held(Kind::Waive, 0, &[(0, 1)]), []);
    // the reasons given, the latest first, each once
    assert_eq!(r.texts(Kind::Waive, 20), ["dummy fill", "seal ring"]);
    assert_eq!(r.texts(Kind::Waive, 1), ["dummy fill"]);
    assert_eq!(r.texts(Kind::NotRequired, 20), ["out of scope"]);
    // turned back: only what was held is said
    assert!(r.clear(Kind::Waive, 1, &[(5, 12), (31, 32)]));
    assert!(!r.clear(Kind::Waive, 1, &[(5, 12)]), "held nothing any more");
    assert!(!r.clear(Kind::Waive, 0, &[(0, 1)]));
    assert!(!r.clear(Kind::NotRequired, 2, &[(0, 40)]));
    for (error, want) in [
        (4, Some("dummy fill")),
        (5, None),
        (11, None),
        (12, Some("seal ring")),
        (30, Some("seal ring")),
        (31, None),
        (32, Some("dummy fill")),
    ] {
        assert_eq!(text_of(&r, Kind::Waive, 1, error), want, "{error}");
    }
    assert!(r.any(Kind::Waive, 1) && r.any(Kind::Waive, 2) && !r.any(Kind::Waive, 0));
    assert_eq!(r.checks(Kind::Waive), [1, 2]);
    assert_eq!(r.checks(Kind::NotRequired), [1]);
    assert!(r.clear(Kind::NotRequired, 1, &[(0, 100)]));
    assert!(!r.any(Kind::NotRequired, 1) && r.checks(Kind::NotRequired).is_empty());
    // the file: the header, then a line an action - and a line taken back
    // names only what was held
    r.flush(54).unwrap();
    let text = fs::read_to_string(&f.paths()[0]).unwrap();
    assert_eq!(
        text,
        format!(
            "# floe drc review reasons\nfloe_reasons=1\nfloe_pack={TAG}\n\
             w 1 10 0,100 dummy fill\n\
             w 1 20 10,10,10,1 seal ring\n\
             n 1 30 15,3 out of scope\n\
             w 2 40 39,1 dummy fill\n\
             W 1 54 5,7,19,1\n\
             N 1 54 15,3\n"
        )
    );
    // read again it says the same
    let again = f.open(&[0, 100, 40]);
    assert!(again.notices.is_empty(), "{:?}", again.notices);
    for error in 0..100 {
        assert_eq!(
            text_of(&again, Kind::Waive, 1, error),
            text_of(&r, Kind::Waive, 1, error),
            "{error}"
        );
        assert_eq!(text_of(&again, Kind::NotRequired, 1, error), None);
    }
    assert_eq!(
        again
            .runs(Kind::Waive, 1)
            .iter()
            .map(|&(a, b, e)| (a, b, again.entries()[e].text.as_str()))
            .collect::<Vec<_>>(),
        [
            (0, 5, "dummy fill"),
            (12, 20, "seal ring"),
            (20, 30, "dummy fill"),
            (30, 31, "seal ring"),
            (32, 100, "dummy fill")
        ]
    );
    // what is refused, before anything is written
    let len = fs::metadata(&f.paths()[0]).unwrap().len();
    for runs in [&[(0u64, 101u64)][..], &[(5, 5)], &[(9, 8)], &[(0, 4), (4, 6)], &[(6, 9), (0, 2)]] {
        assert!(r.set(Kind::Waive, 1, runs, "x", 60).is_err(), "{runs:?}");
    }
    assert!(r.set(Kind::Waive, 3, &[(0, 1)], "x", 60).is_err(), "no such rule");
    assert!(r.set(Kind::Waive, 1, &[(0, 1)], &"x".repeat(TEXT_LIMIT + 1), 60).is_err());
    // nothing to say is nothing written
    r.set(Kind::Waive, 1, &[(0, 1)], "  ", 60).unwrap();
    r.set(Kind::Waive, 1, &[], "x", 60).unwrap();
    assert_eq!(fs::metadata(&f.paths()[0]).unwrap().len(), len);
    assert_eq!(text_of(&r, Kind::Waive, 1, 0), Some("dummy fill"));
}

/// The runs against a table an error long: the same answers after any
/// sequence of actions, and after the file is read back.
#[test]
fn the_runs_say_what_an_error_long_table_says() {
    const N: u64 = 700;
    let f = Folder::new();
    let mut r = f.open(&[N, 50]);
    // (a kind's own table: the rejections' too, 0.12.468)
    // (a kind's slot: the five kinds an error holds a reason of)
    let mut table: [Vec<Option<String>>; 9] = std::array::from_fn(|_| vec![None; N as usize]);
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = |below: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % below
    };
    for step in 0..1500u64 {
        let kind = Kind::HELD[next(5) as usize];
        // a few runs, from single errors to wide ones
        let mut errors = Vec::new();
        for _ in 0..1 + next(6) {
            let start = next(N);
            let len = match next(4) {
                0 => 1,
                1 => 1 + next(4),
                2 => 1 + next(40),
                _ => 1 + next(300),
            };
            errors.extend(start..(start + len).min(N));
        }
        let runs = runs_of(&errors);
        if next(3) == 0 {
            let had = runs
                .iter()
                .any(|&(a, b)| (a..b).any(|e| table[kind.slot()][e as usize].is_some()));
            assert_eq!(r.clear(kind, 0, &runs), had, "step {step}");
            for &(a, b) in &runs {
                table[kind.slot()][a as usize..b as usize].fill(None);
            }
        } else {
            let text = format!("reason {}", next(12));
            r.set(kind, 0, &runs, &text, step).unwrap();
            for &(a, b) in &runs {
                table[kind.slot()][a as usize..b as usize].fill(Some(text.clone()));
            }
        }
        if step % 97 == 0 || step == 1499 {
            for kind in Kind::HELD {
                for e in 0..N {
                    assert_eq!(
                        text_of(&r, kind, 0, e),
                        table[kind.slot()][e as usize].as_deref(),
                        "step {step} error {e}"
                    );
                }
                // the runs are in order, apart, and never empty
                let runs = r.runs(kind, 0);
                assert!(runs.iter().all(|&(a, b, _)| a < b && b <= N));
                assert!(runs.windows(2).all(|w| w[0].1 <= w[1].0));
                // `held` and `common` against the table
                let a = next(N / 2);
                let probe = [(a, N / 2 + 1 + next(N / 2 - 1))];
                let slice = &table[kind.slot()][probe[0].0 as usize..probe[0].1 as usize];
                let held: u64 = r.held(kind, 0, &probe).iter().map(|&(a, b)| b - a).sum();
                assert_eq!(held, slice.iter().filter(|s| s.is_some()).count() as u64);
                let one = slice[0]
                    .as_deref()
                    .filter(|t| slice.iter().all(|s| s.as_deref() == Some(*t)));
                assert_eq!(r.common(kind, 0, &probe), one);
            }
        }
    }
    // the log read back, and then the file written again from what is held
    r.flush(9999).unwrap();
    for pass in 0..2 {
        let again = f.open(&[N, 50]);
        assert!(again.notices.is_empty(), "{:?}", again.notices);
        for kind in Kind::HELD {
            for e in 0..N {
                assert_eq!(
                    text_of(&again, kind, 0, e),
                    table[kind.slot()][e as usize].as_deref(),
                    "pass {pass} error {e}"
                );
            }
        }
        // what the open's rewrite would write
        fs::write(&f.paths()[0], again.serialize()).unwrap();
    }
    // the file written again holds a line for each action that still holds
    // errors and no line that takes anything back
    let text = fs::read_to_string(&f.paths()[0]).unwrap();
    let live = f.open(&[N, 50]).live().len();
    assert!(live > 0);
    assert_eq!(text.lines().count(), 3 + live);
    assert!(!text.lines().any(|l| l.starts_with("W ") || l.starts_with("N ")));
}

/// A job turns statuses back a chunk at a time and drops their reasons as
/// it goes: gone from memory at once, said in the file when the job ends -
/// one line a rule, in whatever order and number of pieces they went.
#[test]
fn what_a_job_takes_back_is_one_line_a_rule() {
    let f = Folder::new();
    let path = f.paths()[0].clone();
    let mut r = f.open(&[100, 100]);
    r.set(Kind::Waive, 0, &[(0, 100)], "a", 1).unwrap();
    r.set(Kind::Waive, 1, &[(0, 100)], "b", 2).unwrap();
    r.set(Kind::NotRequired, 1, &[(40, 50)], "c", 3).unwrap();
    let before = fs::read_to_string(&path).unwrap();
    for runs in [&[(60u64, 70u64)][..], &[(10, 20)], &[(20, 30)], &[(65, 80), (99, 100)]] {
        assert!(r.clear(Kind::Waive, 0, runs));
    }
    assert!(r.clear(Kind::Waive, 1, &[(5, 6)]));
    assert!(r.clear(Kind::NotRequired, 1, &[(0, 100)]));
    // gone at once, not said yet
    assert_eq!(text_of(&r, Kind::Waive, 0, 15), None);
    assert_eq!(text_of(&r, Kind::Waive, 0, 79), None);
    assert_eq!(text_of(&r, Kind::Waive, 0, 80), Some("a"));
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    r.flush(9).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("{before}W 0 9 10,20,30,20,19,1\nW 1 9 5,1\nN 1 9 40,10\n")
    );
    // nothing more to say
    r.flush(10).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 9);
    // a reason given after a drop is written after it: the drop said later
    // would take the new reason back when the file is read
    assert!(r.clear(Kind::Waive, 0, &[(0, 5)]));
    r.set(Kind::Waive, 0, &[(0, 3)], "d", 11).unwrap();
    assert!(fs::read_to_string(&path).unwrap().ends_with("W 0 11 0,5\nw 0 11 0,3 d\n"));
    drop(r);
    let again = f.open(&[100, 100]);
    for (error, want) in [(0, Some("d")), (2, Some("d")), (3, None), (4, None), (5, Some("a")), (15, None), (99, None)] {
        assert_eq!(text_of(&again, Kind::Waive, 0, error), want, "{error}");
    }
    assert_eq!(text_of(&again, Kind::Waive, 1, 5), None);
    assert_eq!(text_of(&again, Kind::Waive, 1, 6), Some("b"));
    assert_eq!(text_of(&again, Kind::NotRequired, 1, 45), None);
}

/// User 2026-11-13: millions of errors waived at once were a note an error.
/// A reason is a line: its size is the runs', not the errors'.
#[test]
fn a_reason_of_millions_of_errors_is_one_short_line() {
    let f = Folder::new();
    const N: u64 = 200_000_000;
    let mut r = f.open(&[N, N]);
    // every error of a rule
    r.set(Kind::Waive, 0, &[(0, N)], "dummy metal: no electrical function", 1).unwrap();
    let len = fs::metadata(&f.paths()[0]).unwrap().len();
    assert!(len < 160, "{len} bytes for 200M errors");
    assert_eq!(text_of(&r, Kind::Waive, 0, N - 1), Some("dummy metal: no electrical function"));
    assert_eq!(r.runs(Kind::Waive, 0).len(), 1);
    // a million errors scattered over the rule: a few bytes a run
    let scattered: Vec<Run> = (0..1_000_000u64).map(|i| (i * 150 + 7, i * 150 + 8 + i % 3)).collect();
    r.set(Kind::Waive, 1, &scattered, "scattered", 2).unwrap();
    let grown = fs::metadata(&f.paths()[0]).unwrap().len() - len;
    assert!(grown < 6_500_000, "{grown} bytes for a million runs");
    assert_eq!(text_of(&r, Kind::Waive, 1, 999_999 * 150 + 7), Some("scattered"));
    assert_eq!(text_of(&r, Kind::Waive, 1, 999_999 * 150 + 6), None);
    // one error taken out of the middle of the whole rule: two runs
    assert!(r.clear(Kind::Waive, 0, &[(77_000_000, 77_000_001)]));
    assert_eq!(r.runs(Kind::Waive, 0).len(), 2);
    assert_eq!(text_of(&r, Kind::Waive, 0, 77_000_000), None);
    assert_eq!(text_of(&r, Kind::Waive, 0, 77_000_001), Some("dummy metal: no electrical function"));
    // read back
    r.flush(3).unwrap();
    let again = f.open(&[N, N]);
    assert_eq!(again.runs(Kind::Waive, 1).len(), 1_000_000);
    assert_eq!(text_of(&again, Kind::Waive, 0, 5), Some("dummy metal: no electrical function"));
    assert_eq!(text_of(&again, Kind::Waive, 0, 77_000_000), None);
}

#[test]
fn a_torn_or_strange_line_is_passed_over() {
    let f = Folder::new();
    let path = f.paths()[0].clone();
    let mut r = f.open(&[100]);
    r.set(Kind::Waive, 0, &[(0, 10)], "first", 1).unwrap();
    r.set(Kind::Waive, 0, &[(20, 30)], "back\\slash and\nbreak", 2).unwrap();
    assert_eq!(text_of(&r, Kind::Waive, 0, 20), Some("back\\slash and break"));
    drop(r);
    let whole = fs::read(&path).unwrap();
    assert!(String::from_utf8_lossy(&whole).ends_with("w 0 2 20,10 back\\\\slash and break\n"));
    // a run that ended inside a line: no line break
    let mut torn = whole.clone();
    torn.extend_from_slice(b"w 0 3 40,5 half a li");
    fs::write(&path, &torn).unwrap();
    let mut r = f.open(&[100]);
    assert!(r.notices.is_empty(), "{:?}", r.notices);
    assert_eq!(text_of(&r, Kind::Waive, 0, 5), Some("first"));
    assert_eq!(text_of(&r, Kind::Waive, 0, 25), Some("back\\slash and break"));
    assert_eq!(text_of(&r, Kind::Waive, 0, 42), None, "a torn line says nothing");
    assert_eq!(fs::read(&path).unwrap(), torn, "reading cuts nothing");
    // the next line is written where the whole lines end
    r.set(Kind::Waive, 0, &[(50, 60)], "after", 4).unwrap();
    drop(r);
    let mut want = whole.clone();
    want.extend_from_slice(b"w 0 4 50,10 after\n");
    assert_eq!(fs::read(&path).unwrap(), want);
    // lines that are none: each passed over and counted, the others read
    let strange = [
        "x 0 5 0,1 unknown first word",
        "w 9 5 0,1 no such rule",
        "w 0 5 0,101 past the rule",
        "w 0 5 0,0 an empty run",
        "w 0 five 0,1 no time",
        "w 0 5 0,1",
        "w 0 5 0,1  ",
        "w 0 5",
        "W 0 5 x",
        "garbage",
    ];
    let mut text = String::from_utf8(want.clone()).unwrap();
    for line in strange {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("\n# a comment\nw 0 6 90,10 last\r\n");
    fs::write(&path, &text).unwrap();
    let r = f.open(&[100]);
    assert_eq!(r.notices.len(), 1, "{:?}", r.notices);
    assert!(
        r.notices[0].contains(&format!("{} line(s)", strange.len())),
        "{:?}",
        r.notices
    );
    assert_eq!(text_of(&r, Kind::Waive, 0, 95), Some("last"));
    assert_eq!(text_of(&r, Kind::Waive, 0, 55), Some("after"));
    assert_eq!(text_of(&r, Kind::Waive, 0, 0), Some("first"));
    drop(r);
    // a file that is empty, or torn inside its header, starts again
    for start in [&b""[..], b"# floe drc review reasons\nfloe_reas"] {
        fs::write(&path, start).unwrap();
        let mut r = f.open(&[100]);
        assert!(r.is_empty());
        r.set(Kind::NotRequired, 0, &[(1, 2)], "again", 7).unwrap();
        drop(r);
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.ends_with(&format!("floe_reasons=1\nfloe_pack={TAG}\nn 0 7 1,1 again\n")),
            "{text:?}"
        );
        assert_eq!(text_of(&f.open(&[100]), Kind::NotRequired, 0, 1), Some("again"));
    }
}

#[test]
fn a_file_of_another_result_is_moved_aside_and_never_read() {
    let f = Folder::new();
    let path = f.paths()[0].clone();
    let others = [
        (
            "another result",
            "# floe drc review reasons\nfloe_reasons=1\nfloe_pack=cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd,65\nw 0 1 0,5 theirs\n".to_string(),
        ),
        (
            "another error count",
            "floe_reasons=1\nfloe_pack=abababababababababababababababab,66\nw 0 1 0,5 theirs\n".to_string(),
        ),
        ("another version", format!("floe_reasons=9\nfloe_pack={TAG}\nw 0 1 0,5 theirs\n")),
        ("no version", format!("floe_pack={TAG}\nw 0 1 0,5 theirs\n")),
        ("no result", "floe_reasons=1\nw 0 1 0,5 theirs\n".to_string()),
        ("something else", "floe_note=0|a notes file\n".to_string()),
    ];
    for (what, text) in &others {
        fs::write(&path, text).unwrap();
        // the statistics refuse it and leave it
        let e = Reasons::load(&path, TAG, &[100])
            .err()
            .unwrap_or_else(|| panic!("{what}: read"));
        assert!(
            e.kind == ErrorKind::Cache && e.message.contains("not this pack's"),
            "{what}: {}",
            e.message
        );
        assert_eq!(&fs::read_to_string(&path).unwrap(), text, "{what}");
        // the review moves it aside and starts its own
        let mut r = f.open(&[100]);
        assert!(r.is_empty(), "{what}");
        assert_eq!(r.notices.len(), 1, "{what}: {:?}", r.notices);
        let said = r.notices[0].clone();
        assert!(
            said.contains("review reasons do not match this pack") && said.contains("moved aside: "),
            "{said}"
        );
        let aside = PathBuf::from(said.rsplit_once("moved aside: ").unwrap().1);
        assert_eq!(&fs::read_to_string(&aside).unwrap(), text, "{what}: kept as it was");
        assert!(
            aside.to_string_lossy().starts_with(&format!("{}.stale-", path.display())),
            "{said}"
        );
        assert!(!path.exists(), "{what}");
        r.set(Kind::Waive, 0, &[(0, 1)], "mine", 9).unwrap();
        drop(r);
        assert_eq!(text_of(&f.open(&[100]), Kind::Waive, 0, 0), Some("mine"));
        fs::remove_file(&aside).unwrap();
        fs::remove_file(&path).unwrap();
    }
    // no file: nothing for the statistics, nothing made
    assert!(Reasons::load(&path, TAG, &[100]).unwrap().is_none());
    assert!(!path.exists());
    // this result's: read, and left byte for byte
    let mine = format!(
        "# floe drc review reasons\nfloe_reasons=1\nfloe_pack={TAG}\nw 0 1 0,5 mine\nW 0 2 0,2\nw 0 3 9,1 torn"
    );
    fs::write(&path, &mine).unwrap();
    let r = Reasons::load(&path, TAG, &[100]).unwrap().unwrap();
    assert_eq!(text_of(&r, Kind::Waive, 0, 4), Some("mine"));
    assert_eq!(text_of(&r, Kind::Waive, 0, 1), None);
    assert_eq!(text_of(&r, Kind::Waive, 0, 9), None);
    assert_eq!(fs::read_to_string(&path).unwrap(), mine);
}

#[test]
fn a_file_grown_past_what_it_says_is_written_again() {
    let f = Folder::new();
    let path = f.paths()[0].clone();
    let mut r = f.open(&[1000]);
    r.set(Kind::Waive, 0, &[(900, 1000)], "kept", 1).unwrap();
    // the same errors waived and turned back again and again
    let long = "y".repeat(200);
    for i in 0..400u64 {
        r.set(Kind::Waive, 0, &[(0, 500)], &format!("{long} {i}"), 2 + i).unwrap();
        assert!(r.clear(Kind::Waive, 0, &[(0, 500)]));
    }
    r.set(Kind::Waive, 0, &[(10, 20)], "kept too", 999).unwrap();
    drop(r);
    let grown = fs::metadata(&path).unwrap().len();
    assert!(grown > SLACK + 1000, "{grown}");
    // the open writes what is held, in the actions' order
    let mut r = f.open(&[1000]);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!(
            "# floe drc review reasons\nfloe_reasons=1\nfloe_pack={TAG}\nw 0 1 900,100 kept\nw 0 999 10,10 kept too\n"
        )
    );
    assert_eq!(text_of(&r, Kind::Waive, 0, 950), Some("kept"));
    assert_eq!(text_of(&r, Kind::Waive, 0, 15), Some("kept too"));
    assert_eq!(text_of(&r, Kind::Waive, 0, 100), None);
    // and goes on from there
    r.set(Kind::Waive, 0, &[(0, 1)], "next", 1000).unwrap();
    drop(r);
    assert!(fs::read_to_string(&path)
        .unwrap()
        .ends_with("w 0 999 10,10 kept too\nw 0 1000 0,1 next\n"));
    // a small file is left as its lines were written, taken back lines too
    let mut r = f.open(&[1000]);
    assert!(r.clear(Kind::Waive, 0, &[(0, 1)]));
    r.flush(1001).unwrap();
    drop(r);
    let before = fs::read_to_string(&path).unwrap();
    assert!(before.ends_with("w 0 1000 0,1 next\nW 0 1001 0,1\n"));
    drop(f.open(&[1000]));
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    // every reason gone: no file
    let mut r = f.open(&[1000]);
    assert!(r.clear(Kind::Waive, 0, &[(0, 1000)]));
    r.flush(1002).unwrap();
    drop(r);
    assert!(path.exists());
    let r = f.open(&[1000]);
    assert!(r.is_empty() && !path.exists() && r.notices.is_empty());
    // the statistics tidy nothing
    fs::write(&path, &before).unwrap();
    assert!(Reasons::load(&path, TAG, &[1000]).unwrap().is_some());
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn a_folder_that_refuses_sends_the_reasons_to_the_other_place() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no arguments and no failure.
    if unsafe { libc::geteuid() } == 0 {
        return; // root writes anywhere
    }
    let f = Folder::new();
    let [beside, temp] = f.paths();
    fs::create_dir(temp.parent().unwrap()).unwrap();
    // reasons kept beside the results while they could be written there
    let mut r = f.open(&[100]);
    r.set(Kind::Waive, 0, &[(0, 10)], "beside", 1).unwrap();
    drop(r);
    fs::set_permissions(&beside, fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(&f.0, fs::Permissions::from_mode(0o555)).unwrap();
    let mut r = f.open(&[100]);
    assert_eq!(text_of(&r, Kind::Waive, 0, 5), Some("beside"));
    r.set(Kind::Waive, 0, &[(20, 30)], "elsewhere", 2).unwrap();
    assert_eq!(r.path, temp);
    assert_eq!(r.notices.len(), 1, "{:?}", r.notices);
    assert!(
        r.notices[0].contains("results folder not writable; review reasons in "),
        "{:?}",
        r.notices
    );
    // everything held went along, and the next line follows it there
    r.set(Kind::NotRequired, 0, &[(1, 2)], "third", 3).unwrap();
    assert_eq!(r.notices.len(), 1);
    drop(r);
    assert_eq!(
        fs::read_to_string(&temp).unwrap(),
        format!(
            "# floe drc review reasons\nfloe_reasons=1\nfloe_pack={TAG}\nw 0 1 0,10 beside\nw 0 2 20,10 elsewhere\nn 0 3 1,1 third\n"
        )
    );
    // nowhere to go: the error, and nothing held that is not written
    fs::set_permissions(temp.parent().unwrap(), fs::Permissions::from_mode(0o555)).unwrap();
    fs::set_permissions(&temp, fs::Permissions::from_mode(0o444)).unwrap();
    let mut r = Reasons::open([temp.clone(), temp.clone()], TAG.into(), vec![100], false);
    assert_eq!(text_of(&r, Kind::Waive, 0, 25), Some("elsewhere"));
    assert!(r.set(Kind::Waive, 0, &[(50, 60)], "lost", 4).is_err());
    assert_eq!(text_of(&r, Kind::Waive, 0, 55), None);
    fs::set_permissions(temp.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&f.0, fs::Permissions::from_mode(0o755)).unwrap();
}

/// The rejects log keeps what its reviewer answered (0.12.469, user
/// 2026-11-14: "이전의 waive사유나 반려사유는 계속 유지되어야"): a kept
/// record takes nothing from an earlier one, is read back in its place,
/// is written again with the rest - and names the rejection it answers by
/// who made it and when.
#[test]
fn what_a_reviewer_answered_is_kept_as_it_was() {
    let f = Folder::new();
    let log = f.0.join(".x.db.rejects.kim");
    let open = || {
        Reasons::open_as(
            "# floe drc waive rejections",
            REJECTS_VERSION,
            [log.clone(), f.0.join("temp").join("unused")],
            TAG.into(),
            vec![50, 15],
            false,
        )
    };
    let mut r = open();
    // a kept record is kept, not set; an answered rejection has who made
    // it, an earlier reason has none; nothing is nothing
    assert!(r.set(Kind::Answered, 0, &[(0, 5)], "x", 1).is_err());
    assert!(r.keep(Kind::Reject, 0, &[(0, 5)], 1, "", "x").is_err());
    assert!(r.keep(Kind::Answered, 0, &[(0, 5)], 1, "", "x").is_err());
    assert!(r.keep(Kind::Earlier, 0, &[(0, 5)], 1, "hong", "x").is_err());
    assert!(r.keep(Kind::Answered, 0, &[(0, 5)], 1, "a b", "x").is_err());
    assert!(r.keep(Kind::Answered, 0, &[(40, 60)], 1, "hong", "x").is_err());
    r.keep(Kind::Earlier, 0, &[(0, 5)], 1, "", "  ").unwrap();
    r.keep(Kind::Answered, 0, &[], 1, "hong", "x").unwrap();
    assert!(r.is_empty() && !log.exists());
    // the reason the waive had, then the rejection that was answered
    r.keep(Kind::Earlier, 0, &[(0, 5)], 100, "", "dummy fill").unwrap();
    r.keep(Kind::Answered, 0, &[(0, 3), (4, 5)], 200, "hong", "not a dummy").unwrap();
    r.keep(Kind::Answered, 0, &[(2, 3)], 300, "park.2", "").unwrap();
    // ... and again, of some of the same errors: nothing is taken from
    // what was kept before
    r.keep(Kind::Earlier, 0, &[(2, 4)], 400, "", "dummy fill \\ rule note").unwrap();
    r.keep(Kind::Answered, 0, &[(2, 4)], 500, "hong", "still not").unwrap();
    r.set(Kind::Reject, 1, &[(0, 3)], "mine", 600).unwrap();
    let said = |e: &Entry| (e.kind, e.time, e.by.clone(), e.text.clone());
    let at = |r: &Reasons, error: u64| r.kept_at(0, error).into_iter().map(said).collect::<Vec<_>>();
    let a = |time: u64, by: &str, text: &str| (Kind::Answered, time, by.to_owned(), text.to_owned());
    let p = |time: u64, text: &str| (Kind::Earlier, time, String::new(), text.to_owned());
    let check = |r: &Reasons| {
        assert_eq!(at(r, 0), [p(100, "dummy fill"), a(200, "hong", "not a dummy")]);
        assert_eq!(
            at(r, 2),
            [
                p(100, "dummy fill"),
                a(200, "hong", "not a dummy"),
                a(300, "park.2", ""),
                p(400, "dummy fill \\ rule note"),
                a(500, "hong", "still not"),
            ]
        );
        assert_eq!(at(r, 3), [p(100, "dummy fill"), p(400, "dummy fill \\ rule note"), a(500, "hong", "still not")]);
        assert!(at(r, 5).is_empty() && r.kept_at(1, 0).is_empty());
        assert_eq!(r.kept_of(Kind::Answered, 0, 4).len(), 1);
        // which rejections are answered, of which errors
        assert_eq!(r.answered(Kind::Answered, 0, "hong", 200), [(0, 3), (4, 5)]);
        assert_eq!(r.answered(Kind::Answered, 0, "hong", 500), [(2, 4)]);
        assert_eq!(r.answered(Kind::Answered, 0, "park.2", 300), [(2, 3)]);
        assert!(r.answered(Kind::Answered, 0, "hong", 300).is_empty() && r.answered(Kind::Answered, 0, "park", 300).is_empty() && r.answered(Kind::Answered, 1, "hong", 200).is_empty());
        assert_eq!((r.kept_checks(Kind::Answered), r.kept_checks(Kind::Earlier)), (vec![0], vec![0]));
        assert_eq!((r.latest(Kind::Reject), r.latest(Kind::Answered), r.latest(Kind::Waive)), (600, 500, 0));
        // (a kept record is no reason an error holds)
        assert!(r.of(Kind::Answered, 0, 2).is_none() && !r.any(Kind::Earlier, 0) && r.runs(Kind::Answered, 0).is_empty());
        assert_eq!(r.runs(Kind::Reject, 1).len(), 1);
    };
    check(&r);
    r.flush(700).unwrap();
    let text = fs::read_to_string(&log).unwrap();
    assert_eq!(
        text,
        format!(
            "# floe drc waive rejections\nfloe_reasons=2\nfloe_pack={TAG}\n\
             p 0 100 0,5 dummy fill\n\
             j 0 200 0,3,1,1 hong not a dummy\n\
             j 0 300 2,1 park.2\n\
             p 0 400 2,2 dummy fill \\\\ rule note\n\
             j 0 500 2,2 hong still not\n\
             r 1 600 0,3 mine\n"
        )
    );
    // read back, and written again whole: the same lines
    drop(r);
    let r = open();
    assert!(r.notices.is_empty(), "{:?}", r.notices);
    check(&r);
    assert_eq!(r.serialize(), text);
    check(&Reasons::load(&log, TAG, &[50, 15]).unwrap().unwrap());
    // a log that holds nothing but what is kept is no empty log: it stays
    drop(r);
    let mut r = open();
    r.clear(Kind::Reject, 1, &[(0, 3)]);
    r.flush(800).unwrap();
    drop(r);
    let r = open();
    assert!(log.exists() && !r.is_empty() && r.runs(Kind::Reject, 1).is_empty());
    assert_eq!(at(&r, 0).len(), 2);
    drop(r);
    // lines that are none of these are passed over and said
    fs::write(&log, format!("# floe drc waive rejections\nfloe_reasons=2\nfloe_pack={TAG}\nj 0 200 0,3\nj 0 200 0,3 a/b why\nJ 0 300 0,3\np 0 100 0,3\nj 0 250 1,1 lee\n")).unwrap();
    let r = open();
    assert_eq!(r.notices.len(), 1, "{:?}", r.notices);
    assert!(r.notices[0].contains("4 line(s)"), "{:?}", r.notices);
    assert_eq!(at(&r, 1), [a(250, "lee", "")]);
    drop(r);
    // a log of the version before (0.12.468: rejections alone) is read, and
    // is this version's from the open of its reviewer's session on - a
    // build that reads the earlier one alone passes it over from then
    fs::write(&log, format!("# floe drc waive rejections\nfloe_reasons=1\nfloe_pack={TAG}\nr 0 100 0,5 earlier\nR 0 110 1,1\n")).unwrap();
    let peer = Reasons::load(&log, TAG, &[50, 15]).unwrap().unwrap();
    assert_eq!(peer.runs(Kind::Reject, 0).iter().map(|r| (r.0, r.1)).collect::<Vec<_>>(), [(0, 1), (2, 5)]);
    assert!(fs::read_to_string(&log).unwrap().contains("floe_reasons=1"), "a reader writes nothing");
    let r = open();
    assert!(r.notices.is_empty(), "{:?}", r.notices);
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        format!("# floe drc waive rejections\nfloe_reasons=2\nfloe_pack={TAG}\nr 0 100 0,1,1,3 earlier\n")
    );
    drop(r);
    // the reasons file is of its own version: one of the rejects log's is
    // not its reviewer's to open as that
    let reasons = f.paths()[0].clone();
    fs::write(&reasons, format!("# floe drc review reasons\nfloe_reasons=2\nfloe_pack={TAG}\nw 0 1 0,5 theirs\n")).unwrap();
    let r = f.open(&[50, 15]);
    assert!(r.is_empty() && !reasons.exists() && r.notices.len() == 1, "{:?}", r.notices);
}
