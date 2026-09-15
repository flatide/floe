//! Fixed synthetic pixels only. No view, renderer, filesystem or query identity.
use crate::transport::{self, Gate};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use std::{
    io::Cursor,
    sync::{atomic::AtomicUsize, OnceLock},
};

pub(crate) const WIDTH: u32 = 360;
pub(crate) const HEIGHT: u32 = 160;
const COLORS: [[u8; 4]; 4] = [
    [255, 51, 51, 255],
    [51, 255, 51, 255],
    [51, 51, 255, 255],
    [255, 255, 51, 255],
];
static IMAGES: OnceLock<Result<(Bytes, Bytes), ()>> = OnceLock::new();

fn images() -> Result<&'static (Bytes, Bytes), ()> {
    IMAGES
        .get_or_init(|| {
            let mut rgba = vec![0; (WIDTH * HEIGHT * 4) as usize];
            for pixel in rgba.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
            for (i, color) in COLORS.iter().enumerate() {
                for y in 30..130 {
                    for x in 20 + i * 85..90 + i * 85 {
                        rgba[(y * WIDTH as usize + x) * 4..][..4].copy_from_slice(color);
                    }
                }
            }
            let mut png = Cursor::new(Vec::new());
            floe_app_core::shots::mosaic::encode_png(
                &mut png,
                WIDTH,
                HEIGHT,
                &rgba,
                &AtomicUsize::new(0),
            )
            .map_err(|_| ())?;
            let mut raw = b"FLOERAW1".to_vec();
            raw.extend_from_slice(&WIDTH.to_le_bytes());
            raw.extend_from_slice(&HEIGHT.to_le_bytes());
            raw.extend(rgba);
            Ok((Bytes::from(raw), Bytes::from(png.into_inner())))
        })
        .as_ref()
        .map_err(|_| ())
}

pub(crate) async fn read(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(format): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    if !matches!(format.as_str(), "raw" | "png") {
        return transport::error(StatusCode::NOT_FOUND);
    }
    let Ok((raw, png)) = images() else {
        return transport::error(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let (mime, body) = if format == "raw" {
        ("application/octet-stream", raw.clone())
    } else {
        ("image/png", png.clone())
    };
    ([(header::CONTENT_TYPE, mime)], body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_is_fixed_opaque_and_bounded() {
        let (raw, png) = images().unwrap();
        assert_eq!(raw.len(), 16 + 360 * 160 * 4);
        assert_eq!(&raw[..8], b"FLOERAW1");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(png.len() < 16 * 1024);
        assert_eq!(
            raw[16..]
                .chunks_exact(4)
                .filter(|p| p[0..3] != [0, 0, 0])
                .count(),
            4 * 70 * 100
        );
        assert!(raw[16..].chunks_exact(4).all(|p| p[3] == 255));
        assert_eq!(images().unwrap().0.as_ptr(), raw.as_ptr());
    }
    #[test]
    #[ignore = "development GTK/browser fixture oracle"]
    fn export_display_fixture() {
        let dir = std::path::PathBuf::from(
            std::env::var_os("FLOE_DISPLAY_TEST_DIR").expect("private output directory"),
        );
        let (raw, png) = images().unwrap();
        for (name, bytes) in [("test.raw", raw), ("test.png", png)] {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join(name))
                .unwrap();
            f.write_all(bytes).unwrap();
        }
    }
}
