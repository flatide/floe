//! Read-only compiled palette. No source, view, worker or filesystem access.
use crate::transport::{self, Gate};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

fn data() -> floe_app_core::Result<Value> {
    let p = floe_app_core::styles::presets::bundled()?;
    let fills: Vec<_> = p
        .fills
        .into_iter()
        .map(|p| {
            // Match GTK adapter canonicalization, retaining the bitmap for its
            // black-on-white, MSB-left preview. No client-side style inference.
            let fill = match p.rows {
                rows if rows == [u16::MAX; 16] => json!({"kind":"solid"}),
                rows if rows == [0; 16] => json!({"kind":"clear"}),
                rows if rows
                    == std::array::from_fn(|y| if y % 2 == 0 { 0xaaaa } else { 0x5555 }) =>
                {
                    json!({"kind":"speckle"})
                }
                rows => json!({"kind":"pattern","rows":rows}),
            };
            json!({"name":p.name,"rows":p.rows,"fill":fill})
        })
        .collect();
    Ok(json!({"version":1,"colors":p.colors,"fills":fills}))
}
pub(crate) async fn read(State(g): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    match data() {
        Ok(p) => Json(p).into_response(),
        Err(_) => transport::error(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preset_payload_is_bounded_and_uses_adapter_fill_semantics() {
        let p = data().unwrap();
        assert!(p.to_string().len() < 16 * 1024);
        assert_eq!(p["version"], 1);
        assert_eq!(p["colors"].as_array().unwrap().len(), 49);
        let f = p["fills"].as_array().unwrap();
        assert_eq!(f.len(), 20);
        assert_eq!(f[6]["fill"], json!({"kind":"speckle"}));
        assert_eq!(f[18]["fill"], json!({"kind":"solid"}));
        assert_eq!(f[19]["fill"], json!({"kind":"clear"}));
        for row in f.iter().filter(|p| p["fill"]["kind"] == "pattern") {
            assert_eq!(row["rows"], row["fill"]["rows"]);
        }
    }
}
