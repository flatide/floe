//! Chip on/off for captures (user 2026-10-06: "a capture names a level and
//! a chip"; "several chips - chip on/off"; floe/jobdeck/chips.py).
//!
//! A chip is a row of the chip view: one source under one mask level
//! (docs/JOBDECK.ko.md §1a), the unit the viewer turns on and off - its key
//! is the view's L/D pair (level, source ordinal). A token names chips by
//! the name the chip view lists (the source's file name), the source's TC
//! path, or a CHIP id (the chips that CHIP block places), with `N:` in
//! front for level N alone and shell wildcards (`*`, `?`, `[...]`) in the
//! name. A CHIP block that shares a source with another in the same level
//! turns their common row on or off as a whole - a row is what the view
//! draws or not.
//!
//! The region of a capture is the extent of the placements named: the chips
//! on, or `--fit-chip`'s, which need not be on and whose `#K` picks the K-th
//! placement alone - counted in deck order (the CHIP blocks as the deck
//! lists them, their ROWS in turn); a CHIP id frames that block's placements
//! alone. Extents are the deck's own (the entries' BX..UY placed), so
//! nothing is read to know them.
use super::{geom::Placement, parser::JobDeck, view::normalize_tc, view::LayerMetadata};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

/// Names an error message lists before "+N more".
const SHOW: usize = 12;

/// 'a, b,,c' -> ["a", "b", "c"].
pub fn tokens(text: &str) -> Vec<&str> {
    text.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect()
}

/// A length as the command line takes it: 45020, 44020.5.
pub fn um(v: f64) -> String {
    let v = if v.abs() < 5e-5 { 0. } else { v };
    let text = format!("{v:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// X0,Y0,X1,Y1 - what --bbox and --corners take.
pub fn box_text(b: [f64; 4]) -> String {
    b.map(um).join(",")
}

fn union(boxes: impl IntoIterator<Item = [f64; 4]>) -> Option<[f64; 4]> {
    boxes.into_iter().reduce(|a, b| {
        [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]
    })
}

/// Python's `%r` of a name, for the messages.
fn quoted(text: &str) -> String {
    format!("'{text}'")
}

/// fnmatch.fnmatchcase: `*`, `?`, `[seq]`, `[!seq]`, ranges; anything else
/// literal (a `[` without its `]` too).
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // the class at p[i] ('[' there): Some((matches c, index after it))
    let class = |i: usize, c: char| -> Option<(bool, usize)> {
        let mut j = i + 1;
        let negate = j < p.len() && p[j] == '!';
        if negate {
            j += 1;
        }
        let start = j;
        // a ']' first is a member
        if j < p.len() && p[j] == ']' {
            j += 1;
        }
        while j < p.len() && p[j] != ']' {
            j += 1;
        }
        if j >= p.len() {
            return None;
        }
        let set = &p[start..j];
        let mut hit = false;
        let mut k = 0;
        while k < set.len() {
            if k + 2 < set.len() && set[k + 1] == '-' {
                if set[k] <= c && c <= set[k + 2] {
                    hit = true;
                }
                k += 3;
            } else {
                if set[k] == c {
                    hit = true;
                }
                k += 1;
            }
        }
        Some((hit != negate, j + 1))
    };
    // iterative glob with backtracking to the last '*'
    let (mut i, mut j) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while j < t.len() {
        if i < p.len() {
            match p[i] {
                '*' => {
                    star = Some((i, j));
                    i += 1;
                    continue;
                }
                '?' => {
                    i += 1;
                    j += 1;
                    continue;
                }
                '[' => {
                    if let Some((hit, next)) = class(i, t[j]) {
                        if hit {
                            i = next;
                            j += 1;
                            continue;
                        }
                    } else if t[j] == '[' {
                        i += 1;
                        j += 1;
                        continue;
                    }
                }
                c if c == t[j] => {
                    i += 1;
                    j += 1;
                    continue;
                }
                _ => {}
            }
        }
        match star {
            Some((si, sj)) => {
                i = si + 1;
                j = sj + 1;
                star = Some((si, sj + 1));
            }
            None => return false,
        }
    }
    p[i..].iter().all(|&c| c == '*')
}

/// One placement of a chip: a CHIP block at one of its ROWS.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    /// 1-based, deck order
    pub n: usize,
    pub chip: String,
    /// the ROWS index in that block (0-based)
    pub row: usize,
    /// the ROWS position, um
    pub at: [f64; 2],
    pub bbox: [f64; 4],
}

