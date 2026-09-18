#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "macos")]
mod macos;
mod service;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!(
            "floe2-desktop [view] [SOURCE] [VIEW OPTIONS]\n\
            Independent embedded WebView; no external browser or Python.\n\
            View options: floe2-web view --help (except browser/session-file options).\n\
            Current host: macOS only; RHEL 8/ETX host is pending.\n\
            --smoke-test: empty-workspace native authentication/close test only.\n\
            D1 preview: local file upload/download and clipboard acceptance pending."
        );
        return;
    }
    let smoke = args == ["--smoke-test"];
    if smoke {
        args.clear();
    }
    if args.first().is_some_and(|a| a == "view") {
        args.remove(0);
    }
    let result = floe_app::embedded::Session::parse(&args).and_then(|session| {
        #[cfg(target_os = "macos")]
        {
            macos::run(session, smoke)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (session, smoke);
            Err(floe_app_core::Error::input(
                "native host not implemented for this platform; use floe2-web",
            ))
        }
    });
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("floe2-desktop: {e}");
            std::process::exit(1);
        }
    }
}
