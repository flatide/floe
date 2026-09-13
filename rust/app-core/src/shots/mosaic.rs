//! Deterministic CPU RGBA composition; no rasterizer, Pillow or browser involved.
use super::{batch::color, MAX_PIXELS};
use crate::{check_cancelled, Error, Result};
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::AtomicUsize;

pub struct Mosaic {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    next: usize,
}
impl Mosaic {
    pub fn new(tile_w: u32, tile_h: u32) -> Result<Self> {
        let count = u64::from(tile_w) * u64::from(tile_h);
        if count == 0 || count > MAX_PIXELS {
            return Err(Error::input("mosaic tile exceeds 16 Mpx"));
        }
        let width = tile_w
            .checked_mul(2)
            .ok_or_else(|| Error::input("mosaic width overflow"))?;
        let height = tile_h
            .checked_mul(2)
            .ok_or_else(|| Error::input("mosaic height overflow"))?;
        Ok(Self {
            width,
            height,
            rgba: vec![0; (count * 16) as usize],
            next: 0,
        })
    }
    pub fn insert(&mut self, rgba: &[u8], cancelled: &AtomicUsize) -> Result<()> {
        let (w, h) = (self.width as usize / 2, self.height as usize / 2);
        if self.next >= 4 || rgba.len() != w * h * 4 {
            return Err(Error::input("mosaic needs four matching RGBA tiles"));
        }
        let (ox, oy) = (self.next % 2 * w, self.next / 2 * h);
        for y in 0..h {
            check_cancelled(cancelled)?;
            let dst = ((oy + y) * self.width as usize + ox) * 4;
            self.rgba[dst..dst + w * 4].copy_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
        }
        self.next += 1;
        Ok(())
    }
    pub fn finish(
        mut self,
        line: f64,
        line_color: &str,
        cancelled: &AtomicUsize,
    ) -> Result<(Vec<u8>, serde_json::Value)> {
        if self.next != 4 {
            return Err(Error::input("mosaic is missing tiles"));
        }
        let rgb = color(line_color)?;
        let blend = |bytes: &mut [u8], alpha: f64| {
            for k in 0..3 {
                bytes[k] = if alpha >= 1. {
                    rgb[k]
                } else {
                    (f64::from(bytes[k]) * (1. - alpha) + f64::from(rgb[k]) * alpha)
                        .round_ties_even() as u8
                };
            }
            bytes[3] = 255;
        };
        for (x, a) in spans(self.width / 2, line, self.width)? {
            check_cancelled(cancelled)?;
            for y in 0..self.height as usize {
                let i = (y * self.width as usize + x as usize) * 4;
                blend(&mut self.rgba[i..i + 4], a);
            }
        }
        for (y, a) in spans(self.height / 2, line, self.height)? {
            check_cancelled(cancelled)?;
            for x in 0..self.width as usize {
                let i = (y as usize * self.width as usize + x) * 4;
                blend(&mut self.rgba[i..i + 4], a);
            }
        }
        let half = line / 2.;
        let full = half.floor();
        let frac = half - full;
        let mut parts = Vec::new();
        if full != 0. {
            parts.push(format!("{full:.0} solid"));
        }
        if frac > 1e-9 {
            parts.push(format!("1 at {:.0}%", (frac * 100.).round_ties_even()));
        }
        let pixels = if parts.is_empty() {
            "none".into()
        } else {
            format!("{} on each side", parts.join(" + "))
        };
        let info = serde_json::json!({"width":self.width,"height":self.height,"tile":[self.width/2,self.height/2],
            "line":line,"line_pixels":pixels,"line_color":line_color});
        Ok((self.rgba, info))
    }
}
/// A very wide separator must cost O(image width), not O(user's line value).
fn spans(seam: u32, line: f64, limit: u32) -> Result<impl Iterator<Item = (u32, f64)>> {
    if !line.is_finite() || line < 0. {
        return Err(Error::input("line width must be finite and nonnegative"));
    }
    let half = line / 2.;
    let full = half.floor();
    let frac = half - full;
    Ok((0..limit).filter_map(move |i| {
        let distance = if i < seam { seam - 1 - i } else { i - seam };
        if f64::from(distance) < full {
            Some((i, 1.))
        } else if f64::from(distance) == full && frac > 1e-9 {
            Some((i, frac))
        } else {
            None
        }
    }))
}

