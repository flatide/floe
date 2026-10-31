//! `--workdir DIR` (user 2026-10-31: "--workdir로 .ice, .tray, .sqlite등의
//! 파일을 읽고 쓰는 위치를 지정할 수 있게"): the folder every derived file -
//! a source's cache, a results database's pack, their review and analysis
//! sidecars, revisions and locks - is read from and written to, at the
//! source's absolute path under it (app-core `cache::names`). The option is
//! every command's and goes anywhere on the line (`floe2 --workdir DIR
//! render ...`, `floe2 render ... --workdir DIR`, `--workdir=DIR`); FLOE_WORKDIR
//! is its default. It is taken out of the arguments before the command
//! parses them, made absolute, created when it is not there, set in
//! app-core and exported as FLOE_WORKDIR so the processes this one starts
//! (floe-index, floe-renderd, the GTK viewer and its service) place their
//! files the same way. The rule is floe_vfs::workdir's, floe-index's too.
use floe_app_core::{Error, Result};
use std::ffi::OsString;
use std::path::PathBuf;

/// Take `--workdir DIR` / `--workdir=DIR` out of `args` (up to a bare `--`;
/// the last one wins) and apply it, else FLOE_WORKDIR (floe_vfs::workdir:
/// `~/` expanded, absolute, made when it is not there, set for this
/// process and exported for its children): the work directory now in
/// force, if any. An error is the command line's (exit 2).
pub fn take(args: &mut Vec<OsString>) -> Result<Option<PathBuf>> {
    floe_vfs::workdir::take_option(args).map_err(Error::input)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }
    #[test]
    fn the_option_is_taken_from_anywhere_before_a_bare_double_dash() {
        // observed through values apply refuses: the process-wide setting
        // is never made in a test
        let mut v = os(&["--workdir", "/", "render", "x.oas", "--px", "64"]);
        assert!(take(&mut v).unwrap_err().message.contains("root directory"));
        assert_eq!(v, os(&["render", "x.oas", "--px", "64"]));
        let mut v = os(&["render", "x.oas", "--workdir=", "--out", "a.png"]);
        assert!(take(&mut v).unwrap_err().message.contains("needs a directory"));
        assert_eq!(v, os(&["render", "x.oas", "--out", "a.png"]));
        let mut v = os(&["index", "--workdir", "/a", "--workdir", "/", "x.oas"]);
        assert!(take(&mut v).unwrap_err().message.contains("root directory"));
        assert_eq!(v, os(&["index", "x.oas"]));
        let mut v = os(&["index", "--", "--workdir"]);
        let _ = take(&mut v);
        assert_eq!(v, os(&["index", "--", "--workdir"]));
    }
    #[test]
    fn a_missing_or_empty_value_is_a_command_line_error() {
        let mut v = os(&["info", "x.oas", "--workdir"]);
        let e = take(&mut v).unwrap_err();
        assert!(e.message.contains("needs a directory"), "{}", e.message);
        let mut v = os(&["info", "--workdir=", "x.oas"]);
        let e = take(&mut v).unwrap_err();
        assert!(e.message.contains("needs a directory"), "{}", e.message);
        let mut v = os(&["info", "--workdir", " ", "x.oas"]);
        assert!(take(&mut v).is_err());
    }
}