#[derive(Clone, Debug)]
pub struct ChipRow {
    pub level: i64,
    /// the view's L/D key: (level, source ordinal)
    pub key: (i64, i64),
    /// what the chip view lists: the source's file name
    pub name: String,
    /// the source path, normalised
    pub tc: String,
    /// CHIP ids, deck order
    pub chips: Vec<String>,
    /// indices of the load's placements
    pub places: Vec<usize>,
}
impl ChipRow {
    pub fn label(&self) -> String {
        format!("{}:{}", self.level, self.name)
    }
}

/// What one capture's chip options come to.
#[derive(Clone, Debug)]
pub struct Selection {
    /// the chips on, indices into the table's rows (None: the options name none)
    pub rows: Option<Vec<usize>>,
    /// their L/D keys, for the render's visible set
    pub keys: Option<Vec<(i64, i64)>>,
    /// um: the placements' extent the options frame
    pub region: Option<[f64; 4]>,
    /// how many placements that extent covers
    pub placements: usize,
    /// what the region is ("the chips on", "--fit-chip X")
    pub frame: String,
}

/// The chip view's rows of a loaded deck.
pub struct ChipTable<'a> {
    placements: &'a [Placement],
    deck_levels: BTreeSet<i64>,
    titles: BTreeMap<i64, String>,
    pub rows: Vec<ChipRow>,
    by_source: BTreeMap<(i64, String), usize>,
    chip_pos: BTreeMap<String, usize>,
}

impl<'a> ChipTable<'a> {
    /// `layers`: the view's meta rows (the level heads and their chips);
    /// `placements`: the load's.
    pub fn new(deck: &JobDeck, placements: &'a [Placement], layers: &[LayerMetadata]) -> Self {
        let mut titles = BTreeMap::new();
        let mut rows = Vec::new();
        let mut by_source = BTreeMap::new();
        for l in layers {
            if l.jobdeck_head {
                titles.insert(l.layer, l.name.clone());
                continue;
            }
            // the source layer view has no chips
            let Some(source) = &l.jobdeck_source else {
                continue;
            };
            let row = ChipRow {
                level: l.layer,
                key: (l.layer, l.datatype),
                name: l.name.clone(),
                tc: normalize_tc(source),
                chips: Vec::new(),
                places: Vec::new(),
            };
            by_source.insert((row.level, row.tc.clone()), rows.len());
            rows.push(row);
        }
        let mut chip_pos = BTreeMap::new();
        for c in &deck.chips {
            let n = chip_pos.len();
            chip_pos.entry(c.id.clone()).or_insert(n);
            for e in &c.entries {
                if let Some(&r) = by_source.get(&(e.idx, normalize_tc(&e.tc))) {
                    if !rows[r].chips.contains(&c.id) {
                        rows[r].chips.push(c.id.clone());
                    }
                }
            }
        }
        for (i, p) in placements.iter().enumerate() {
            if let Some(&r) = by_source.get(&(p.idx, normalize_tc(&p.tc))) {
                rows[r].places.push(i);
            }
        }
        Self {
            placements,
            deck_levels: deck.levels(),
            titles,
            rows,
            by_source,
            chip_pos,
        }
    }

