//! Baked structural overview, not a second renderer. World math stays here;
//! HTTP readers only borrow immutable 180x180 palette-indexed base images.
use super::Viewport;
use crate::{catalog, Error, ErrorKind, Result};
use serde::{de::SeqAccess, Deserialize, Deserializer, Serialize};
use std::{
    fmt,
    io::{BufReader, Read},
};

pub const SIZE: usize = 180;
const DEPTHS: usize = 32;
const ROWS: usize = 6000;
const META_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug)]
pub struct Minimap {
    plain: String,
    bases: Vec<String>,
}
impl Default for Minimap {
    fn default() -> Self {
        Self::plain([0.; 4])
    }
}

// Do not deserialize untrusted Vec lengths before checking the bake bounds.
#[derive(Debug)]
struct Bounded<T, const N: usize>(Vec<T>);
impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for Bounded<T, N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor<T, const N: usize>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> serde::de::Visitor<'de> for Visitor<T, N> {
            type Value = Bounded<T, N>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "at most {N} overview entries")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element()? {
                    if out.len() == N {
                        return Err(serde::de::Error::custom("overview entry limit"));
                    }
                    out.push(v);
                }
                Ok(Bounded(out))
            }
        }
        d.deserialize_seq(Visitor::<T, N>(std::marker::PhantomData))
    }
}
#[derive(Debug, Deserialize)]
struct Frontier {
    // Older caches may contain just four coordinates, newer ones add a band.
    depths: Bounded<Bounded<Bounded<f64, 5>, ROWS>, DEPTHS>,
}
#[derive(Deserialize)]
struct Header {
    bbox: [i64; 4],
    dbu: f64,
    src: Identity,
    #[serde(default)]
    frontier: Option<Frontier>,
}
#[derive(Deserialize)]
struct Identity {
    size: u64,
    mtime: u64,
}
#[derive(Clone, Copy, Debug)]
struct Geometry {
    bbox: [f64; 4],
    scale: f64,
    die: [f64; 4],
}
impl Geometry {
    fn new(bbox: [f64; 4]) -> Option<Self> {
        let w = bbox[2] - bbox[0];
        let h = bbox[3] - bbox[1];
        if !bbox.iter().all(|v| v.is_finite()) || w <= 0. || h <= 0. {
            return None;
        }
        let scale = SIZE as f64 / w.max(h);
        let mw = (w * scale).round_ties_even().max(2.);
        let mh = (h * scale).round_ties_even().max(2.);
        Some(Self {
            bbox,
            scale,
            die: [
                ((SIZE as f64 - mw) / 2.).floor(),
                ((SIZE as f64 - mh) / 2.).floor(),
                mw,
                mh,
            ],
        })
    }
    fn x(self, x: f64) -> f64 {
        self.die[0] + (x - self.bbox[0]) * self.scale
    }
    fn y(self, y: f64) -> f64 {
        self.die[1] + (self.bbox[3] - y) * self.scale
    }
    fn point(self, p: [f64; 2]) -> Option<[f64; 2]> {
        let [x, y, w, h] = self.die;
        (p[0] >= x && p[0] <= x + w - 1. && p[1] >= y && p[1] <= y + h - 1.).then(|| {
            [
                self.bbox[0] + (p[0] - x) / self.scale,
                self.bbox[3] - (p[1] - y) / self.scale,
            ]
        })
    }
}
/// Clipped palette rectangles, all integers in the overview's own pixel grid.
pub type Rect = [u16; 5];
fn fill(out: &mut Vec<Rect>, x: f64, y: f64, w: f64, h: f64, color: u16) {
    let (x, y, w, h) = (
        x.round_ties_even(),
        y.round_ties_even(),
        w.round_ties_even(),
        h.round_ties_even(),
    );
    // Clip in f64 before integer conversion: an off-die viewport can project
    // far outside i64 even though its world coordinates are supported.
    let (a, b, c, d) = (
        x.max(0.),
        y.max(0.),
        (x + w).min(SIZE as f64),
        (y + h).min(SIZE as f64),
    );
    if c > a && d > b {
        out.push([a as u16, b as u16, (c - a) as u16, (d - b) as u16, color]);
    }
}
fn outline(out: &mut Vec<Rect>, x: f64, y: f64, w: f64, h: f64, color: u16) {
    fill(out, x, y, w, 1., color);
    fill(out, x, y + h - 1., w, 1., color);
    fill(out, x, y, 1., h, color);
    fill(out, x + w - 1., y, 1., h, color);
}
fn paint(pixels: &mut [u8], rows: &[Rect]) {
    for &[x, y, w, h, color] in rows {
        for row in usize::from(y)..usize::from(y + h) {
            pixels[row * SIZE + usize::from(x)..row * SIZE + usize::from(x + w)]
                .fill(b'0' + color as u8);
        }
    }
}
#[derive(Debug, Serialize)]
pub struct Projection {
    pub size: usize,
    pub base: String,
    pub die: Option<[u16; 4]>,
    pub marks: Vec<Rect>,
}
impl Minimap {
    pub fn shared(layout: &catalog::Layout) -> Result<std::sync::Arc<Self>> {
        if let Some(m) = layout.minimap.get() {
            return Ok(std::sync::Arc::clone(m));
        }
        let m = std::sync::Arc::new(Self::load(layout)?);
        Ok(std::sync::Arc::clone(layout.minimap.get_or_init(|| m)))
    }
    pub fn plain(bbox: [f64; 4]) -> Self {
        Self::build(bbox, None).expect("plain overview has no malformed rows")
    }
    fn load(layout: &catalog::Layout) -> Result<Self> {
        let f = catalog::regular_file(&layout.directory.join("meta.json"))?;
        if f.metadata()?.len() > META_BYTES {
            return Err(Error::new(ErrorKind::Cache, "overview metadata limit"));
        }
        let m: Header = serde_json::from_reader(BufReader::new(f.take(META_BYTES + 1)))
            .map_err(|_| Error::new(ErrorKind::Cache, "invalid overview metadata"))?;
        let original = &layout.metadata;
        if m.bbox != original.bbox
            || m.dbu != original.dbu
            || m.src.size != original.src.size
            || m.src.mtime != original.src.mtime
        {
            return Err(Error::new(
                ErrorKind::Cache,
                "overview metadata changed during open",
            ));
        }
        Self::build(m.bbox.map(|v| v as f64), m.frontier)
    }
    fn build(bbox: [f64; 4], frontier: Option<Frontier>) -> Result<Self> {
        let mut pixels = vec![b'0'; SIZE * SIZE];
        let geometry = Geometry::new(bbox);
        if let Some(g) = geometry {
            let [x, y, w, h] = g.die;
            let mut ops = Vec::new();
            fill(&mut ops, x, y, w, h, 1);
            outline(&mut ops, x, y, w, h, 2);
            paint(&mut pixels, &ops);
        }
        let plain = String::from_utf8(pixels).expect("palette is ASCII");
        let mut bases = Vec::new();
        if let Some(f) = frontier {
            for rows in f.depths.0 {
                let mut pixels = plain.as_bytes().to_vec();
                for row in rows.0 {
                    let r = row.0;
                    if r.len() < 4
                        || !r
                            .iter()
                            .all(|v| v.is_finite() && v.abs() <= (1u64 << 62) as f64)
                        || r[2] < r[0]
                        || r[3] < r[1]
                    {
                        return Err(Error::new(ErrorKind::Cache, "invalid overview box"));
                    }
                    if let Some(g) = geometry {
                        let (w, h) = ((r[2] - r[0]) * g.scale, (r[3] - r[1]) * g.scale);
                        if w < 0.7 && h < 0.7 {
                            continue;
                        }
                        let mut ops = Vec::with_capacity(4);
                        outline(
                            &mut ops,
                            g.x(r[0]),
                            g.y(r[3]),
                            w.round_ties_even().max(1.),
                            h.round_ties_even().max(1.),
                            3,
                        );
                        paint(&mut pixels, &ops);
                    }
                }
                bases.push(String::from_utf8(pixels).expect("palette is ASCII"));
            }
        }
        Ok(Self { plain, bases })
    }
    pub fn base(&self, key: &str) -> Option<&str> {
        if key == "full" {
            return Some(&self.plain);
        }
        let n = key.parse::<usize>().ok()?;
        (n.to_string() == key)
            .then(|| self.bases.get(n))
            .flatten()
            .map(String::as_str)
    }
    pub fn projection(&self, bbox: [f64; 4], v: Viewport, depth: Option<u32>) -> Projection {
        let base = depth
            .filter(|&d| d < 999 && (d as usize) < self.bases.len())
            .map_or("full".into(), |d| d.to_string());
        let mut out = Projection {
            size: SIZE,
            base,
            die: None,
            marks: Vec::new(),
        };
        if let Some(g) = Geometry::new(bbox) {
            let [x, y, w, h] = g.die;
            out.die = Some(g.die.map(|v| v as u16));
            let [a, b, c, d] = v.bbox;
            if (c - a) * g.scale >= 6. && (d - b) * g.scale >= 6. {
                let (x0, y0, x1, y1) = (
                    x.max(g.x(a)),
                    y.max(g.y(d)),
                    (x + w - 1.).min(g.x(c)),
                    (y + h - 1.).min(g.y(b)),
                );
                // Unlike the old GTK helper, do not draw a stray border when
                // the viewport is entirely outside the die.
                if x1 >= x0 && y1 >= y0 {
                    outline(
                        &mut out.marks,
                        x0,
                        y0,
                        (x1 - x0 + 1.).round_ties_even().max(2.),
                        (y1 - y0 + 1.).round_ties_even().max(2.),
                        4,
                    );
                }
            } else {
                let (px, py) = (g.x(a + (c - a) / 2.), g.y(b + (d - b) / 2.));
                fill(&mut out.marks, px - 3., py - 3., 7., 7., 0);
                fill(&mut out.marks, px - 2., py - 2., 5., 5., 4);
            }
        }
        out
    }
}
pub(super) fn navigate(v: Viewport, bbox: [f64; 4], p: [f64; 2]) -> Result<Viewport> {
    if !p
        .iter()
        .all(|x| x.is_finite() && (0. ..=SIZE as f64).contains(x))
    {
        return Err(Error::input("invalid overview point"));
    }
    let Some(target) = Geometry::new(bbox).and_then(|g| g.point(p)) else {
        return Ok(v);
    };
    let [x0, y0, x1, y1] = v.bbox;
    let step = 16. * (x1 - x0) / f64::from(v.width);
    let center = [x0 + (x1 - x0) / 2., y0 + (y1 - y0) / 2.];
    let shift = [0, 1].map(|i| ((target[i] - center[i]) / step).round_ties_even() * step);
    Viewport::new(
        [x0 + shift[0], y0 + shift[1], x1 + shift[0], y1 + shift[1]],
        v.width,
        v.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    #[test]
    fn baked_depths_plain_fallback_and_bounded_metadata() {
        let bbox = [-100., -200., 900., 800.];
        let f = serde_json::from_value(json!({"depths":[[[0,0,500,500,1]],[]]})).unwrap();
        let m = Minimap::build(bbox, Some(f)).unwrap();
        let v = Viewport::new(bbox, 1000, 1000).unwrap();
        assert_eq!(m.base("full").unwrap().len(), SIZE * SIZE);
        assert_ne!(m.base("0"), m.base("full"));
        assert_eq!(m.base("1"), m.base("full"));
        for depth in [None, Some(2), Some(999)] {
            assert_eq!(m.projection(bbox, v, depth).base, "full");
        }
        assert_eq!(m.projection(bbox, v, Some(0)).base, "0");
        for key in ["-1", "00", "32", "1/../../meta.json"] {
            assert!(m.base(key).is_none());
        }
        assert!(serde_json::from_value::<Frontier>(
            json!({"depths":vec![Vec::<Vec<i64>>::new();33]})
        )
        .is_err());
        assert!(
            serde_json::from_value::<Frontier>(json!({"depths":[vec![vec![0,0,1,1];6001]]}))
                .is_err()
        );
        for r in [
            json!([0, 0, 1]),
            json!([0, 0, 1, 1, 0, 0]),
            json!([1, 0, 0, 1]),
            json!([0, 0, 1e100, 1]),
        ] {
            let parsed = serde_json::from_value::<Frontier>(json!({"depths":[[r]]}));
            assert!(parsed.is_err() || Minimap::build(bbox, Some(parsed.unwrap())).is_err());
        }
        let plain = Minimap::default();
        assert!(plain.base("full").unwrap().bytes().all(|c| c == b'0'));
    }
    #[test]
    fn overview_click_preserves_scale_and_snaps_relative_to_current_phase() {
        let bbox = [0., 0., 1000., 1000.];
        let v = Viewport::new([-10.9375, 0., 89.0625, 80.], 100, 80).unwrap();
        let after = navigate(v, bbox, [90., 90.]).unwrap();
        assert_eq!(after.bbox, [453.0625, 464., 553.0625, 544.]);
        assert_eq!(after.bbox[2] - after.bbox[0], 100.);
        assert_eq!(after.bbox[3] - after.bbox[1], 80.);
        for p in [[f64::NAN, 0.], [f64::INFINITY, 0.], [-1., 0.], [181., 90.]] {
            assert!(navigate(v, bbox, p).is_err());
        }
        assert_eq!(
            navigate(v, [0., 0., 1000., 100.], [0., 0.]).unwrap(),
            v,
            "letterbox click"
        );
        let m = Minimap::plain(bbox);
        let far = Viewport::new([1e18, 1e18, 1e18 + 1024., 1e18 + 1024.], 1024, 1024).unwrap();
        assert!(m.projection(bbox, far, None).marks.is_empty());
    }
    #[test]
    #[ignore = "run tools/validate_minimap.py with actual GTK pixel/navigation cases"]
    fn gtk_overview_pixels_and_snapped_navigation_match() {
        let p = std::env::var_os("FLOE_MINIMAP_CASES").expect("private oracle input");
        let cases: Vec<Value> = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        assert!(cases.len() >= 100);
        let array = |v: &Value| std::array::from_fn::<_, 4, _>(|i| v[i].as_f64().unwrap());
        for (i, c) in cases.iter().enumerate() {
            let bbox = array(&c["bbox"]);
            let depth = c["depth"].as_u64().map(|n| n as u32);
            let m = Minimap::build(bbox, serde_json::from_value(c["frontier"].clone()).unwrap())
                .unwrap();
            let v = Viewport::new(array(&c["view"]), 800, 600).unwrap();
            let projected = m.projection(bbox, v, depth);
            let base = m.base(&projected.base).unwrap();
            assert_eq!(base, c["base"].as_str().unwrap(), "base case {i}");
            let mut pixels = base.as_bytes().to_vec();
            paint(&mut pixels, &projected.marks);
            assert_eq!(
                String::from_utf8(pixels).unwrap(),
                c["pixels"].as_str().unwrap(),
                "pixels case {i}"
            );
            let p = [
                c["point"][0].as_f64().unwrap(),
                c["point"][1].as_f64().unwrap(),
            ];
            let after = navigate(v, bbox, p).unwrap();
            let want = array(&c["after"]);
            let tol = 64.
                * f64::EPSILON
                * bbox
                    .iter()
                    .chain(want.iter())
                    .fold(1_f64, |a, x| a.max(x.abs()));
            for (got, expected) in after.bbox.iter().zip(want) {
                assert!(
                    (got - expected).abs() <= tol,
                    "click case {i}: {after:?}, {want:?}"
                );
            }
        }
        println!(
            "MINIMAP GTK/RUST: ALL OK ({} cases, base/overlay pixels + snapped navigation)",
            cases.len()
        );
    }
}
