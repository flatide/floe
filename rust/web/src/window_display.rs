//! File-menu preferences belong to the owner window, not the source's layers.
//! CLI opens remain explicit. Capture only accepted controller state; no DOM
//! values or late post-open edits may change the first rendered generation.
use floe_app_core::{
    shots::{Detail, Thin},
    view::{Depth, Patch, ViewState},
};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenDisplay {
    #[default]
    Explicit,
    Window,
}

#[derive(Clone, Debug)]
pub(crate) struct WindowDisplay {
    depth: Option<u32>,
    detail: Detail,
    thin: Thin,
    frames: bool,
    // Jobdeck cannot render labels, but visiting one must not erase the
    // window's layout-label preference (GTK keeps labels_on while incapable).
    labels: bool,
    font_px: u32,
}
impl Default for WindowDisplay {
    fn default() -> Self {
        Self {
            depth: Some(0),
            detail: Detail::Medium,
            thin: Thin::Auto,
            frames: true,
            labels: true,
            font_px: 14,
        }
    }
}
impl WindowDisplay {
    pub fn initial(patch: Patch) -> floe_app_core::Result<Self> {
        let mut s = Self::default();
        if let Some(depth) = patch.depth {
            s.depth = match depth {
                Depth::Full => None,
                Depth::Levels(n) => (n < 999).then_some(n),
                Depth::Step(_) => {
                    return Err(floe_app_core::Error::input(
                        "initial depth must be absolute",
                    ))
                }
            };
        }
        s.detail = patch.detail.unwrap_or(s.detail);
        s.thin = patch.thin.unwrap_or(s.thin);
        s.frames = patch.frames.unwrap_or(s.frames);
        s.labels = s.frames && patch.labels.unwrap_or(s.labels);
        s.font_px = patch.font_px.unwrap_or(s.font_px);
        if !(6..=96).contains(&s.font_px) {
            return Err(floe_app_core::Error::input("label font must be 6..96 px"));
        }
        Ok(s)
    }
    pub fn label_preference(mut self, labels: Option<bool>) -> Self {
        if let Some(labels) = labels {
            self.labels = labels;
        }
        self
    }
    pub fn capture(&self, state: &ViewState, deck: bool) -> Self {
        Self {
            depth: state.depth,
            detail: state.detail,
            thin: state.thin,
            frames: state.frames,
            labels: if deck { self.labels } else { state.labels },
            font_px: state.font_px,
        }
    }
    pub fn patch(&self, deck: bool) -> Patch {
        Patch {
            depth: Some(if deck {
                Depth::Full
            } else {
                self.depth.map_or(Depth::Full, Depth::Levels)
            }),
            detail: Some(self.detail),
            thin: Some(self.thin),
            frames: Some(self.frames),
            labels: Some(!deck && self.labels),
            font_px: Some(self.font_px),
            // No camera, mono, selection or layer styles: a different source
            // starts fitted with its own defaults. Same-source reuse bypasses
            // this patch entirely, preserving its camera and caches.
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn open_policy_is_explicit_unless_requested_and_rejects_invalid_values() {
        let request =
            json!({"kind":"open","seq":"1","source_id":"source","mode":"level","body":{}});
        for policy in [None, Some(json!("window")), Some(json!("explicit"))] {
            let mut value = request.clone();
            if let Some(policy) = policy {
                value["display_policy"] = policy;
            }
            assert!(serde_json::from_value::<crate::service::OperationDto>(value).is_ok());
        }
        for policy in [Value::Null, json!(true), json!("inherit"), json!({})] {
            let mut value = request.clone();
            value["display_policy"] = policy;
            assert!(serde_json::from_value::<crate::service::OperationDto>(value).is_err());
        }
        assert!(matches!(OpenDisplay::default(), OpenDisplay::Explicit));
        let mut null = request.clone();
        null["label_preference"] = Value::Null;
        assert!(serde_json::from_value::<crate::service::OperationDto>(null).is_err());
        let default = WindowDisplay::default().patch(false);
        assert!(matches!(default.depth, Some(Depth::Levels(0))));
        assert_eq!(
            (default.frames, default.labels, default.font_px),
            (Some(true), Some(true), Some(14))
        );
    }

    #[test]
    fn initial_window_preserves_baseline_and_validates_font_and_absolute_depth() {
        let seed = WindowDisplay::initial(Patch {
            frames: Some(false),
            labels: Some(true),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(seed.patch(false).labels, Some(false));
        assert!(WindowDisplay::initial(Patch {
            font_px: Some(97),
            ..Default::default()
        })
        .is_err());
        assert!(WindowDisplay::initial(Patch {
            depth: Some(Depth::Step(1)),
            ..Default::default()
        })
        .is_err());
        let full = WindowDisplay::initial(Patch {
            depth: Some(Depth::Levels(999)),
            ..Default::default()
        })
        .unwrap();
        assert!(matches!(full.patch(false).depth, Some(Depth::Full)));
    }

    #[test]
    #[ignore = "run tools/validate_web_file_display.py GTK source oracle"]
    fn gtk_file_display_oracle() {
        let cases: Vec<Value> = serde_json::from_slice(
            &std::fs::read(std::env::var_os("FLOE_FILE_DISPLAY_ORACLE").unwrap()).unwrap(),
        )
        .unwrap();
        assert!(cases.len() >= 1000);
        for case in &cases {
            let old = &case["before"];
            let display = WindowDisplay {
                depth: old["depth"].as_u64().filter(|n| *n < 999).map(|n| n as u32),
                detail: match old["detail"].as_u64().unwrap() {
                    0 => Detail::Low,
                    1 => Detail::Medium,
                    2 => Detail::High,
                    _ => unreachable!(),
                },
                thin: match old["thin"].as_str().unwrap() {
                    "auto" => Thin::Auto,
                    "keep" => Thin::Keep,
                    "cull" => Thin::Cull,
                    _ => unreachable!(),
                },
                frames: old["frames"].as_bool().unwrap(),
                labels: old["labels"].as_bool().unwrap(),
                font_px: old["font_px"].as_u64().unwrap() as u32,
            };
            let deck = case["deck"].as_bool().unwrap();
            let same = case["same"].as_bool().unwrap();
            let p = display.patch(deck);
            assert!(p.mono.is_none() && p.layers.is_none() && p.navigation.is_none());
            let got = json!({
                "depth": if same {display.depth} else {match p.depth.unwrap() {Depth::Full=>None,Depth::Levels(n)=>Some(n),_=>unreachable!()}},
                "detail":match p.detail.unwrap() {Detail::Low=>0,Detail::Medium=>1,Detail::High=>2,_=>unreachable!()},
                "thin":format!("{:?}",p.thin.unwrap()).to_lowercase(),"frames":p.frames.unwrap(),
                "labels":p.labels.unwrap(),"font_px":p.font_px.unwrap(),"mono":same});
            assert_eq!(got, case["want"], "{case}");
        }
        println!(
            "GTK FILE DISPLAY: ALL OK ({} source-derived cases)",
            cases.len()
        );
    }
}
