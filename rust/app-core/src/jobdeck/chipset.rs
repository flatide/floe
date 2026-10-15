//! Which chip set a source belongs to (user 2026-10-15: "jobdeck 파일에서 oas
//! 파일이 어느 chipset에 속해있는지 표시하는 기능이 필요함. cli로만 동작하면
//! 됨."): `floe2 jobdeck DECK.jb --chipset NAME[,NAME...]`.
//!
//! A chip set is a CHIP block of the deck: one placement unit whose `$`
//! entries - the patterns of its mask levels, each a TC source - stack at
//! every position its ROWS lists (docs/JOBDECK.ko.md §1). The CHIP line's
//! text after the id (`* MAIN 1.0000`) is shown as the deck writes it; its
//! meaning is not known. A source two blocks share belongs to both.
//!
//! The answer is the deck's text alone: no source is opened, none need
//! exist, nothing is indexed.
use super::{chips::wildcard, parser::JobDeck, view::normalize_tc};
use crate::{Error, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Sources an error message lists before "+N more".
const SHOW: usize = 12;

/// One source in one CHIP block.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Member {
    /// the source: its TC as the deck writes it, normalised
    pub source: String,
    /// the CHIP block's id
    pub chip: String,
    /// the CHIP line's text after the id ("* MAIN 1.0000"), as written
    pub tail: String,
    /// the CHIP line's number in the deck
    pub line: usize,
    /// the mask levels the block places the source at, ascending
    pub levels: Vec<i64>,
    /// the block's placements: the positions its ROWS list
    pub placements: usize,
}

/// Every source of the deck in every CHIP block that places it: the sources
/// in the order the deck first uses them, each one's blocks in deck order.
/// `levels`: those levels' entries alone.
pub fn members(deck: &JobDeck, levels: Option<&BTreeSet<i64>>) -> Vec<Member> {
    let mut order: Vec<String> = Vec::new();
    let mut by_source: BTreeMap<String, Vec<Member>> = BTreeMap::new();
    for chip in &deck.chips {
        // this block's sources, each with its levels, in entry order
        let mut here: Vec<(String, Vec<i64>)> = Vec::new();
        for e in &chip.entries {
            if levels.is_some_and(|l| !l.contains(&e.idx)) || e.tc.trim().is_empty() {
                continue;
            }
            let tc = normalize_tc(e.tc.trim());
            match here.iter_mut().find(|(s, _)| *s == tc) {
                Some((_, lv)) => {
                    if !lv.contains(&e.idx) {
                        lv.push(e.idx);
                    }
                }
                None => here.push((tc, vec![e.idx])),
            }
        }
        for (tc, mut lv) in here {
            lv.sort_unstable();
            if !by_source.contains_key(&tc) {
                order.push(tc.clone());
            }
            by_source.entry(tc.clone()).or_default().push(Member {
                source: tc,
                chip: chip.id.clone(),
                tail: chip.tail.trim().into(),
                line: chip.lineno,
                levels: lv,
                placements: chip.rows.len(),
            });
        }
    }
    order
        .into_iter()
        .flat_map(|s| by_source.remove(&s).unwrap_or_default())
        .collect()
}

/// Whether `name` names `source` (a TC, normalised): its file name, the TC
/// path the deck writes, either with the `*`, `?`, `[...]` wildcards, or a
/// path to the file - from `cwd`, as the TC is from `base`.
fn names(name: &str, source: &str, base: &Path, cwd: &Path) -> bool {
    let file = source.rsplit('/').next().unwrap_or(source);
    if wildcard(name, file) || wildcard(&normalize_tc(name), source) {
        return true;
    }
    name.contains('/')
        && normalize_tc(&cwd.join(name).to_string_lossy())
            == normalize_tc(&base.join(source).to_string_lossy())
}

