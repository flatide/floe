use super::{dto::Command, RESPONSE_BYTES};
use floe_app_core::{
    check_cancelled,
    drc::{cd_segments, Cursor, Hit, InfoHit, ListRequest, Pack, RecordInfo},
    Error, ErrorKind, Result,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    io::{self, Write},
    sync::atomic::AtomicUsize,
};

struct Output(Vec<u8>);
impl Write for Output {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if b.len() > RESPONSE_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("DRC response limit"));
        }
        self.0.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn short(s: &str) -> (String, bool) {
    (s.chars().take(256).collect(), s.chars().count() > 256)
}
fn bounds(p: &Pack, b: Option<[i64; 4]>) -> Result<Value> {
    Ok(match b {
        Some(b) => json!(p.bbox_um(b)?.map(|v| v.to_string())),
        None => Value::Null,
    })
}
fn hit(p: &Pack, h: &Hit) -> Result<Value> {
    info_hit(
        p,
        &InfoHit {
            check: h.check,
            local: h.local,
            status: h.status,
            record: RecordInfo::from(&h.violation),
        },
    )
}
fn info_hit(p: &Pack, h: &InfoHit) -> Result<Value> {
    Ok(
        json!({"check":h.check.to_string(),"local":h.local.to_string(),"global":h.record.number.to_string(),
        "kind":h.record.kind.to_string(),"status":h.status,"bbox_um":bounds(p,Some(h.record.bbox))?,"points":h.record.points.to_string()}),
    )
}
fn next(c: Option<Cursor>) -> Value {
    c.map_or(
        Value::Null,
        |c| json!({"check":c.check.to_string(),"error":c.error.to_string()}),
    )
}
pub(super) fn execute(p: &mut Pack, request: Command, stop: &AtomicUsize) -> Result<Vec<u8>> {
    check_cancelled(stop)?;
    p.unchanged()?;
    let value = match request {
        Command::List {
            check,
            start,
            waived,
            limit,
            filters,
        } => {
            let b = filters.bounds()?;
            let page = p.filtered_errors(
                ListRequest {
                    check,
                    start,
                    waived,
                    limit,
                    bbox_um: b,
                    selected: filters.selection()?,
                },
                stop,
            )?;
            json!({"rows":page.hits.iter().map(|h| info_hit(p,h)).collect::<Result<Vec<_>>>()?, "next":page.next.map(|n|n.to_string()),
                "scanned":page.scanned.to_string(), "bbox_um":b.map(|b|b.map(|n|n.to_string())), "selection_rev":filters.selection_rev.map(|n|n.to_string())})
        }
        Command::FilteredStep {
            mut request,
            filters,
        } => {
            request.bbox_um = filters.bounds()?;
            let page = p.filtered_step(request, filters.selection()?, stop)?;
            json!({"hit":page.hit.as_ref().map(|h|info_hit(p,h)).transpose()?,"next":page.next.map(|c|json!({"next":c.next.to_string(),"remaining":c.remaining.to_string()})),
                "scanned":page.scanned.to_string(),"bbox_um":request.bbox_um.map(|b|b.map(|n|n.to_string())),"selection_rev":filters.selection_rev.map(|n|n.to_string())})
        }
        Command::SelectionCandidates {
            check,
            errors,
            bbox_um,
            waived,
        } => {
            let hits = check
                .map(|ci| p.selection_candidates(ci, &errors, bbox_um, waived, stop))
                .transpose()?
                .unwrap_or_default();
            json!({"rows": hits.iter().map(|h| info_hit(p, h)).collect::<Result<Vec<_>>>()?})
        }
        Command::ValidatePanel(data) => {
            data.validate_pack(p)?;
            json!({})
        }
        Command::Step(request) => {
            let page = p.step(request, stop)?;
            json!({"hit":page.hit.as_ref().map(|h| info_hit(p,h)).transpose()?,
                "next":page.next.map(|c|json!({"next":c.next.to_string(),"remaining":c.remaining.to_string()})),
                "scanned":page.scanned.to_string()})
        }
        Command::Rules {
            start,
            search,
            limit,
        } => {
            if start > p.checks.len() {
                return Err(Error::input("rule cursor"));
            }
            let mut rows = Vec::new();
            let mut end = start;
            // Bound scanned NAME bytes as well as rule count (a single name
            // may be 1 MiB). Empty+next is incomplete, never an empty full list.
            let mut bytes = 0usize;
            while end < p.checks.len()
                && end - start < 4096
                && rows.len() < limit
                && bytes < 1024 * 1024
            {
                check_cancelled(stop)?;
                let c = &p.checks[end];
                bytes += c.name.len();
                if search.is_empty() || c.name.to_lowercase().contains(&search) {
                    let (name, truncated) = short(&c.name);
                    rows.push(json!({"check":end.to_string(),"name":name,"name_truncated":truncated,
                        "errors":c.count.to_string(),"waived":p.waived_count(end)?.to_string(),"bbox_um":bounds(p,c.bbox)?}));
                }
                end += 1;
            }
            json!({"rows":rows,"next":if end<p.checks.len(){Some(end.to_string())}else{None},"scanned":(end-start).to_string()})
        }
        Command::Rule { check } => {
            let c = p
                .checks
                .get(check)
                .ok_or_else(|| Error::input("rule index"))?;
            if c.name.len().saturating_add(c.desc.len()) > RESPONSE_BYTES {
                return Err(Error::new(
                    ErrorKind::Incomplete,
                    "DRC rule text exceeds transport limit",
                ));
            }
            json!({"check":check.to_string(),"name":c.name,"description":c.desc,"errors":c.count.to_string(),
                "declared":c.declared.to_string(),"original":c.original.to_string(),"waived":p.waived_count(check)?.to_string(),"bbox_um":bounds(p,c.bbox)?})
        }
        Command::Errors {
            check,
            start,
            waived,
            limit,
        } => {
            let c = p
                .checks
                .get(check)
                .ok_or_else(|| Error::input("rule index"))?;
            if start > c.count {
                return Err(Error::input("error cursor"));
            }
            if let Some(b) = c.bbox {
                let page = p.query(
                    p.bbox_um(b)?,
                    Some(&BTreeSet::from([check])),
                    waived,
                    Cursor {
                        check,
                        error: start,
                    },
                    limit,
                    stop,
                )?;
                let rows = page
                    .hits
                    .iter()
                    .map(|h| hit(p, h))
                    .collect::<Result<Vec<_>>>()?;
                json!({"rows":rows,"next":page.next.filter(|c|c.check==check).map(|c|c.error.to_string()),"scanned":page.scanned.to_string()})
            } else {
                json!({"rows":[],"next":null,"scanned":"0"})
            }
        }
        Command::Query {
            bbox_um,
            checks,
            waived,
            cursor,
            limit,
        } => {
            let page = p.query(bbox_um, checks.as_ref(), waived, cursor, limit, stop)?;
            let rows = page
                .hits
                .iter()
                .map(|h| hit(p, h))
                .collect::<Result<Vec<_>>>()?;
            json!({"rows":rows,"next":next(page.next),"scanned":page.scanned.to_string()})
        }
        Command::Geometry {
            check,
            error,
            start,
            limit,
        } => {
            let page = p.error_points(check, error, start, limit, stop)?;
            let v = page.record;
            let points = page
                .points
                .iter()
                .map(|xy| xy.map(|v| v.to_string()))
                .collect::<Vec<_>>();
            json!({"check":check.to_string(),"local":error.to_string(),"global":v.number.to_string(),"kind":v.kind.to_string(),
                "status":p.status(check,error)?,"bbox_um":bounds(p,Some(v.bbox))?,"precision":p.precision.to_string(),
                "points_dbu":points,"start":page.start.to_string(),"total":v.points.to_string(),
                "next":page.next.map(|v|v.to_string())})
        }
        Command::Measurements { check, error } => {
            // CD supports at most four vertices. Validate/decode the containing
            // block as usual, but never clone a large selected polygon here.
            let page = p.error_points(check, error, 0, 4, stop)?;
            let segments = if page.next.is_none() {
                cd_segments(page.record.kind, &page.points, p.precision)?
            } else {
                Vec::new()
            };
            let segments: Vec<_> = segments
                .into_iter()
                .map(|s| {
                    json!({
                "endpoints_um":s.endpoints_um.map(|p|p.map(|v|v.to_string())),
                "distance_um":s.distance_um.to_string(),"offset":s.offset})
                })
                .collect();
            json!({"check":check.to_string(),"local":error.to_string(),
                "global":page.record.number.to_string(),"segments":segments})
        }
        Command::InView {
            waived,
            cursor,
            limit,
            context,
        } => {
            let c = context.ok_or_else(|| Error::input("query requires an authoritative view"))?;
            let b = c.bbox_dbu.map(|v| v * c.dbu);
            let page = p.query(b, None, waived, cursor, limit, stop)?;
            let rows = page
                .hits
                .iter()
                .map(|h| hit(p, h))
                .collect::<Result<Vec<_>>>()?;
            json!({"rows":rows,"next":next(page.next),"scanned":page.scanned.to_string(),"bbox_um":b.map(|v|v.to_string())})
        }
        Command::Focus {
            check,
            error,
            fit,
            context,
        } => {
            let c = context.ok_or_else(|| Error::input("focus requires an authoritative view"))?;
            let v = p.error_info(check, error, stop)?;
            let b = p.bbox_um(v.bbox)?;
            let mut width = (c.bbox_dbu[2] - c.bbox_dbu[0]) * c.dbu;
            if fit {
                width = ((b[2] - b[0]) / 0.3)
                    .max((b[3] - b[1]) / 0.3 * f64::from(c.pixels[0]) / f64::from(c.pixels[1]));
                if width <= 0. {
                    width = 0.1;
                }
            }
            let center = [b[0] * 0.5 + b[2] * 0.5, b[1] * 0.5 + b[3] * 0.5];
            if !width.is_finite() || width <= 0. || !center.iter().all(|v| v.is_finite()) {
                return Err(Error::input("unrepresentable DRC focus viewport"));
            }
            json!({"check":check.to_string(),"local":error.to_string(),"navigation":{
                "kind":"goto","center_um":center.map(|v|v.to_string()),"width_um":width.to_string()}})
        }
    };
    check_cancelled(stop)?;
    p.unchanged()?;
    let mut out = Output(Vec::new());
    serde_json::to_writer(&mut out, &value).map_err(|_| {
        Error::new(
            ErrorKind::Incomplete,
            "DRC response exceeds transport limit",
        )
    })?;
    check_cancelled(stop)?;
    Ok(out.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escaped_json_cannot_allocate_an_unbounded_response() {
        let mut out = Output(Vec::new());
        assert!(serde_json::to_writer(&mut out, &"\u{0001}".repeat(RESPONSE_BYTES / 4)).is_err());
        assert!(out.0.len() <= RESPONSE_BYTES);
        assert!(out.0.capacity() <= RESPONSE_BYTES);
        let (value, truncated) = short(&"한".repeat(257));
        assert_eq!(value.chars().count(), 256);
        assert!(truncated);
    }
}