/// PNG filter-0/RGBA8, one streaming IDAT. The compressed bytes go to staging
/// storage directly; no second frame-sized filtered or compressed Vec exists.
pub fn encode_png(
    output: &mut (impl Write + Seek),
    width: u32,
    height: u32,
    rgba: &[u8],
    cancelled: &AtomicUsize,
) -> Result<()> {
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > 4 * MAX_PIXELS || pixels * 4 != rgba.len() as u64 {
        return Err(Error::input(
            "PNG dimensions/length exceed the 64 Mpx mosaic limit",
        ));
    }
    check_cancelled(cancelled)?;
    output.write_all(b"\x89PNG\r\n\x1a\n")?;
    let mut header = width.to_be_bytes().to_vec();
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(output, b"IHDR", &header)?;
    let length_pos = output.stream_position()?;
    output.write_all(&[0; 4])?;
    output.write_all(b"IDAT")?;
    struct Compressed<'a, W> {
        output: &'a mut W,
        hash: crc32fast::Hasher,
        length: u64,
    }
    impl<W: Write> Write for Compressed<'_, W> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let n = self.output.write(bytes)?;
            self.hash.update(&bytes[..n]);
            self.length += n as u64;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.output.flush()
        }
    }
    let mut hash = crc32fast::Hasher::new();
    hash.update(b"IDAT");
    let stream = Compressed {
        output: &mut *output,
        hash,
        length: 0,
    };
    let mut z = flate2::write::ZlibEncoder::new(stream, flate2::Compression::new(6));
    for row in rgba.chunks_exact(width as usize * 4) {
        check_cancelled(cancelled)?;
        z.write_all(&[0])?;
        z.write_all(row)?;
    }
    let stream = z.finish()?;
    let length =
        u32::try_from(stream.length).map_err(|_| Error::input("PNG IDAT length overflow"))?;
    let crc = stream.hash.finalize();
    output.write_all(&crc.to_be_bytes())?;
    let end = output.stream_position()?;
    output.seek(SeekFrom::Start(length_pos))?;
    output.write_all(&length.to_be_bytes())?;
    output.seek(SeekFrom::Start(end))?;
    chunk(output, b"IEND", &[])?;
    check_cancelled(cancelled)
}
fn chunk(output: &mut impl Write, kind: &[u8; 4], data: &[u8]) -> Result<()> {
    output.write_all(&(data.len() as u32).to_be_bytes())?;
    output.write_all(kind)?;
    output.write_all(data)?;
    let mut hash = crc32fast::Hasher::new();
    hash.update(kind);
    hash.update(data);
    output.write_all(&hash.finalize().to_be_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};
    #[test]
    fn streaming_png_and_fractional_seams() {
        let flag = AtomicUsize::new(0);
        let mut m = Mosaic::new(2, 2).unwrap();
        for c in [10, 20, 30, 40] {
            m.insert(&[c, 0, 0, 255].repeat(4), &flag).unwrap();
        }
        let (rgba, info) = m.finish(1., "#808080", &flag).unwrap();
        assert_eq!(info["line_pixels"], "1 at 50% on each side");
        assert_eq!(&rgba[..4], &[10, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[69, 64, 64, 255]);
        assert_eq!(&rgba[20..24], &[98, 96, 96, 255]);
        let mut png = Cursor::new(Vec::new());
        encode_png(&mut png, 4, 4, &rgba, &flag).unwrap();
        let png = png.into_inner();
        let mut offset = 8;
        let mut kinds = Vec::new();
        while offset < png.len() {
            let length = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
            let end = offset + 8 + length;
            kinds.push(&png[offset + 4..offset + 8]);
            assert_eq!(
                crc32fast::hash(&png[offset + 4..end]),
                u32::from_be_bytes(png[end..end + 4].try_into().unwrap())
            );
            offset = end + 4;
        }
        assert_eq!(kinds, [b"IHDR", b"IDAT", b"IEND"]);
        let n = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&png[41..41 + n])
            .read_to_end(&mut raw)
            .unwrap();
        for (y, row) in raw.chunks_exact(17).enumerate() {
            assert_eq!(row[0], 0);
            assert_eq!(&row[1..], &rgba[y * 16..(y + 1) * 16]);
        }
        assert_eq!(spans(2, 1e100, 4).unwrap().count(), 4);
        assert!(Mosaic::new(0, 1).is_err());
    }
}