/// The members of the sources `wanted` names (see `names`), in `members`'
/// order, each once however many names name it. A name that names no source
/// is an input error that lists the deck's sources (those of `levels`).
pub fn lookup(
    deck: &JobDeck,
    wanted: &[String],
    base: &Path,
    cwd: &Path,
    levels: Option<&BTreeSet<i64>>,
) -> Result<Vec<Member>> {
    let all = members(deck, levels);
    let mut sources: Vec<String> = Vec::new();
    for m in &all {
        if !sources.contains(&m.source) {
            sources.push(m.source.clone());
        }
    }
    let mut picked: BTreeSet<String> = BTreeSet::new();
    let mut unknown: Vec<&str> = Vec::new();
    for name in wanted {
        let hits: Vec<&String> = sources
            .iter()
            .filter(|s| names(name, s, base, cwd))
            .collect();
        if hits.is_empty() {
            unknown.push(name);
        }
        picked.extend(hits.into_iter().cloned());
    }
    if !unknown.is_empty() {
        let place = match levels {
            Some(l) => format!(
                "level{} {}",
                if l.len() == 1 { "" } else { "s" },
                l.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
            ),
            None => "the deck".into(),
        };
        let mut known = sources
            .iter()
            .take(SHOW)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        if sources.len() > SHOW {
            known.push_str(&format!(", +{} more", sources.len() - SHOW));
        }
        return Err(Error::input(format!(
            "no source of {place} is named {} - its sources: {}",
            unknown
                .iter()
                .map(|n| format!("'{n}'"))
                .collect::<Vec<_>>()
                .join(", "),
            if known.is_empty() { "none".into() } else { known }
        )));
    }
    Ok(all
        .into_iter()
        .filter(|m| picked.contains(&m.source))
        .collect())
}

