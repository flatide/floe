use crc32fast::Hasher;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;

pub(crate) fn encode_rgba(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("PNG width and height must be positive".to_string());
    }
    let stride = (width as usize)
        .checked_mul(4)
        .ok_or_else(|| "PNG stride overflow".to_string())?;
    let expected = stride
        .checked_mul(height as usize)
        .ok_or_else(|| "PNG image length overflow".to_string())?;
    if pixels.len() != expected {
        return Err(format!(
            "PNG RGBA length mismatch: got {}, expected {}",
            pixels.len(),
            expected
        ));
    }

    let raw_len = expected
        .checked_add(height as usize)
        .ok_or_else(|| "PNG scanline length overflow".to_string())?;
    let mut raw = Vec::with_capacity(raw_len);
    for row in pixels.chunks_exact(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(&raw).map_err(|error| error.to_string())?;
    let compressed = encoder.finish().map_err(|error| error.to_string())?;

    let mut png = Vec::with_capacity(compressed.len().saturating_add(57));
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    append_chunk(&mut png, b"IHDR", &header)?;
    append_chunk(&mut png, b"IDAT", &compressed)?;
    append_chunk(&mut png, b"IEND", &[])?;
    Ok(png)
}

fn append_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<(), String> {
    let len: u32 = data
        .len()
        .try_into()
        .map_err(|_| "PNG chunk exceeds u32 length".to_string())?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut hasher = Hasher::new();
    hasher.update(kind);
    hasher.update(data);
    out.extend_from_slice(&hasher.finalize().to_be_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    #[test]
    fn emits_deterministic_filter_zero_rgba_png() {
        let pixels = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let first = encode_rgba(2, 2, &pixels).unwrap();
        let second = encode_rgba(2, 2, &pixels).unwrap();
        assert_eq!(first, second);
        assert_eq!(&first[..8], b"\x89PNG\r\n\x1a\n");

        let ihdr_len = u32::from_be_bytes(first[8..12].try_into().unwrap()) as usize;
        let idat_offset = 8 + 12 + ihdr_len;
        assert_eq!(&first[idat_offset + 4..idat_offset + 8], b"IDAT");
        let idat_len =
            u32::from_be_bytes(first[idat_offset..idat_offset + 4].try_into().unwrap()) as usize;
        let compressed = &first[idat_offset + 8..idat_offset + 8 + idat_len];
        let mut decoded = Vec::new();
        ZlibDecoder::new(compressed)
            .read_to_end(&mut decoded)
            .unwrap();
        let mut expected = vec![0];
        expected.extend_from_slice(&pixels[..8]);
        expected.push(0);
        expected.extend_from_slice(&pixels[8..]);
        assert_eq!(decoded, expected);
    }
}
