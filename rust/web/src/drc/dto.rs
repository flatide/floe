use super::Failure;
use floe_app_core::drc::Cursor;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CursorDto {
    pub check: String,
    pub error: String,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Rules {
        start: String,
        search: String,
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
    Focus {
        check: String,
        error: String,
        fit: bool,
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
    ValidatePanel(Box<super::panel::Data>),
    Rules {
        start: usize,
        search: String,
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
    Focus {
        check: usize,
        error: u64,
        fit: bool,
        context: Option<FocusContext>,
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
fn index(s: &str) -> Result<usize, Failure> {
    usize::try_from(number(s)?).map_err(|_| "invalid_drc_request")
}
fn cap(n: usize, max: usize) -> Result<usize, Failure> {
    if n == 0 || n > max {
        Err("invalid_drc_request")
    } else {
        Ok(n)
    }
}
impl Request {
    pub(super) fn core(self) -> Result<Command, Failure> {
        Ok(match self {
            Self::Rules {
                start,
                search,
                limit,
            } => {
                if search.len() > 256 {
                    return Err("invalid_drc_request");
                }
                Command::Rules {
                    start: index(&start)?,
                    search: search.to_lowercase(),
                    limit: cap(limit, 64)?,
                }
            }
            Self::Rule { check } => Command::Rule {
                check: index(&check)?,
            },
            Self::Focus { check, error, fit } => Command::Focus {
                check: index(&check)?,
                error: number(&error)?,
                fit,
                context: None,
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
                let mut b = [0.; 4];
                for (out, s) in b.iter_mut().zip(bbox_um) {
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
