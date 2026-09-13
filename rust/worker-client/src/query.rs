//! Typed, scene-pinned local queries. No filesystem or browser authority here.
use crate::{Error, Fields, Layers, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneId {
    pub generation: u64,
    pub round: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryScene {
    pub id: Option<SceneId>,
    pub complete: bool,
    pub summary_layers: u64,
}
impl QueryScene {
    pub fn queryable(self) -> bool {
        self.id.is_some() && self.complete && self.summary_layers == 0
    }
    pub(crate) fn parse(f: &Fields) -> Result<Self> {
        let generation = counter(f, "scene_gen")?;
        let round = counter(f, "scene_round")?;
        let complete = f.flag("scene_complete")?;
        let summary_layers = f.u64("scene_summary")?;
        if (generation == 0) != (round == 0)
            || (generation == 0 && (complete || summary_layers != 0))
        {
            return Err(Error::protocol("invalid query scene metadata"));
        }
        Ok(Self {
            id: (generation != 0).then_some(SceneId { generation, round }),
            complete,
            summary_layers,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryOperation {
    Snap,
    Pick { nth: i64 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryKind {
    Snap,
    Pick,
}
impl QueryKind {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Snap => "snap",
            Self::Pick => "pick",
        }
    }
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "snap" => Ok(Self::Snap),
            "pick" => Ok(Self::Pick),
            _ => Err(Error::protocol("invalid query kind")),
        }
    }
}
impl QueryOperation {
    pub fn kind(self) -> QueryKind {
        match self {
            Self::Snap => QueryKind::Snap,
            Self::Pick { .. } => QueryKind::Pick,
        }
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Snap => "snap",
            Self::Pick { .. } => "pick",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryRequest {
    pub scene: SceneId,
    pub operation: QueryOperation,
    pub x: i64,
    pub y: i64,
    pub radius: i64,
    pub layers: Layers,
}
impl QueryRequest {
    pub(crate) fn command(&self, sequence: u64) -> Result<String> {
        if sequence == 0
            || sequence > i64::MAX as u64
            || self.scene.generation == 0
            || self.scene.round == 0
            || self.radius < 0
        {
            return Err(Error::input("invalid query sequence/scene/radius"));
        }
        let nth = match self.operation {
            QueryOperation::Snap => String::new(),
            QueryOperation::Pick { nth } => format!(" nth={nth}"),
        };
        Ok(format!(
            "{} seq={sequence} x={} y={} r={} layers={} scene_gen={} scene_round={}{nth}",
            self.operation.name(),
            self.x,
            self.y,
            self.radius,
            self.layers.wire()?,
            self.scene.generation,
            self.scene.round
        ))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryStatus {
    Ok,
    Unavailable,
    Mismatch,
    Incomplete,
    Summary,
    Superseded,
    Error,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    Vertex,
    Edge,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapHit {
    pub x: i64,
    pub y: i64,
    pub kind: SnapKind,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PickHit {
    pub count: u64,
    pub index: u64,
    pub layer: (u32, u32),
    pub layer_name: String,
    pub cell_name: String,
    pub area: f64,
    pub bbox: [i64; 4],
    pub points: Vec<(i64, i64)>,
    pub points_truncated: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub enum QueryHit {
    Snap(SnapHit),
    Pick(PickHit),
}
#[derive(Clone, Debug, PartialEq)]
pub struct QueryReply {
    pub sequence: u64,
    pub request: QueryRequest,
    /// Actual immutable scene captured by the query thread, not a later read.
    pub scene: QueryScene,
    pub status: QueryStatus,
    /// Summarized layers intersecting this request, not all displayed layers.
    pub summary_layers: u64,
    pub hit: Option<QueryHit>,
    /// Native local diagnostic. A future HTTP adapter must map to safe codes.
    pub error: Option<String>,
}
fn int(s: &str) -> Result<i64> {
    s.parse()
        .map_err(|_| Error::protocol("invalid query coordinate"))
}
pub(crate) fn counter(fields: &Fields, key: &str) -> Result<u64> {
    let n = fields.u64(key)?;
    if fields.required(key)? != n.to_string() {
        return Err(Error::protocol("noncanonical query counter"));
    }
    Ok(n)
}
fn unhex(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) || !bytes.is_ascii() {
        return Err(Error::protocol("invalid query hex"));
    }
    let nibble = |b| match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(Error::protocol("invalid query hex")),
    };
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        out.push(nibble(pair[0])? * 16 + nibble(pair[1])?);
    }
    String::from_utf8(out).map_err(|_| Error::protocol("non-UTF8 query text"))
}
pub(crate) fn parse_reply(kind: &str, f: Fields, request: QueryRequest) -> Result<QueryReply> {
    if kind != request.operation.name() {
        return Err(Error::protocol("query kind mismatch"));
    }
    let sequence = counter(&f, "seq")?;
    if sequence == 0 || sequence > i64::MAX as u64 {
        return Err(Error::protocol("invalid query sequence"));
    }
    let scene = QueryScene::parse(&f)?;
    let summary_layers = counter(&f, "query_summary")?;
    if summary_layers > scene.summary_layers {
        return Err(Error::protocol("invalid query summary count"));
    }
    let status = match f.required("query_status")? {
        "ok" => QueryStatus::Ok,
        "scene_unavailable" => QueryStatus::Unavailable,
        "scene_mismatch" => QueryStatus::Mismatch,
        "scene_incomplete" => QueryStatus::Incomplete,
        "scene_summary" => QueryStatus::Summary,
        "superseded" => QueryStatus::Superseded,
        "error" => QueryStatus::Error,
        _ => return Err(Error::protocol("invalid query status")),
    };
    let found = f.flag("found")?;
    if found && request.layers == Layers::None {
        return Err(Error::protocol("query hit with no layers"));
    }
    let matches = scene.id == Some(request.scene);
    let valid = match status {
        QueryStatus::Ok => matches && scene.complete && summary_layers == 0,
        QueryStatus::Unavailable => scene.id.is_none(),
        QueryStatus::Mismatch => scene.id.is_some() && !matches,
        QueryStatus::Incomplete => matches && !scene.complete,
        QueryStatus::Summary => matches && scene.complete && summary_layers != 0,
        QueryStatus::Superseded => matches && scene.complete && summary_layers == 0,
        QueryStatus::Error => scene.id.is_none() || (matches && scene.complete),
    };
    let error = f.get("err_hex").map(unhex).transpose()?;
    if matches!(
        status,
        QueryStatus::Ok | QueryStatus::Summary | QueryStatus::Superseded
    ) && match &request.layers {
        Layers::All => scene.summary_layers != summary_layers,
        Layers::None => summary_layers != 0,
        Layers::Only(pairs) => summary_layers > pairs.len() as u64,
    } {
        return Err(Error::protocol("query summary selection mismatch"));
    }
    if !valid
        || (status != QueryStatus::Ok && (found || error.is_none()))
        || (status == QueryStatus::Ok && error.is_some())
    {
        return Err(Error::protocol("query result/context mismatch"));
    }
    let hit = if kind == "snap" {
        let (x, y) = (int(f.required("x")?)?, int(f.required("y")?)?);
        match (found, f.required("snap")?) {
            (true, "vertex") => Some(QueryHit::Snap(SnapHit {
                x,
                y,
                kind: SnapKind::Vertex,
            })),
            (true, "edge") => Some(QueryHit::Snap(SnapHit {
                x,
                y,
                kind: SnapKind::Edge,
            })),
            (false, "-") if (x, y) == (request.x, request.y) => None,
            _ => return Err(Error::protocol("invalid snap result")),
        }
    } else {
        let count = f.u64("count")?;
        if !found {
            if count != 0 {
                return Err(Error::protocol("nonzero empty pick count"));
            }
            None
        } else {
            let index = f.u64("index")?;
            if !(1..=64).contains(&count) || index >= count {
                return Err(Error::protocol("invalid pick count/index"));
            }
            if !matches!(request.operation,QueryOperation::Pick{nth} if nth.rem_euclid(count as i64) as u64==index)
            {
                return Err(Error::protocol("pick overlap index mismatch"));
            }
            let bbox = f
                .required("bbox")?
                .split(',')
                .map(int)
                .collect::<Result<Vec<_>>>()?;
            let bbox: [i64; 4] = bbox
                .try_into()
                .map_err(|_| Error::protocol("invalid pick bbox"))?;
            if bbox[0] >= bbox[2] || bbox[1] >= bbox[3] {
                return Err(Error::protocol("invalid pick bbox"));
            }
            let mut points = Vec::new();
            for point in f.required("points")?.split(';') {
                if points.len() == 512 {
                    return Err(Error::protocol("pick points limit"));
                }
                let (x, y) = point
                    .split_once(',')
                    .ok_or_else(|| Error::protocol("invalid pick point"))?;
                let (x, y) = (int(x)?, int(y)?);
                if x < bbox[0] || x > bbox[2] || y < bbox[1] || y > bbox[3] {
                    return Err(Error::protocol("pick point outside bbox"));
                }
                points.push((x, y));
            }
            let truncated = f.flag("points_truncated")?;
            if points.len() < 3 || (truncated && points.len() != 512) {
                return Err(Error::protocol("invalid pick outline"));
            }
            let area: f64 = f
                .required("area")?
                .parse()
                .map_err(|_| Error::protocol("invalid pick area"))?;
            if !area.is_finite() || area < 0. {
                return Err(Error::protocol("invalid pick area"));
            }
            let layer = u32::try_from(f.u64("layer")?)
                .map_err(|_| Error::protocol("invalid pick layer"))?;
            let datatype = u32::try_from(f.u64("datatype")?)
                .map_err(|_| Error::protocol("invalid pick datatype"))?;
            if matches!(&request.layers,Layers::Only(pairs) if !pairs.contains(&(layer,datatype))) {
                return Err(Error::protocol("pick hit outside requested layers"));
            }
            Some(QueryHit::Pick(PickHit {
                count,
                index,
                layer: (layer, datatype),
                layer_name: unhex(f.required("lname_hex")?)?,
                cell_name: unhex(f.required("cell_hex")?)?,
                area,
                bbox,
                points,
                points_truncated: truncated,
            }))
        }
    };
    Ok(QueryReply {
        sequence,
        request,
        scene,
        status,
        summary_layers,
        hit,
        error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> QueryRequest {
        QueryRequest {
            scene: SceneId {
                generation: 7,
                round: 2,
            },
            operation: QueryOperation::Snap,
            x: 1,
            y: 2,
            radius: 3,
            layers: Layers::Only(vec![(7, 0), (3, 300), (7, 0)]),
        }
    }
    fn fields(text: &str) -> Fields {
        let mut f = crate::protocol::parse_line(text.as_bytes()).unwrap().fields;
        let count = if f.get("query_status") == Some("scene_summary") {
            f.get("scene_summary").unwrap_or("0")
        } else {
            "0"
        };
        f.0.insert("query_summary".into(), count.into());
        f
    }
    fn snap() -> Fields {
        fields("snap seq=5 found=1 x=0 y=0 snap=vertex scene_gen=7 scene_round=2 scene_complete=1 scene_summary=0 query_status=ok")
    }
    #[test]
    fn request_is_numeric_bounded_and_canonical() {
        let mut r = request();
        assert_eq!(
            r.command(5).unwrap(),
            "snap seq=5 x=1 y=2 r=3 layers=3/300,7/0 scene_gen=7 scene_round=2"
        );
        r.operation = QueryOperation::Pick { nth: i64::MIN };
        assert!(r
            .command(i64::MAX as u64)
            .unwrap()
            .ends_with("nth=-9223372036854775808"));
        assert!(r.command(i64::MAX as u64 + 1).is_err());
        assert!(r.command(0).is_err());
        r.layers = Layers::Only(vec![]);
        assert!(r.command(1).is_err());
        r.layers = Layers::All;
        r.radius = -1;
        assert!(r.command(1).is_err());
        r.radius = 0;
        r.scene.round = 0;
        assert!(r.command(1).is_err());
    }
    #[test]
    fn snap_scene_and_kind_are_strict_not_just_sequence() {
        let r = parse_reply("snap", snap(), request()).unwrap();
        assert_eq!(
            r.hit,
            Some(QueryHit::Snap(SnapHit {
                x: 0,
                y: 0,
                kind: SnapKind::Vertex
            }))
        );
        assert!(r.scene.queryable());
        assert_eq!(r.sequence, 5);
        for (key, bad) in [
            ("scene_gen", "6"),
            ("scene_gen", "07"),
            ("seq", "05"),
            ("scene_round", "1"),
            ("scene_complete", "0"),
            ("query_summary", "1"),
            ("found", "2"),
            ("snap", "face"),
            ("x", "9223372036854775808"),
            ("query_status", "maybe"),
            ("err_hex", "ff"),
        ] {
            let mut f = snap();
            f.0.insert(key.into(), bad.into());
            assert!(parse_reply("snap", f, request()).is_err(), "{key}");
        }
        assert!(parse_reply("pick", snap(), request()).is_err());
        let mut f = snap();
        f.0.remove("scene_round");
        assert!(QueryScene::parse(&f).is_err());
        for bad in ["한글", "gg", "0", "ff"] {
            assert!(unhex(bad).is_err());
        }
        assert_eq!(unhex("544f5020ed959ceab880").unwrap(), "TOP 한글");
    }
    #[test]
    fn refusals_are_not_successful_empty_results() {
        for (wire, status, generation, round, complete, summary) in [
            ("scene_unavailable", QueryStatus::Unavailable, 0, 0, 0, 0),
            ("scene_mismatch", QueryStatus::Mismatch, 8, 1, 1, 0),
            ("scene_incomplete", QueryStatus::Incomplete, 7, 2, 0, 0),
            ("scene_summary", QueryStatus::Summary, 7, 2, 1, 2),
            ("superseded", QueryStatus::Superseded, 7, 2, 1, 0),
            ("error", QueryStatus::Error, 7, 2, 1, 0),
        ] {
            let f=fields(&format!("snap seq=5 found=0 x=1 y=2 snap=- scene_gen={generation} scene_round={round} scene_complete={complete} scene_summary={summary} query_status={wire} err_hex=6572726f72"));
            let r = parse_reply("snap", f.clone(), request()).unwrap();
            assert_eq!(r.status, status);
            assert!(r.hit.is_none());
            let mut bad = f;
            bad.0.insert("found".into(), "1".into());
            assert!(parse_reply("snap", bad, request()).is_err());
        }
        let mut f = snap();
        f.0.insert("scene_gen".into(), "0".into());
        assert!(QueryScene::parse(&f).is_err());
        let mut f = snap();
        f.0.insert("scene_summary".into(), "1".into());
        assert_eq!(
            parse_reply("snap", f.clone(), request()).unwrap().status,
            QueryStatus::Ok
        );
        let mut all = request();
        all.layers = Layers::All;
        assert!(
            parse_reply("snap", f, all).is_err(),
            "all-layer query must not hide summarized layers"
        );
    }
    #[test]
    fn pick_outline_size_text_and_extreme_integers_are_checked() {
        let mut req = request();
        req.operation = QueryOperation::Pick { nth: 1 };
        req.layers = Layers::All;
        let good=fields("pick seq=5 found=1 count=2 index=1 layer=4294967295 datatype=300 lname_hex=332f333030 cell_hex=544f5020ed959ceab880 area=100 bbox=-9223372036854775808,0,10,10 points=-9223372036854775808,0;0,10;10,0 points_truncated=0 scene_gen=7 scene_round=2 scene_complete=1 scene_summary=0 query_status=ok");
        let QueryHit::Pick(p) = parse_reply("pick", good.clone(), req.clone())
            .unwrap()
            .hit
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(p.cell_name, "TOP 한글");
        assert_eq!(p.bbox[0], i64::MIN);
        for (key, bad) in [
            ("count", "65"),
            ("index", "2"),
            ("layer", "4294967296"),
            ("area", "NaN"),
            ("bbox", "0,0,0,1"),
            ("points", "0,0;0,1"),
            ("points_truncated", "1"),
            ("cell_hex", "한글"),
        ] {
            let mut f = good.clone();
            f.0.insert(key.into(), bad.into());
            assert!(parse_reply("pick", f, req.clone()).is_err(), "{key}");
        }
        let mut f = good.clone();
        f.0.insert("points".into(), vec!["0,0"; 513].join(";"));
        assert!(parse_reply("pick", f, req.clone()).is_err());
        let mut f = good;
        f.0.insert("points".into(), vec!["0,0"; 512].join(";"));
        f.0.insert("points_truncated".into(), "1".into());
        assert!(matches!(
            parse_reply("pick", f, req).unwrap().hit,
            Some(QueryHit::Pick(PickHit {
                points_truncated: true,
                ..
            }))
        ));
    }
}
