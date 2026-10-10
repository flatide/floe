//! floe2 - the product's command line (docs/SHARED_APP_LAYER.ko.md; user
//! 2026-10-09: every main function but the UI in Rust, shared with the web
//! port). index, info, render, probe, clip, jobdeck, drc, svrf, fe-embed and
//! selfcheck are the shared Rust CLI (floe-app-cli, the same code as
//! floe2-web's); `view` - and a bare source or nothing at all - starts the
//! GTK viewer, the only part left in Python.
#![forbid(unsafe_code)]
mod gtk;
mod service;
mod view;

use floe_app_cli::Host;
use std::ffi::OsString;

static HOST: Host = Host {
    name: "floe2",
    version: env!("FLOE2_VERSION"),
    usage: "Usage: floe2 view [SOURCE] [VIEW OPTIONS]      (the GTK viewer; floe2 view --help)\n       floe2 [VIEW OPTIONS] SOURCE                (same as view)\n       floe2                                       (the viewer, empty)\n       floe2 gtktest [PNG]                         (a GTK display check: a black view's diagnosis)\n",
    metadata: || vec![("viewer", serde_json::Value::from("gtk"))],
    input_error_exit: 1,
};

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let first = args.first().and_then(|a| a.to_str()).unwrap_or("");
    // the GTK viewer's non-UI answers (P2): spawned by floe/gtkservice.py
    if first == "gtk-service" {
        std::process::exit(service::run());
    }
    if floe_app_cli::COMMANDS.contains(&first) || matches!(first, "--help" | "-h" | "--version") {
        floe_app_cli::main(&HOST, args);
    }
    // the viewer: `view ARGS`, or the arguments as they are (`gtktest`,
    // the GTK display check, is the viewer entry's too: floe/viewcli.py)
    let rest = if first == "view" {
        args[1..].to_vec()
    } else {
        args
    };
    std::process::exit(gtk::launch(rest));
}
