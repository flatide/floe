//! floe2-web - the web product's command line (docs/SHARED_APP_LAYER.ko.md):
//! the shared application CLI (floe-app-cli - index, info, render, probe,
//! clip, jobdeck, drc, svrf, fe-embed and selfcheck, the same code as
//! feature/jobdeck's `floe2`) and the web's own commands - `view` (the web
//! viewer; a bare source, or nothing at all), `server` and `displaytest`.
#![forbid(unsafe_code)]
mod display_test;
pub mod embedded;
mod server;
mod web_view;
use floe_app_cli::Host;
use floe_app_core::{Error, ErrorKind, Result};
use std::ffi::OsString;
use std::sync::{atomic::AtomicUsize, Arc};

/// This host of the shared CLI: its name in the messages, the usage of its
/// own commands, the web bundle in `--version` and selfcheck, input errors 2.
static HOST: Host = Host {
    name: "floe2-web",
    version: env!("CARGO_PKG_VERSION"),
    usage: "Usage: floe2-web view [SOURCE ...] [OPTIONS]   (the web viewer; floe2-web view --help)
       floe2-web [VIEW OPTIONS] SOURCE ...     (same as view)
       floe2-web                               (empty workspace / present existing window)
       floe2-web server --help                 (policy check / public demo)
       floe2-web displaytest [PNG] [--no-open] [--port N] [--firefox PATH]
                                               (synthetic PNG/raw/crop pixels and an optional
                                               frozen PNG; GTK's gtktest has no alias here)
",
    metadata: || {
        vec![(
            "web_bundle",
            serde_json::Value::from(floe_web::transport::BUNDLE),
        )]
    },
    input_error_exit: 2,
};

/// The web's own commands (the shared ones are floe_app_cli's).
enum Cli {
    View(Box<web_view::Command>),
    Server(server::Command),
    DisplayTest(display_test::Command),
}
fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Cli> {
    let args: Vec<String> = args
        .into_iter()
        .map(|a| {
            a.into_string()
                .map_err(|_| Error::input("arguments must be UTF-8"))
        })
        .collect::<Result<_>>()?;
    if args.is_empty() {
        return web_view::parse(&["view".into()]).map(|c| Cli::View(Box::new(c)));
    }
    match args[0].as_str() {
        "server" => server::parse(&args).map(Cli::Server),
        "displaytest" => display_test::parse(&args).map(Cli::DisplayTest),
        "view" => web_view::parse(&args).map(|c| Cli::View(Box::new(c))),
        "gtktest" => Err(Error::new(
            ErrorKind::Unsupported,
            format!(
                "{} is GTK-only and replaced by displaytest [PNG] in the Rust product (no alias); About > Run display test provides synthetic checks. For GTK widget diagnosis, use the existing GTK reference package (no Python fallback)",
                args[0]
            ),
        )),
        // the shared CLI's global help and version, which it takes alone
        // (cli_main hands them to it before this parser)
        "--help" | "-h" | "--version" => {
            Err(Error::input("global help/version must be used alone"))
        }
        // the shared commands too: never a view's source
        command if floe_app_cli::COMMANDS.contains(&command) => Err(Error::input(format!(
            "{command} is the shared command line's (floe_app_cli)"
        ))),
        _ => {
            // Do not inspect the filesystem or guess extensions while parsing.
            // Known commands win; `-- NAME` or `./NAME` disambiguates a source.
            let mut view = Vec::with_capacity(args.len() + 1);
            view.push("view".into());
            view.extend(args);
            web_view::parse(&view).map(|c| Cli::View(Box::new(c)))
        }
    }
}
fn run(cli: Cli, cancelled: &Arc<AtomicUsize>) -> Result<i32> {
    match cli {
        Cli::Server(command) => server::run(command, cancelled),
        Cli::DisplayTest(command) => display_test::run(command, cancelled),
        Cli::View(command) => web_view::run(*command, cancelled),
    }
}
/// floe2-web's main: the shared commands, help and version run in the
/// shared CLI as this host; the web's own commands under the same signal
/// handling and exit statuses (floe_app_cli::exit_with).
pub fn cli_main() -> ! {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let first = args.first().and_then(|a| a.to_str()).unwrap_or("");
    if floe_app_cli::COMMANDS.contains(&first) || matches!(first, "--help" | "-h" | "--version") {
        floe_app_cli::main(&HOST, args);
    }
    let cli = match parse(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}: {e}", HOST.name);
            std::process::exit(2);
        }
    };
    floe_app_cli::exit_with(&HOST, |cancelled| run(cli, cancelled))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_web_commands_and_the_view_shorthand_parse_here_the_shared_ones_do_not() {
        let parsed = |words: &[&str]| parse(words.iter().map(OsString::from));
        assert!(matches!(parsed(&[]).unwrap(), Cli::View(_)));
        assert!(matches!(parsed(&["view"]).unwrap(), Cli::View(_)));
        assert!(matches!(
            parsed(&["한 글.oas", "--thin", "keep"]).unwrap(),
            Cli::View(_)
        ));
        assert!(matches!(
            parsed(&["server", "--help"]).unwrap(),
            Cli::Server(_)
        ));
        assert!(matches!(
            parsed(&["displaytest", "--no-open"]).unwrap(),
            Cli::DisplayTest(_)
        ));
        for shared in floe_app_cli::COMMANDS {
            assert!(parsed(&[shared]).is_err(), "{shared}");
        }
        for words in [
            &["--help", "a.oas"][..],
            &["--version", "a.oas"],
            &["gtktest"],
            &["view", "--thin", "keep"],
        ] {
            assert!(parsed(words).is_err(), "{words:?}");
        }
    }
}
