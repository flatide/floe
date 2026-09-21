//! No caller path, origin or storage directory may enter session-loss QA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loss {
    Storage,
    Cookie,
}
pub fn requested(args: &[String]) -> Option<Loss> {
    if args.len() != 1 {
        return None;
    }
    match args[0].as_str() {
        "--smoke-test-storage-loss" => Some(Loss::Storage),
        "--smoke-test-cookie-loss" => Some(Loss::Cookie),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_empty_workspace_modes_are_accepted() {
        for (flag, kind) in [
            ("--smoke-test-storage-loss", Loss::Storage),
            ("--smoke-test-cookie-loss", Loss::Cookie),
        ] {
            assert_eq!(requested(&[flag.into()]), Some(kind));
            for extra in ["user.oas", "--root", "--drc", "--drc-reviewer"] {
                assert_eq!(requested(&[flag.into(), extra.into()]), None);
            }
            assert_eq!(requested(&["view".into(), flag.into()]), None);
        }
        assert_eq!(requested(&[]), None);
    }
}
