//! Explicit read-only Canvas QA; the development driver supplies a NEW valmini.
use floe_app_core::{Error, Result};

pub fn arguments(args: &[String]) -> Result<Option<Vec<String>>> {
    if args
        .first()
        .is_none_or(|s| s != "--smoke-frame-parity-test")
    {
        return Ok(None);
    }
    if args.len() != 2 || !std::path::Path::new(&args[1]).is_absolute() {
        return Err(Error::input(
            "frame parity QA requires one absolute synthetic source",
        ));
    }
    Ok(Some(vec![
        args[1].clone(),
        "--goto".into(),
        "200,200,300".into(),
        "--depth".into(),
        "full".into(),
        "--detail".into(),
        "high".into(),
        "--jobs".into(),
        "4".into(),
        "--raster-jobs".into(),
        "4".into(),
        "--refinement".into(),
        "off".into(),
        "--raw".into(),
    ]))
}

/// Only finite numeric metrics from the fixed probe may leave the native host.
/// Phase zero reports the accepted label difference; phases one/two are strict.
pub fn metric(text: &str, expected_phase: u8) -> Option<String> {
    let raw = text.strip_prefix("layout-metric ")?;
    let fields: Vec<_> = raw.split(' ').collect();
    if fields.len() != 7
        || fields
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
    {
        return None;
    }
    let nums: Vec<f64> = fields
        .iter()
        .map(|s| s.parse::<f64>().ok())
        .collect::<Option<_>>()?;
    if nums.iter().any(|n| !n.is_finite())
        || expected_phase > 2
        || nums[0] != f64::from(expected_phase)
        || nums[1..3].iter().any(|n| *n < 1. || n.fract() != 0.)
        || nums[1] * nums[2] > 16. * 1024. * 1024.
        || !(0.25..=8.).contains(&nums[3])
        || nums[4..]
            .iter()
            .any(|n| *n < 0. || n.fract() != 0. || *n > nums[1] * nums[2])
        || nums[4] == 0.
        || nums[5] == 0.
        || (expected_phase != 0 && nums[6] != 0.)
    {
        return None;
    }
    Some(format!(
        "DESKTOP LAYOUT: phase={} pixels={}x{} dpr={} foreground_lit={} margin_lit={} changed={}",
        nums[0], nums[1], nums[2], nums[3], nums[4], nums[5], nums[6]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_qa_only_and_fixed_read_only_arguments() {
        assert!(arguments(&["view".into(), "/tmp/a".into()])
            .unwrap()
            .is_none());
        for args in [
            vec!["--smoke-frame-parity-test".into()],
            vec!["--smoke-frame-parity-test".into(), "relative".into()],
            vec![
                "--smoke-frame-parity-test".into(),
                "/tmp/a".into(),
                "--force".into(),
            ],
        ] {
            assert!(arguments(&args).is_err());
        }
        let args = arguments(&["--smoke-frame-parity-test".into(), "/tmp/합성 a.oas".into()])
            .unwrap()
            .unwrap();
        assert_eq!(args[0], "/tmp/합성 a.oas");
        assert!(!args
            .iter()
            .any(|s| s.contains("reviewer") || s == "--force"));
        assert_eq!(args.last().unwrap(), "--raw");
    }
    #[test]
    fn metrics_reject_mismatch_non_numeric_or_unbounded_content() {
        assert!(metric("layout-metric 0 100 80 2 300 328 28", 0).is_some());
        assert!(metric("layout-metric 1 100 80 2 300 300 0", 1).is_some());
        for bad in [
            "layout-metric 1 100 80 2 300 328 28",
            "layout-metric 0 100 80 2 300 300 0",
            "layout-metric 1 100 80 NaN 300 300 0",
            "layout-metric 1 100 80 2 0 0 0",
            "layout-metric 1 100 80 2 9000 300 0",
            "layout-metric 1 100000 100000 2 300 300 0",
            "layout-metric 1 100 80 2 300 300 0 secret",
            "layout-metric 1 100 80 2 300 300 -1",
        ] {
            assert!(metric(bad, 1).is_none(), "{bad}");
        }
    }
}