/// What `--chipset` prints: a line a member, its columns aligned - the
/// source, its CHIP block and that block's line text, the levels it is
/// placed at (`$N` and the MTITLE name), the block's placements, the CHIP
/// line's number in the deck.
pub fn lines(deck: &JobDeck, members: &[Member]) -> Vec<String> {
    let rows: Vec<[String; 6]> = members
        .iter()
        .map(|m| {
            let levels = m
                .levels
                .iter()
                .map(|&l| match deck.title(l) {
                    "" => format!("${l}"),
                    title => format!("${l} {title}"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            [
                m.source.clone(),
                format!("CHIP {}", m.chip),
                m.tail.clone(),
                levels,
                format!(
                    "{} placement{}",
                    m.placements,
                    if m.placements == 1 { "" } else { "s" }
                ),
                format!("line {}", m.line),
            ]
        })
        .collect();
    let mut widths = [0usize; 6];
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    rows.iter()
        .map(|row| {
            let mut text = String::new();
            for (k, cell) in row.iter().enumerate() {
                // a column every row leaves empty (no CHIP line text) is dropped
                if widths[k] == 0 {
                    continue;
                }
                if !text.is_empty() {
                    text.push_str("  ");
                }
                text.push_str(cell);
                text.push_str(&" ".repeat(widths[k] - cell.chars().count()));
            }
            text.trim_end().to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    /// tools/validate_jobdeck.py's test.jb, and a CHIP whose sources sit in
    /// directories (one file name in two of them, one absolute path)
    const DECK: &str = "SLICE 1,17
RETICLE
* test.jb
OPTION PA, AA=0.0200, BA=0.002000, SA=80
MTITLE 1,METAL1
MTITLE 2,VIA1
MTITLE 3,ALIGN
MTITLE 4,NEVER_PLACED
*PLACE-INFO
*
CHIP ID001, * MAIN 1.0000
*
$ (1, METAL1, AD=0.00020, SF=1, TC=chipA.oas, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (2, VIA1, AD=0.00020, SF=1, TC=chipA.oas, LY={456}, DT={0}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (3, ALIGN, AD=0.00020, SF=1, TC=mark.oas, LY={999}, DT={0}, BX=0.0, BY=0.0, UX=100.0, UY=100.0 )
ROWS 85120.0/45020.0
ROWS 85120.0/55020.0
*
CHIP ID002, * MAIN2 1.0000
*
$ (1, METAL1, AD=0.00020, SF=1, TC=chipA.oas, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (2, VIA1, AD=0.00020, SF=0.5, TC=chipB.oas, LY={456}, DT={0}, BX=0.0, BY=0.0, UX=1000.0, UY=1000.0 )
$ (5, GUARD, AD=0.00020, SF=1, TC=chipB.oas, LY={7}, DT={2}, BX=0.0, BY=0.0, UX=1000.0, UY=1000.0 )
ROWS 90000.0/45020.0
ROWS 90000.0/55020.0
ROWS 90000.0/65020.0
*
CHIP ID003, * MAIN3 1.0000
*
$ (1, METAL1, AD=0.00040, SF=1, TC=chipA.oas, LY={123}, DT={43}, BX=0.0, BY=0.0, UX=2000.0, UY=2550.0 )
$ (3, ALIGN, AD=0.00020, SF=1, TC=mark.oas, LY={999}, DT={0}, ZZ=7, BX=0.0, BY=0.0, UX=100.0, UY=100.0 )
ROWS 95000.0/45020.0
CHIP TEG
$ (1, T1, AD=0.00020, SF=1, TC=./teg/x/pad.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=10.0, UY=10.0 )
$ (2, T2, AD=0.00020, SF=1, TC=teg/y/pad.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=10.0, UY=10.0 )
$ (3, T3, AD=0.00020, SF=1, TC=/masks/abs.oas, LY={1}, DT={0}, BX=0.0, BY=0.0, UX=10.0, UY=10.0 )
ROWS 1000.0/2000.0, 3000.0/4000.0
*END-PLACE
END
";

    fn deck() -> JobDeck {
        JobDeck::parse("/decks/test.jb", DECK, true, &AtomicUsize::new(0)).unwrap()
    }

    fn found(names: &[&str], cwd: &str, levels: Option<&[i64]>) -> Result<Vec<(String, String)>> {
        let levels: Option<BTreeSet<i64>> = levels.map(|l| l.iter().copied().collect());
        let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        Ok(lookup(&deck(), &names, Path::new("/decks"), Path::new(cwd), levels.as_ref())?
            .into_iter()
            .map(|m| (m.source, m.chip))
            .collect())
    }

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn every_source_in_every_block_that_places_it() {
        let m = members(&deck(), None);
        let got: Vec<(&str, &str, Vec<i64>, usize, usize)> = m
            .iter()
            .map(|m| (m.source.as_str(), m.chip.as_str(), m.levels.clone(), m.placements, m.line))
            .collect();
        assert_eq!(
            got,
            [
                ("chipA.oas", "ID001", vec![1, 2], 2, 11),
                ("chipA.oas", "ID002", vec![1], 3, 19),
                ("chipA.oas", "ID003", vec![1], 1, 28),
                ("mark.oas", "ID001", vec![3], 2, 11),
                ("mark.oas", "ID003", vec![3], 1, 28),
                ("chipB.oas", "ID002", vec![2, 5], 3, 19),
                ("teg/x/pad.oas", "TEG", vec![1], 2, 33),
                ("teg/y/pad.oas", "TEG", vec![2], 2, 33),
                ("/masks/abs.oas", "TEG", vec![3], 2, 33),
            ]
        );
        assert_eq!(m[0].tail, "* MAIN 1.0000");
        assert_eq!(m[6].tail, "");
        // a level filter keeps that level's entries alone
        let two: BTreeSet<i64> = [2].into();
        let got: Vec<(String, String, Vec<i64>)> = members(&deck(), Some(&two))
            .into_iter()
            .map(|m| (m.source, m.chip, m.levels))
            .collect();
        assert_eq!(
            got,
            [
                ("chipA.oas".to_string(), "ID001".to_string(), vec![2]),
                ("chipB.oas".to_string(), "ID002".to_string(), vec![2]),
                ("teg/y/pad.oas".to_string(), "TEG".to_string(), vec![2]),
            ]
        );
    }

    #[test]
    fn a_source_is_named_by_its_file_name_its_tc_or_a_path_to_it() {
        let a = pairs(&[("chipA.oas", "ID001"), ("chipA.oas", "ID002"), ("chipA.oas", "ID003")]);
        assert_eq!(found(&["chipA.oas"], "/elsewhere", None).unwrap(), a);
        // the TC path, as written or normalised; a path from the current
        // directory; an absolute path
        let x = pairs(&[("teg/x/pad.oas", "TEG")]);
        assert_eq!(found(&["teg/x/pad.oas"], "/elsewhere", None).unwrap(), x);
        assert_eq!(found(&["./teg/x/pad.oas"], "/elsewhere", None).unwrap(), x);
        assert_eq!(found(&["x/pad.oas"], "/decks/teg", None).unwrap(), x);
        assert_eq!(found(&["../x/pad.oas"], "/decks/teg/y", None).unwrap(), x);
        assert_eq!(found(&["/decks/teg/x/pad.oas"], "/", None).unwrap(), x);
        assert_eq!(found(&["../chipA.oas"], "/decks/teg", None).unwrap(), a);
        assert_eq!(
            found(&["/masks/abs.oas"], "/elsewhere", None).unwrap(),
            pairs(&[("/masks/abs.oas", "TEG")])
        );
        // one file name in two directories: both
        assert_eq!(
            found(&["pad.oas"], "/elsewhere", None).unwrap(),
            pairs(&[("teg/x/pad.oas", "TEG"), ("teg/y/pad.oas", "TEG")])
        );
        // wildcards on the file name or the TC; names in any order come
        // out in the deck's, each source once
        assert_eq!(
            found(&["mark.oas", "chip?.oas", "chipA.oas"], "/", None).unwrap(),
            pairs(&[
                ("chipA.oas", "ID001"),
                ("chipA.oas", "ID002"),
                ("chipA.oas", "ID003"),
                ("mark.oas", "ID001"),
                ("mark.oas", "ID003"),
                ("chipB.oas", "ID002"),
            ])
        );
        assert_eq!(found(&["teg/*/pad.oas"], "/", None).unwrap().len(), 2);
        assert_eq!(found(&["*"], "/", None).unwrap().len(), 9);
        // not a file name's part, not a path from elsewhere
        assert!(found(&["chipA"], "/", None).is_err());
        assert!(found(&["pad.oas/"], "/", None).is_err());
        assert!(found(&["teg/x/pad.oas"], "/decks", None).is_ok());
        assert!(found(&["x/pad.oas"], "/elsewhere", None).is_err());
    }

    #[test]
    fn a_name_that_names_nothing_lists_the_sources() {
        let e = found(&["chipA.oas", "nope.oas", "zz*"], "/", None).unwrap_err();
        assert_eq!(e.kind, crate::ErrorKind::InvalidInput);
        assert!(
            e.message.starts_with("no source of the deck is named 'nope.oas', 'zz*' - its sources: chipA.oas, mark.oas, chipB.oas, teg/x/pad.oas"),
            "{}",
            e.message
        );
        // a level filter: the source is in the deck, not at that level
        let e = found(&["mark.oas"], "/", Some(&[2])).unwrap_err();
        assert_eq!(
            e.message,
            "no source of level 2 is named 'mark.oas' - its sources: chipA.oas, chipB.oas, teg/y/pad.oas"
        );
        assert_eq!(
            found(&["chipA.oas"], "/", Some(&[2, 3])).unwrap(),
            pairs(&[("chipA.oas", "ID001")])
        );
    }

    #[test]
    fn the_lines_say_the_block_its_text_levels_placements_and_line() {
        let d = deck();
        let m = lookup(
            &d,
            &["chip?.oas".to_string(), "pad.oas".to_string()],
            Path::new("/decks"),
            &PathBuf::from("/"),
            None,
        )
        .unwrap();
        assert_eq!(
            lines(&d, &m),
            [
                "chipA.oas      CHIP ID001  * MAIN 1.0000   $1 METAL1, $2 VIA1  2 placements  line 11",
                "chipA.oas      CHIP ID002  * MAIN2 1.0000  $1 METAL1           3 placements  line 19",
                "chipA.oas      CHIP ID003  * MAIN3 1.0000  $1 METAL1           1 placement   line 28",
                "chipB.oas      CHIP ID002  * MAIN2 1.0000  $2 VIA1, $5         3 placements  line 19",
                "teg/x/pad.oas  CHIP TEG                    $1 METAL1           2 placements  line 33",
                "teg/y/pad.oas  CHIP TEG                    $2 VIA1             2 placements  line 33",
            ]
        );
        // no block with CHIP line text: the column goes
        let teg = lookup(&d, &["pad.oas".to_string()], Path::new("/decks"), Path::new("/"), None).unwrap();
        assert_eq!(
            lines(&d, &teg),
            [
                "teg/x/pad.oas  CHIP TEG  $1 METAL1  2 placements  line 33",
                "teg/y/pad.oas  CHIP TEG  $2 VIA1    2 placements  line 33",
            ]
        );
        assert!(lines(&d, &[]).is_empty());
    }
}