    /// The levels whose chips this load holds.
    pub fn levels(&self) -> Vec<i64> {
        self.rows
            .iter()
            .map(|r| r.level)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn row_of(&self, idx: i64, tc: &str) -> Option<&ChipRow> {
        self.by_source
            .get(&(idx, normalize_tc(tc)))
            .map(|&r| &self.rows[r])
    }

    fn known(&self, level: Option<i64>) -> String {
        let rows: Vec<&ChipRow> = self
            .rows
            .iter()
            .filter(|r| level.is_none_or(|l| r.level == l))
            .collect();
        let mut names: Vec<&str> = Vec::new();
        let mut ids: Vec<&str> = Vec::new();
        for r in &rows {
            if !names.contains(&r.name.as_str()) {
                names.push(&r.name);
            }
            for c in &r.chips {
                if !ids.contains(&c.as_str()) {
                    ids.push(c);
                }
            }
        }
        let cap = |items: &[&str]| -> String {
            let mut text = items
                .iter()
                .take(SHOW)
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            if items.len() > SHOW {
                text.push_str(&format!(", +{} more", items.len() - SHOW));
            }
            if text.is_empty() {
                "none".into()
            } else {
                text
            }
        };
        let place = match level {
            Some(l) => format!("level {l}"),
            None => format!(
                "the loaded levels ({})",
                self.levels()
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        };
        format!(
            "chips of {place}: {}; CHIP ids: {} (`{} info DECK --chips` lists them)",
            cap(&names),
            cap(&ids),
            crate::program()
        )
    }

    /// (picks, k): the rows a token names, each with the CHIP ids that
    /// named it (None: named by its source), and its #K.
    #[allow(clippy::type_complexity)]
    pub fn matches(
        &self,
        token: &str,
        allow_k: bool,
    ) -> Result<(Vec<(usize, Option<Vec<String>>)>, Option<usize>)> {
        let token = token.trim();
        let mut rest = token;
        let mut level = None;
        if let Some((head, tail)) = rest.split_once(':') {
            if !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit()) {
                level = Some(head.parse::<i64>().map_err(|_| {
                    Error::input(format!("chip {}: level out of range", quoted(token)))
                })?);
                rest = tail;
            }
        }
        let mut k = None;
        if let Some(at) = rest.rfind('#') {
            let digits = &rest[at + 1..];
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                k = Some(digits.parse::<usize>().map_err(|_| {
                    Error::input(format!("chip {}: #K out of range", quoted(token)))
                })?);
                rest = &rest[..at];
            }
        }
        let name = rest.trim();
        if name.is_empty() {
            return Err(Error::input(format!(
                "chip {} names nothing",
                quoted(token)
            )));
        }
        if k.is_some() && !allow_k {
            return Err(Error::input(format!(
                "chip {}: #K picks one placement for the region (--fit-chip); a chip is on or off as a whole",
                quoted(token)
            )));
        }
        let levels = self.levels();
        if let Some(l) = level {
            if !levels.contains(&l) {
                let list = |v: &mut dyn Iterator<Item = &i64>| {
                    v.map(i64::to_string).collect::<Vec<_>>().join(",")
                };
                if self.deck_levels.contains(&l) {
                    return Err(Error::input(format!(
                        "chip {}: level {l} is not loaded (--level {})",
                        quoted(token),
                        list(&mut levels.iter())
                    )));
                }
                return Err(Error::input(format!(
                    "chip {}: level {l} is not in the deck (it places {})",
                    quoted(token),
                    list(&mut self.deck_levels.iter())
                )));
            }
        }
        let wild = name.contains(['*', '?', '[']);
        let path = normalize_tc(name);
        let hit = |text: &str| {
            if wild {
                wildcard(name, text)
            } else {
                text == name
            }
        };
        let mut picks = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            if level.is_some_and(|l| row.level != l) {
                continue;
            }
            let by_tc = if wild {
                wildcard(name, &row.tc)
            } else {
                row.tc == path
            };
            if hit(&row.name) || by_tc {
                picks.push((i, None));
                continue;
            }
            let ids: Vec<String> = row.chips.iter().filter(|c| hit(c)).cloned().collect();
            if !ids.is_empty() {
                picks.push((i, Some(ids)));
            }
        }
        if picks.is_empty() {
            return Err(Error::input(format!(
                "chip {} matches no chip - {}",
                quoted(token),
                self.known(level)
            )));
        }
        Ok((picks, k))
    }

    /// The rows the names in `text` name, in the table's order.
    pub fn rows_of(&self, text: &str) -> Result<Vec<usize>> {
        let mut want = BTreeSet::new();
        for token in tokens(text) {
            for (r, _) in self.matches(token, false)?.0 {
                want.insert(self.rows[r].key);
            }
        }
        Ok((0..self.rows.len())
            .filter(|&r| want.contains(&self.rows[r].key))
            .collect())
    }

    /// The placements the picks name, one per CHIP block and ROWS position,
    /// in deck order (a position whose rows several picks name is one
    /// placement, their extents together).
    pub fn instances(&self, picks: &[(usize, Option<Vec<String>>)]) -> Vec<Instance> {
        let mut acc: BTreeMap<(String, usize), ([f64; 2], [f64; 4])> = BTreeMap::new();
        for (r, ids) in picks {
            for &i in &self.rows[*r].places {
                let p = &self.placements[i];
                if ids.as_ref().is_some_and(|ids| !ids.contains(&p.chip)) {
                    continue;
                }
                let key = (p.chip.clone(), p.row);
                let at = [p.jx, p.jy];
                let bbox = match acc.get(&key) {
                    Some((_, prev)) => union([*prev, p.bbox_um]).unwrap(),
                    None => p.bbox_um,
                };
                acc.insert(key, (at, bbox));
            }
        }
        let mut order: Vec<&(String, usize)> = acc.keys().collect();
        order.sort_by_key(|(chip, row)| (self.chip_pos.get(chip).copied().unwrap_or(0), *row));
        order
            .into_iter()
            .enumerate()
            .map(|(n, key)| {
                let (at, bbox) = acc[key];
                Instance {
                    n: n + 1,
                    chip: key.0.clone(),
                    row: key.1,
                    at,
                    bbox,
                }
            })
            .collect()
    }

    /// (region, placements) of `--fit-chip`: every token's placements, or
    /// its #K alone.
    pub fn fit(&self, text: &str) -> Result<([f64; 4], usize)> {
        let mut boxes = Vec::new();
        for token in tokens(text) {
            let (picks, k) = self.matches(token, true)?;
            let mut insts = self.instances(&picks);
            if insts.is_empty() {
                return Err(Error::input(format!(
                    "chip {} has nothing placed (its source is skipped: see 'skipped' above)",
                    quoted(token)
                )));
            }
            if let Some(k) = k {
                if !(1..=insts.len()).contains(&k) {
                    return Err(Error::input(format!(
                        "chip {}: it has {} placement{} (#1..#{}; `{} info DECK --chips` lists them)",
                        quoted(token),
                        insts.len(),
                        if insts.len() == 1 { "" } else { "s" },
                        insts.len(),
                        crate::program()
                    )));
                }
                insts = vec![insts.swap_remove(k - 1)];
            }
            boxes.extend(insts.iter().map(|i| i.bbox));
        }
        let count = boxes.len();
        let region = union(boxes).ok_or_else(|| Error::input("--fit-chip names nothing"))?;
        Ok((region, count))
    }

    /// A capture's chips: `on`'s (every chip loaded when only `off` is
    /// given) less `off`'s, and the region - `fit`'s placements, else those
    /// of the chips on (no rows: the options name no chips).
    pub fn select(
        &self,
        on: Option<&str>,
        off: Option<&str>,
        fit: Option<&str>,
    ) -> Result<Selection> {
        let on = on.filter(|s| !s.is_empty());
        let off = off.filter(|s| !s.is_empty());
        let fit = fit.filter(|s| !s.is_empty());
        let mut rows = None;
        if on.is_some() || off.is_some() {
            let mut picked = match on {
                Some(on) => self.rows_of(on)?,
                None => (0..self.rows.len()).collect(),
            };
            if let Some(off) = off {
                let drop: BTreeSet<(i64, i64)> = self
                    .rows_of(off)?
                    .into_iter()
                    .map(|r| self.rows[r].key)
                    .collect();
                picked.retain(|&r| !drop.contains(&self.rows[r].key));
            }
            if picked.is_empty() {
                return Err(Error::input(format!(
                    "no chip is left on (--chip {}, --chip-off {})",
                    on.unwrap_or("-"),
                    off.unwrap_or("-")
                )));
            }
            rows = Some(picked);
        }
        let (mut region, mut count, mut frame) = (None, 0, String::new());
        if let Some(fit) = fit {
            let (r, n) = self.fit(fit)?;
            (region, count, frame) = (Some(r), n, format!("--fit-chip {fit}"));
        } else if let Some(rows) = &rows {
            let picks: Vec<_> = rows.iter().map(|&r| (r, None)).collect();
            let insts = self.instances(&picks);
            region = union(insts.iter().map(|i| i.bbox));
            count = insts.len();
            frame = "the chips on".into();
        }
        Ok(Selection {
            keys: rows
                .as_ref()
                .map(|rows| rows.iter().map(|&r| self.rows[r].key).collect()),
            rows,
            region,
            placements: count,
            frame,
        })
    }

    /// The labels (`2:chipB.oas`) of a selection's chips on.
    pub fn labels(&self, sel: &Selection) -> Option<Vec<String>> {
        sel.rows
            .as_ref()
            .map(|rows| rows.iter().map(|&r| self.rows[r].label()).collect())
    }

    /// One log line: the chips on and the region.
    pub fn describe(&self, sel: &Selection) -> String {
        let mut parts = Vec::new();
        if let Some(names) = self.labels(sel) {
            let mut shown = names.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
            if names.len() > 8 {
                shown.push_str(&format!(", +{} more", names.len() - 8));
            }
            parts.push(format!(
                "chips on {} of {} ({shown})",
                names.len(),
                self.rows.len()
            ));
        }
        if let Some(region) = sel.region {
            parts.push(format!(
                "region {} um ({}, {} placement{})",
                box_text(region),
                sel.frame,
                sel.placements,
                if sel.placements == 1 { "" } else { "s" }
            ));
        }
        parts.join("; ")
    }

    /// `floe2 info deck.jb --chips`: every chip of the load by level, its
    /// CHIP blocks, its placements and their extents in the form --bbox and
    /// --corners take.
    pub fn listing(&self) -> Vec<String> {
        let mut out = Vec::new();
        for level in self.levels() {
            out.push(format!(
                "${level} {}",
                self.titles.get(&level).map(String::as_str).unwrap_or("")
            ));
            for (r, row) in self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.level == level)
            {
                let insts = self.instances(&[(r, None)]);
                let extent = union(insts.iter().map(|i| i.bbox));
                out.push(format!(
                    "  {}/{} {}  CHIP {}  {} placement{}{}",
                    row.key.0,
                    row.key.1,
                    row.name,
                    if row.chips.is_empty() {
                        "-".into()
                    } else {
                        row.chips.join(",")
                    },
                    insts.len(),
                    if insts.len() == 1 { "" } else { "s" },
                    match extent {
                        Some(b) => format!("  {} um", box_text(b)),
                        None => "  (source skipped)".into(),
                    }
                ));
                if row.tc != row.name {
                    out.push(format!("      source {}", row.tc));
                }
                if insts.len() > 1 {
                    for i in &insts {
                        out.push(format!(
                            "      #{} CHIP {} ROWS {} at {},{}: {} um",
                            i.n,
                            i.chip,
                            i.row + 1,
                            um(i.at[0]),
                            um(i.at[1]),
                            box_text(i.bbox)
                        ));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_are_fnmatchcase() {
        for (p, t, want) in [
            ("chip?.oas", "chipA.oas", true),
            ("chip?.oas", "chipAB.oas", false),
            ("*", "", true),
            ("*.oas", "a.oas", true),
            ("*.oas", "a.gds", false),
            ("ID00[12]", "ID002", true),
            ("ID00[!12]", "ID002", false),
            ("ID00[!12]", "ID003", true),
            ("ID00[1-3]", "ID003", true),
            ("ID00[]", "ID00[]", true),
            ("a*b*c", "aXXbYYc", true),
            ("a*b*c", "aXXbYY", false),
            ("Chip*", "chipA", false),
        ] {
            assert_eq!(wildcard(p, t), want, "{p} {t}");
        }
    }

    #[test]
    fn lengths_read_as_the_command_line_takes_them() {
        assert_eq!(um(45020.), "45020");
        assert_eq!(um(44020.5), "44020.5");
        assert_eq!(um(-0.00001), "0");
        assert_eq!(um(0.12345), "0.1235");
        assert_eq!(box_text([1., 2.5, -3., 4.25]), "1,2.5,-3,4.25");
        assert_eq!(tokens(" a, b,,c "), ["a", "b", "c"]);
    }
}
