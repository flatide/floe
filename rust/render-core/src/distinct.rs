//! Distinct colours for the deck layers a picture paints (user 2026-11-11:
//! "가능하면 칠해진 레이어의 색이 겹치지 않아야 하는데 현재는 겹치기도 함.
//! 따라서 실제 칠해지는 레이어의 색을 겹치지 않게 하려고함. 실제로 칠할때
//! 색을 결정하고 이전에 그린 색과 최대한 겹치지 않게 선택하면 될 것 같음.").
//!
//! A deck view gives its rows their colours by their place in a palette of
//! ten, so of the rows a capture shows two may have one colour - level 1 and
//! level 11, the third chip and the thirteenth - and its legend then lists
//! two rows that cannot be told apart. With DeckRenderRequest::distinct the
//! colour of a row is decided when a pass of it first leaves a pixel in the
//! frame (Deck::render lays the passes opaque-over in placement order, each
//! of one colour): its own colour while no row painted before it has it,
//! else the colour of the palette that is farthest from every colour painted
//! so far. A row that paints nothing takes no colour, so the few rows of a
//! picture are told apart by the palette's best, however many rows the deck
//! has. The rows a view colours alike on purpose - a level's chips in the
//! level view, a source chip under every level in the chip view - are one
//! colour still (`RowsBy`): what is told apart is what the palette's turn
//! made alike.

use std::collections::{BTreeMap, BTreeSet};

/// A deck layer's key (`layer/datatype`; DeckLayer).
pub type Key = (u32, u32);

/// Which rows are one colour and decided together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowsBy {
    /// every key its own
    #[default]
    Pair,
    /// the rows of a layer number (a jobdeck's level view: a level and the
    /// chips under it; its layer view: a source layer's datatypes)
    Layer,
    /// the rows of a datatype number (a jobdeck's chip view: a source chip,
    /// whose place among the deck's sources is its rows' datatype, under
    /// every level)
    Datatype,
}

impl RowsBy {
    /// The key the rows that are one colour with `key` share.
    pub fn group(self, key: Key) -> Key {
        match self {
            Self::Pair => key,
            Self::Layer => (key.0, 0),
            Self::Datatype => (0, key.1),
        }
    }
}

/// What a frame is told to decide its layers' colours by.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DistinctColors {
    /// The rows that are one colour.
    pub by: RowsBy,
    /// The colours to choose from, in tiers: a tier is turned to when every
    /// colour of the tiers before it is in use (the view's palette, then the
    /// reserve).
    pub palette: Vec<Vec<[u8; 4]>>,
    /// Rows whose colour is theirs to keep (the layer properties gave it).
    /// The rows that are one colour with such a row are left as they are,
    /// each in its own colour, and no other row is given a colour of
    /// theirs - from the frame's start, whether they paint or not.
    pub fixed: BTreeSet<Key>,
    /// The colours decided before this frame, by a row of those that are
    /// one colour: the picture's earlier frames (a mosaic's tiles are one
    /// picture). They stay, and count as painted.
    pub taken: BTreeMap<Key, [u8; 4]>,
}

/// A colour in OKLab, in 1/10000 (whole numbers: the choice must not turn
/// on the last bit of a platform's cube root).
type Lab = [i64; 3];

fn lab(color: [u8; 4]) -> Lab {
    let linear = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (linear(color[0]), linear(color[1]), linear(color[2]));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    [
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    ]
    .map(|v| (v * 10_000.0).round() as i64)
}

fn distance2(a: Lab, b: Lab) -> i64 {
    (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum()
}

/// A frame's colours as they are decided (Deck::render).
pub(crate) struct Paints<'a> {
    request: &'a DistinctColors,
    /// the groups (RowsBy::group) with a fixed row: left as they are
    fixed: BTreeSet<Key>,
    /// the colour of each group decided in the picture: those taken before
    /// the frame, and those of the groups that painted in it
    colors: BTreeMap<Key, [u8; 4]>,
    /// the colours in use - the fixed groups' rows', the taken, the decided -
    /// each with the number of groups that have it
    used: Vec<([u8; 4], Lab, u32)>,
    /// the rows that painted in this frame, and in which colour
    painted: BTreeMap<Key, [u8; 4]>,
}

