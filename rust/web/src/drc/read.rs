use super::{dto::Command, RESPONSE_BYTES};
use floe_app_core::{
    check_cancelled,
    drc::{Cursor, Hit, Pack},
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
    Ok(
        json!({"check":h.check.to_string(),"local":h.local.to_string(),"global":h.violation.number.to_string(),
        "kind":h.violation.kind.to_string(),"status":h.status,"bbox_um":bounds(p,Some(h.violation.bbox))?,"points":h.violation.points.len().to_string()}),
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
            let v = p.error(check, error, stop)?;
            if start > v.points.len() {
                return Err(Error::input("point cursor"));
            }
            let end = (start + limit).min(v.points.len());
            let points = v.points[start..end]
                .iter()
                .map(|xy| xy.map(|v| v.to_string()))
                .collect::<Vec<_>>();
            json!({"check":check.to_string(),"local":error.to_string(),"global":v.number.to_string(),"kind":v.kind.to_string(),
                "status":p.status(check,error)?,"bbox_um":bounds(p,Some(v.bbox))?,"precision":p.precision.to_string(),
                "points_dbu":points,"start":start.to_string(),"total":v.points.len().to_string(),
                "next":if end<v.points.len(){Some(end.to_string())}else{None}})
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
