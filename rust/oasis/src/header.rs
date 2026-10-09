//! Bounded START-record probe for application metadata. Uses the same cursor
//! as the indexer; never scans geometry or inflates an OASIS CBLOCK.
use super::{err, Cur, OasisError, MAGIC};

#[derive(Clone, Debug, PartialEq)]
pub struct StartHeader {
    pub version: String,
    /// Grid steps per micron, as stored in START.
    pub unit: f64,
    /// Microns per database unit, for application coordinates.
    pub dbu: f64,
}

pub fn probe_start(bytes: &[u8]) -> Result<StartHeader, OasisError> {
    if !bytes.starts_with(MAGIC) {
        return err(0, "not an OASIS file (bad magic)");
    }
    let bounded = &bytes[..bytes.len().min(4096)];
    let mut c = Cur::new(&bounded[MAGIC.len()..], MAGIC.len());
    if c.uint()? != 1 {
        return err(c.here(), "OASIS file does not begin with a START record");
    }
    let version = c.string()?;
    if !version.is_ascii() {
        return err(c.here(), "OASIS version must be ASCII");
    }
    let version = String::from_utf8(version.to_vec()).expect("ASCII checked");
    let unit = c.real()?;
    let dbu = 1. / unit;
    if !unit.is_finite() || unit <= 0. || !dbu.is_finite() || dbu <= 0. {
        return err(c.here(), "OASIS unit is not finite and positive");
    }
    Ok(StartHeader { version, unit, dbu })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write::W;
    fn prefix() -> W {
        let mut w = W::new();
        w.out.extend_from_slice(MAGIC);
        w.uint(1);
        w.string(b"1.0");
        w
    }
    #[test]
    fn probe_accepts_all_positive_real_encodings() {
        for kind in [0, 2, 4, 6, 7] {
            let mut w = prefix();
            w.uint(kind);
            let expected = match kind {
                0 => {
                    w.uint(1000);
                    1000.
                }
                2 => {
                    w.uint(1000);
                    0.001
                }
                4 => {
                    w.uint(1000);
                    w.uint(3);
                    1000. / 3.
                }
                6 => {
                    w.out.extend_from_slice(&20000f32.to_le_bytes());
                    20000.
                }
                7 => {
                    w.out.extend_from_slice(&20000f64.to_le_bytes());
                    20000.
                }
                _ => unreachable!(),
            };
            let end = w.out.len();
            w.out.extend_from_slice(&[0xff; 6000]); // malformed geometry is irrelevant
            assert_eq!(
                probe_start(&w.out).unwrap(),
                StartHeader {
                    version: "1.0".into(),
                    unit: expected,
                    dbu: 1. / expected
                }
            );
            for n in 0..end {
                assert!(probe_start(&w.out[..n]).is_err());
            }
        }
    }
    #[test]
    fn malformed_units_and_unbounded_varints_return_errors() {
        for value in [0., -1., f64::NAN, f64::INFINITY, f64::MIN_POSITIVE / 100.] {
            let mut w = prefix();
            w.real_f64(value);
            assert!(probe_start(&w.out).is_err());
        }
        for bytes in [
            vec![0x80; 100],
            vec![0xff; 11],
            vec![0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 2],
        ] {
            let mut w = prefix();
            w.uint(0);
            w.out.extend(bytes);
            assert!(probe_start(&w.out).is_err());
        }
        let mut w = prefix();
        w.uint(0);
        w.uint(u64::MAX);
        assert_eq!(probe_start(&w.out).unwrap().unit, u64::MAX as f64);
        let mut w = W::new();
        w.out.extend_from_slice(MAGIC);
        w.uint(1);
        w.uint(1 << 40);
        assert!(probe_start(&w.out).is_err());
    }
}
