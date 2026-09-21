#![deny(unsafe_op_in_unsafe_fn)]

mod actions;
#[cfg(target_os = "macos")]
mod macos;
mod service;
mod transfers;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!(
            "floe2-desktop [view] [SOURCE] [VIEW OPTIONS]\n\
            Independent embedded WebView; no external browser or Python.\n\
            View options: floe2-web view --help (except browser/session-file options).\n\
            Current host: macOS only; RHEL 8/ETX host is pending.\n\
            --smoke-test: empty-workspace native authentication/close test only.\n\
            --smoke-test-notices: same test plus packaged About notice reads.\n\
            --check-notices: verify every packaged notice chunk; no GUI or workers.\n\
            With no SOURCE or --root, choose an approved working folder before startup.\n\
            macOS preview: native file dialogs, File/Edit menus and explicit Recover View."
        );
        return;
    }
    if args == ["--check-notices"] {
        match floe_app::embedded::check_notices() {
            Ok(count) => println!("DESKTOP NOTICES: OK ({count} files; all chunks checked; not signature/legal approval)"),
            Err(error) => {
                eprintln!("floe2-desktop: {}", actions::error_text(&error.to_string()));
                std::process::exit(1);
            }
        }
        return;
    }
    let smoke_notices = args == ["--smoke-test-notices"];
    let smoke = smoke_notices || args == ["--smoke-test"];
    if smoke {
        args.clear();
    }
    if args.first().is_some_and(|a| a == "view") {
        args.remove(0);
    }
    let result = floe_app::embedded::Session::parse(&args).and_then(|session| {
        #[cfg(target_os = "macos")]
        {
            macos::run(session, smoke, smoke_notices)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (session, smoke, smoke_notices);
            Err(floe_app_core::Error::input(
                "native host not implemented for this platform; use floe2-web",
            ))
        }
    });
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            let message = actions::error_text(&e.to_string());
            eprintln!("floe2-desktop: {message}");
            #[cfg(target_os = "macos")]
            if !smoke {
                macos::show_error(&message);
            }
            std::process::exit(1);
        }
    }
}