impl<'a> Paints<'a> {
    /// `layers`: the deck's layers, key and colour (the rows of a fixed
    /// group keep their layers' colours).
    pub(crate) fn new(
        request: &'a DistinctColors,
        layers: impl Iterator<Item = (Key, [u8; 4])>,
    ) -> Self {
        let mut paints = Self {
            request,
            fixed: request.fixed.iter().map(|&key| request.by.group(key)).collect(),
            colors: BTreeMap::new(),
            used: Vec::new(),
            painted: BTreeMap::new(),
        };
        // every colour of a fixed group's rows is in use, once a group
        let mut seen = BTreeSet::new();
        for (key, color) in layers {
            let group = request.by.group(key);
            if paints.fixed.contains(&group) && seen.insert((group, color)) {
                paints.use_color(color);
            }
        }
        for (&row, &color) in &request.taken {
            let group = request.by.group(row);
            if !paints.fixed.contains(&group) && paints.colors.insert(group, color).is_none() {
                paints.use_color(color);
            }
        }
        paints
    }

    fn use_color(&mut self, color: [u8; 4]) {
        match self.used.iter_mut().find(|(c, _, _)| *c == color) {
            Some(entry) => entry.2 += 1,
            None => self.used.push((color, lab(color), 1)),
        }
    }

    fn users(&self, color: [u8; 4]) -> u32 {
        self.used
            .iter()
            .find(|(c, _, _)| *c == color)
            .map_or(0, |entry| entry.2)
    }

    /// The colour of the palette farthest from the colours in use and from
    /// the black the frame is laid on: of the first tier with a colour not
    /// in use, the one whose nearest colour in use is farthest (the earlier
    /// one of two as far). With every colour in use, the one the fewest rows
    /// have. None: no palette.
    fn pick(&self) -> Option<[u8; 4]> {
        let black = lab([0, 0, 0, 255]);
        for tier in &self.request.palette {
            let mut best: Option<(i64, [u8; 4])> = None;
            for &color in tier {
                if self.users(color) != 0 {
                    continue;
                }
                let at = lab(color);
                let nearest = self
                    .used
                    .iter()
                    .map(|(_, used, _)| distance2(at, *used))
                    .fold(distance2(at, black), i64::min);
                if best.is_none_or(|(far, _)| nearest > far) {
                    best = Some((nearest, color));
                }
            }
            if let Some((_, color)) = best {
                return Some(color);
            }
        }
        let mut best: Option<(u32, [u8; 4])> = None;
        for &color in self.request.palette.iter().flatten() {
            let users = self.users(color);
            if best.is_none_or(|(least, _)| users < least) {
                best = Some((users, color));
            }
        }
        best.map(|(_, color)| color)
    }

    /// The colour the layer `key`, whose own is `own`, is painted in - asked
    /// when a pass of it leaves a pixel in the frame. Its own in a fixed
    /// group; else its group's, decided the first time a row of the group
    /// paints: the colour taken before the frame, or `own` when no colour in
    /// use is that, or the palette's farthest (`pick`).
    pub(crate) fn paint(&mut self, key: Key, own: [u8; 4]) -> [u8; 4] {
        let group = self.request.by.group(key);
        let color = if self.fixed.contains(&group) {
            // in use since the frame's start
            own
        } else if let Some(&color) = self.colors.get(&group) {
            color
        } else {
            let color = if self.users(own) == 0 {
                own
            } else {
                self.pick().unwrap_or(own)
            };
            self.use_color(color);
            self.colors.insert(group, color);
            color
        };
        self.painted.insert(key, color);
        color
    }

