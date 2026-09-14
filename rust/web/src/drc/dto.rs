use super::Failure;
use floe_app_core::drc::{Cursor, StepCursor, StepRequest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CursorDto {
    pub check: String,
    pub error: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepCursorDto {
    pub next: String,
    pub remaining: String,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    List {
        check: String,
        start: String,
        waived: Option<bool>,
        limit: usize,
        in_view: bool,
        selection_rev: Option<String>,
    },
    FilteredStep {
        check: String,
        backwards: bool,
        after: Option<String>,
        cursor: Option<StepCursorDto>,
        waived: Option<bool>,
        in_view: bool,
        selection_rev: Option<String>,
    },
    Rules {
        start: String,
        search: String,
        limit: usize,
        metric: Option<String>,
        waived: Option<bool>,
    },
    Types {
        start: String,
        limit: usize,
    },
    Rule {
        check: String,
    },
    Errors {
        check: String,
        start: String,
        waived: Option<bool>,
        limit: usize,
    },
    Geometry {
        check: String,
        error: String,
        start: String,
        limit: usize,
    },
    Measurements {
        check: String,
        error: String,
    },
    Comparison {
        check: String,
        error: String,
    },
    Records {
        check: String,
        errors: Vec<String>,
    },
    Focus {
        check: String,
        error: String,
        fit: bool,
        #[serde(default)]
        isolate: bool,
    },
    Step {
        check: String,
        backwards: bool,
        after: Option<String>,
        cursor: Option<StepCursorDto>,
        waived: Option<bool>,
        bbox_um: Option<[String; 4]>,
    },
    InView {
        waived: Option<bool>,
        cursor: CursorDto,
        limit: usize,
    },
    Query {
        bbox_um: [String; 4],
        checks: Option<Vec<String>>,
        waived: Option<bool>,
        cursor: CursorDto,
        limit: usize,
    },
}
pub(super) enum Command {
    List {
        check: usize,
        start: u64,
        waived: Option<bool>,
        limit: usize,
        filters: Filters,
    },
    FilteredStep {
        request: StepRequest,
        filters: Filters,
    },
    ValidatePanel(Box<super::panel::Data>),
    // Internal only: wire requests cannot forge filesystem pack identities.
    ValidateReview(floe_app_core::drc::review::Identity),
    SelectionCandidates {
        check: Option<usize>,
        errors: Vec<u64>,
        bbox_um: Option<[f64; 4]>,
        waived: Option<bool>,
    },
    Step(StepRequest),
    Rules {
        start: usize,
        search: String,
        limit: usize,
        metric: Option<String>,
        waived: Option<bool>,
    },
    Types {
        start: usize,
        limit: usize,
    },
    Rule {
        check: usize,
    },
    Errors {
        check: usize,
        start: u64,
        waived: Option<bool>,
        limit: usize,
    },
    Geometry {
        check: usize,
        error: u64,
        start: usize,
        limit: usize,
    },
    Measurements {
        check: usize,
        error: u64,
    },
    Comparison {
        check: usize,
        error: u64,
    },
    Focus {
        check: usize,
        error: u64,
        fit: bool,
        context: Option<FocusContext>,
        isolate: bool,
        preparation: Option<super::focus::Preparation>,
    },
    InView {
        waived: Option<bool>,
        cursor: Cursor,
        limit: usize,
        context: Option<FocusContext>,
    },
    Query {
        bbox_um: [f64; 4],
        checks: Option<BTreeSet<usize>>,
        waived: Option<bool>,
        cursor: Cursor,
        limit: usize,
    },
}
pub(super) struct Filters {
    pub in_view: bool,
    pub selection_rev: Option<u64>,
    pub context: Option<FocusContext>,
    pub selected: Option<BTreeSet<u64>>,
}
#[derive(Clone, Copy)]
pub(super) struct FocusContext {
    pub bbox_dbu: [f64; 4],
    pub dbu: f64,
    pub pixels: [u32; 2],
}
pub(super) fn number(s: &str) -> Result<u64, Failure> {
    if s.len() > 20 {
        return Err("invalid_drc_request");
    }
    let n = s.parse::<u64>().map_err(|_| "invalid_drc_request")?;
    if n.to_string() != s {
        return Err("invalid_drc_request");
    }
    Ok(n)
}
pub(super) fn index(s: &str) -> Result<usize, Failure> {
    usize::try_from(number(s)?).map_err(|_| "invalid_drc_request")
}
fn cap(n: usize, max: usize) -> Result<usize, Failure> {
    if n == 0 || n > max {
        Err("invalid_drc_request")
    } else {
        Ok(n)
    }
}
pub(super) fn bbox(strings: [String; 4]) -> Result<[f64; 4], Failure> {
    let mut b = [0.; 4];
    for (out, s) in b.iter_mut().zip(strings) {
        if s.len() > 80 {
            return Err("invalid_drc_request");
        }
        *out = s.parse::<f64>().map_err(|_| "invalid_drc_request")?;
        if !out.is_finite() {
            return Err("invalid_drc_request");
        }
    }
    if b[0] > b[2] || b[1] > b[3] {
        return Err("invalid_drc_request");
    }
    Ok(b)
}
impl Request {
    pub(super) fn selection_filter(&self) -> Result<Option<(u64, usize)>, Failure> {
        let pair = match self {
            Self::List {
                check,
                selection_rev,
                ..
            }
            | Self::FilteredStep {
                check,
                selection_rev,
                ..
            } => (check, selection_rev),
            _ => return Ok(None),
        };
        pair.1
            .as_ref()
            .map(|rev| {
                Ok((
                    crate::view::counter(rev).map_err(|_| "invalid_drc_request")?,
                    index(pair.0)?,
                ))
            })
            .transpose()
    }
    pub(super) fn core(self) -> Result<Command, Failure> {
        Ok(match self {
            Self::List {
                check,
                start,
                waived,
                limit,
                in_view,
                selection_rev,
            } => Command::List {
                check: index(&check)?,
                start: number(&start)?,
                waived,
                limit: cap(limit, 64)?,
                filters: Filters::new(in_view, selection_rev)?,
            },
            Self::FilteredStep {
                check,
                backwards,
                after,
                cursor,
                waived,
                in_view,
                selection_rev,
            } => {
                if after.is_some() && cursor.is_some()
                    || selection_rev.is_some() && cursor.is_some()
                {
                    return Err("invalid_drc_request");
                }
                Command::FilteredStep {
                    request: StepRequest {
                        check: index(&check)?,
                        backwards,
                        after: after.as_deref().map(number).transpose()?,
                        cursor: cursor
                            .map(|c| {
                                Ok::<_, Failure>(StepCursor {
                                    next: number(&c.next)?,
                                    remaining: number(&c.remaining)?,
                                })
                            })
                            .transpose()?,
                        waived,
                        bbox_um: None,
                    },
                    filters: Filters::new(in_view, selection_rev)?,
                }
            }
            Self::Records { check, errors } => {
                if errors.len() > floe_app_core::drc::SELECTION_INPUT {
                    return Err("invalid_drc_request");
                }
                Command::SelectionCandidates {
                    check: Some(index(&check)?),
                    errors: errors.iter().map(|s| number(s)).collect::<Result<_, _>>()?,
                    bbox_um: None,
                    waived: None,
                }
            }
            Self::Step {
                check,
                backwards,
                after,
                cursor,
                waived,
                bbox_um,
            } => {
                if after.is_some() && cursor.is_some() {
                    return Err("invalid_drc_request");
                }
                Command::Step(StepRequest {
                    check: index(&check)?,
                    backwards,
                    after: after.as_deref().map(number).transpose()?,
                    cursor: cursor
                        .map(|c| {
                            Ok::<_, Failure>(StepCursor {
                                next: number(&c.next)?,
                                remaining: number(&c.remaining)?,
                            })
                        })
                        .transpose()?,
                    waived,
                    bbox_um: bbox_um.map(bbox).transpose()?,
                })
            }
            Self::Rules {
                start,
                search,
                limit,
                metric,
                waived,
            } => {
                if search.len() > 256
                    || metric
                        .as_ref()
                        .is_some_and(|m| m.is_empty() || m.len() > 64)
                {
                    return Err("invalid_drc_request");
                }
                Command::Rules {
                    start: index(&start)?,
                    search: search.to_lowercase(),
                    limit: cap(limit, 64)?,
                    metric,
                    waived,
                }
            }
            Self::Types { start, limit } => Command::Types {
                start: index(&start)?,
                limit: cap(limit, 64)?,
            },
            Self::Rule { check } => Command::Rule {
                check: index(&check)?,
            },
            Self::Measurements { check, error } => Command::Measurements {
                check: index(&check)?,
                error: number(&error)?,
            },
            Self::Comparison { check, error } => Command::Comparison {
                check: index(&check)?,
                error: number(&error)?,
            },
            Self::Focus {
                check,
                error,
                fit,
                isolate,
            } => Command::Focus {
                check: index(&check)?,
                error: number(&error)?,
                fit,
                context: None,
                isolate,
                preparation: None,
            },
            Self::InView {
                waived,
                cursor,
                limit,
            } => Command::InView {
                waived,
                cursor: Cursor {
                    check: index(&cursor.check)?,
                    error: number(&cursor.error)?,
                },
                limit: cap(limit, 64)?,
                context: None,
            },
            Self::Errors {
                check,
                start,
                waived,
                limit,
            } => Command::Errors {
                check: index(&check)?,
                start: number(&start)?,
                waived,
                limit: cap(limit, 64)?,
            },
            Self::Geometry {
                check,
                error,
                start,
                limit,
            } => Command::Geometry {
                check: index(&check)?,
                error: number(&error)?,
                start: index(&start)?,
                limit: cap(limit, 2048)?,
            },
            Self::Query {
                bbox_um,
                checks,
                waived,
                cursor,
                limit,
            } => {
                let b = bbox(bbox_um)?;
                if checks.as_ref().is_some_and(|c| c.len() > 128) {
                    return Err("invalid_drc_request");
                }
                Command::Query {
                    bbox_um: b,
                    checks: checks
                        .map(|c| c.iter().map(|v| index(v)).collect::<Result<_, _>>())
                        .transpose()?,
                    waived,
                    cursor: Cursor {
                        check: index(&cursor.check)?,
                        error: number(&cursor.error)?,
                    },
                    limit: cap(limit, 64)?,
                }
            }
        })
    }
}
impl Filters {
    fn new(in_view: bool, selection_rev: Option<String>) -> Result<Self, Failure> {
        Ok(Self {
            in_view,
            selection_rev: selection_rev
                .map(|s| crate::view::counter(&s).map_err(|_| "invalid_drc_request"))
                .transpose()?,
            context: None,
            selected: None,
        })
    }
    pub fn bounds(&self) -> floe_app_core::Result<Option<[f64; 4]>> {
        if self.in_view {
            let c = self
                .context
                .ok_or_else(|| floe_app_core::Error::input("filter requires authoritative view"))?;
            Ok(Some(c.bbox_dbu.map(|v| v * c.dbu)))
        } else {
            Ok(None)
        }
    }
    pub fn selection(&self) -> floe_app_core::Result<Option<&BTreeSet<u64>>> {
        if self.selection_rev.is_some() != self.selected.is_some() {
            return Err(floe_app_core::Error::input(
                "filter requires authoritative selection",
            ));
        }
        Ok(self.selected.as_ref())
    }
}
