#![deny(unsafe_op_in_unsafe_fn)]

mod actions;
mod close_request;
mod download_qa;
#[cfg(target_os = "macos")]
mod macos;
mod recovery;
mod review_qa;
mod service;
mod session_qa;
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
            --smoke-test-recovery: same test plus explicit reload and close-timeout cancel.\n\
            --smoke-test-review-recovery: NEW synthetic files only; save ACK loss/reload.\n\
            --smoke-test-storage-loss / --smoke-test-cookie-loss: NEW empty WebView only.\n\
            --smoke-test-download-cancel: NEW synthetic blob/staging files only.\n\
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
    let smoke_recovery = args == ["--smoke-test-recovery"];
    let smoke_review = review_qa::requested(&args);
    let smoke_loss = session_qa::requested(&args);
    let smoke_download = download_qa::requested(&args);
    let smoke = smoke_notices
        || smoke_recovery
        || smoke_review
        || smoke_loss.is_some()
        || smoke_download
        || args == ["--smoke-test"];
    if smoke {
        args.clear();
    }
    if args.first().is_some_and(|a| a == "view") {
        args.remove(0);
    }
    let result = (|| {
        if smoke_review && !cfg!(target_os = "macos") {
            return Err(floe_app_core::Error::input(
                "review WebView QA requires macOS",
            ));
        }
        let fixture = if smoke_review {
            Some(review_qa::Fixture::create()?)
        } else {
            None
        };
        if let Some(fixture) = &fixture {
            args = fixture.arguments()?;
        }
        let session = floe_app::embedded::Session::parse(&args)?;
        #[cfg(target_os = "macos")]
        {
            let code = macos::run(
                session,
                smoke,
                smoke_notices,
                smoke_recovery,
                smoke_review,
                smoke_loss,
                smoke_download,
            )?;
            if let Some(fixture) = &fixture {
                fixture.verify()?;
                println!("DESKTOP REVIEW FILES: OK (exact note/waive read-back; 0600; original inputs unchanged)");
            }
            Ok(code)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (session, smoke, smoke_notices, smoke_recovery, fixture);
            Err(floe_app_core::Error::input(
                "native host not implemented for this platform; use floe2-web",
            ))
        }
    })();
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