    /// The rows that painted in the frame and the colour each was painted
    /// in, by key.
    pub(crate) fn painted(&self) -> Vec<(Key, [u8; 4])> {
        self.painted.iter().map(|(&key, &color)| (key, color)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn rgb(value: u32) -> [u8; 4] {
        [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255]
    }
    const BLUE: [u8; 4] = rgb(0x0000ff);
    const YELLOW: [u8; 4] = rgb(0xffff00);
    const RED: [u8; 4] = rgb(0xff0000);
    const PINK: [u8; 4] = rgb(0xffc0cb);
    const ORANGE: [u8; 4] = rgb(0xffa500);
    const WHITE: [u8; 4] = rgb(0xffffff);
    const PURPLE: [u8; 4] = rgb(0xa020f0);
    const CYAN: [u8; 4] = rgb(0x00ffff);
    const MAGENTA: [u8; 4] = rgb(0xff00ff);
    const GREEN: [u8; 4] = rgb(0x00ff00);
    /// the jobdeck palette (app-core jobdeck::color::PALETTE), in its order
    const TEN: [[u8; 4]; 10] = [
        BLUE, YELLOW, RED, PINK, ORANGE, WHITE, PURPLE, CYAN, MAGENTA, GREEN,
    ];
    const STEEL: [u8; 4] = rgb(0x418dd9);
    const FOREST: [u8; 4] = rgb(0x00b85c);

    fn request(by: RowsBy) -> DistinctColors {
        DistinctColors {
            by,
            palette: vec![TEN.to_vec(), vec![RED, STEEL, FOREST]],
            ..DistinctColors::default()
        }
    }

    #[test]
    fn a_row_keeps_its_colour_until_one_painted_before_it_has_it() {
        let request = request(RowsBy::Pair);
        let mut paints = Paints::new(&request, std::iter::empty());
        // the first row: its own; a row of another colour: its own
        assert_eq!(paints.paint((1, 1), BLUE), BLUE);
        assert_eq!(paints.paint((1, 2), RED), RED);
        // asked again (the row's next pass, a streamed pass's next slice):
        // the colour decided
        assert_eq!(paints.paint((1, 1), BLUE), BLUE);
        // a row whose colour a painted row has: the palette's farthest from
        // blue, red and the black under them
        let third = paints.paint((2, 1), BLUE);
        assert_eq!(third, GREEN);
        assert_eq!(paints.paint((2, 1), BLUE), GREEN);
        // the colour a row was given is in use like any other: the row
        // whose own it is takes another
        let fourth = paints.paint((3, 1), GREEN);
        assert!(fourth != GREEN && TEN.contains(&fourth), "{fourth:?}");
        // the rows that painted, by key, with what they were painted in
        assert_eq!(
            paints.painted(),
            vec![((1, 1), BLUE), ((1, 2), RED), ((2, 1), GREEN), ((3, 1), fourth)]
        );
    }

    #[test]
    fn the_colours_given_are_the_palettes_farthest_first_then_the_reserves() {
        // twelve rows of one colour: the first keeps it, nine take the other
        // nine of the palette - each the farthest from those before it -,
        // and the palette used up the reserve's are turned to, a colour in
        // use (the reserve's red) never
        let request = request(RowsBy::Pair);
        let mut paints = Paints::new(&request, std::iter::empty());
        let given: Vec<_> = (1..=12).map(|n| paints.paint((n, 0), BLUE)).collect();
        assert_eq!(
            given,
            vec![
                BLUE, YELLOW, RED, MAGENTA, PINK, CYAN, GREEN, PURPLE, ORANGE, WHITE, STEEL,
                FOREST
            ]
        );
        // every colour in use: the one the fewest rows have, the earlier of
        // two - no row is left without a colour of the palette
        assert_eq!(paints.paint((13, 0), BLUE), BLUE);
        assert_eq!(paints.paint((14, 0), BLUE), YELLOW);
        // no palette: the row's own
        let bare = DistinctColors::default();
        let mut paints = Paints::new(&bare, std::iter::empty());
        assert_eq!(paints.paint((1, 0), BLUE), BLUE);
        assert_eq!(paints.paint((2, 0), BLUE), BLUE);
    }

    #[test]
    fn the_rows_a_view_colours_alike_are_one_colour_still() {
        // a level view: level 1's chips are the level's colour, level 11's -
        // blue as well - another, the same for each of its chips
        let request = request(RowsBy::Layer);
        let mut paints = Paints::new(&request, std::iter::empty());
        assert_eq!(paints.paint((1, 1), BLUE), BLUE);
        assert_eq!(paints.paint((1, 2), BLUE), BLUE);
        assert_eq!(paints.paint((11, 1), BLUE), YELLOW);
        assert_eq!(paints.paint((11, 3), BLUE), YELLOW);
        // said by the rows that painted
        assert_eq!(
            paints.painted(),
            vec![((1, 1), BLUE), ((1, 2), BLUE), ((11, 1), YELLOW), ((11, 3), YELLOW)]
        );
        // a chip view: the third source chip is red under every level, the
        // thirteenth - red as well by the palette's turn - another
        let request = self::request(RowsBy::Datatype);
        let mut paints = Paints::new(&request, std::iter::empty());
        assert_eq!(paints.paint((1, 3), RED), RED);
        assert_eq!(paints.paint((2, 3), RED), RED);
        let other = paints.paint((1, 13), RED);
        assert!(other != RED && TEN.contains(&other), "{other:?}");
        assert_eq!(paints.paint((2, 13), RED), other);
        // each key its own colour: the second chip of level 1 is given one
        let request = self::request(RowsBy::Pair);
        let mut paints = Paints::new(&request, std::iter::empty());
        assert_eq!(paints.paint((1, 1), BLUE), BLUE);
        assert_eq!(paints.paint((1, 2), BLUE), YELLOW);
        // the keys the rows of one colour share
        assert_eq!(RowsBy::Pair.group((7, 3)), (7, 3));
        assert_eq!(RowsBy::Layer.group((7, 3)), (7, 0));
        assert_eq!(RowsBy::Datatype.group((7, 3)), (0, 3));
    }

    #[test]
    fn a_fixed_row_keeps_its_colour_and_no_other_row_is_given_it() {
        // row 5/0's colour is the layer properties': red, in use from the
        // start - the row painted first whose own is red takes another, and
        // the fixed row is red whenever it paints
        let mut request = request(RowsBy::Pair);
        request.fixed.insert((5, 0));
        let layers = [((1, 0), RED), ((5, 0), RED), ((6, 0), BLUE)];
        let mut paints = Paints::new(&request, layers.into_iter());
        let first = paints.paint((1, 0), RED);
        assert!(first != RED && TEN.contains(&first), "{first:?}");
        assert_eq!(paints.paint((5, 0), RED), RED);
        // nor is red given to a row in need of a colour
        let mut seen = vec![first, RED];
        for n in 10..17 {
            let color = paints.paint((n, 0), first);
            assert!(!seen.contains(&color), "{color:?} twice");
            seen.push(color);
        }
        // a fixed row that paints nothing is not among the painted
        let mut paints = Paints::new(&request, layers.into_iter());
        paints.paint((6, 0), BLUE);
        assert_eq!(paints.painted(), vec![((6, 0), BLUE)]);
        // a chip view's chip one of whose rows the layer properties
        // coloured: the chip's rows are left as they are, each in its own
        // colour, and neither colour is given to another chip
        let mut request = self::request(RowsBy::Datatype);
        request.fixed.insert((2, 3));
        let layers = [((1, 3), RED), ((2, 3), STEEL), ((1, 13), RED), ((1, 4), BLUE)];
        let mut paints = Paints::new(&request, layers.into_iter());
        let other = paints.paint((1, 13), RED);
        assert!(other != RED && other != STEEL, "{other:?}");
        assert_eq!(paints.paint((1, 3), RED), RED);
        assert_eq!(paints.paint((2, 3), STEEL), STEEL);
        assert_eq!(
            paints.painted(),
            vec![((1, 3), RED), ((1, 13), other), ((2, 3), STEEL)]
        );
    }

    #[test]
    fn the_colours_taken_before_the_frame_stay() {
        // a mosaic's second tile: level 11 was given yellow in the first -
        // it is yellow here, level 2, whose own yellow is, takes another,
        // and a row taken that does not paint here is not said painted
        let mut request = request(RowsBy::Layer);
        request.taken = BTreeMap::from([((1, 2), BLUE), ((11, 1), YELLOW), ((11, 3), YELLOW)]);
        let mut paints = Paints::new(&request, std::iter::empty());
        assert_eq!(paints.paint((11, 4), BLUE), YELLOW);
        let second = paints.paint((2, 1), YELLOW);
        assert!(second != YELLOW && second != BLUE && TEN.contains(&second), "{second:?}");
        assert_eq!(paints.painted(), vec![((2, 1), second), ((11, 4), YELLOW)]);
    }

    #[test]
    fn the_distance_is_a_perceptual_one_in_whole_numbers() {
        // black is the origin; white is one whole lightness, no chroma
        assert_eq!(lab([0, 0, 0, 255]), [0, 0, 0]);
        assert_eq!(lab(WHITE), [10_000, 0, 0]);
        let d = |a, b| distance2(lab(a), lab(b));
        // the palette's nearest pairs are its light ones; red is far from
        // blue and near orange
        assert!(d(PINK, WHITE) < d(RED, ORANGE));
        assert!(d(RED, ORANGE) < d(RED, BLUE));
        assert!(d(BLUE, PURPLE) < d(BLUE, YELLOW));
        assert_eq!(d(RED, GREEN), d(GREEN, RED));
    }
}
