//! Independent local display diagnostics. No source, indexer, renderer or IPC owner.
use crate::web_view::SessionFile;
use floe_app_core::{browser, Error, Result};
use floe_web::transport::{self, Gateway, BUNDLE};
use serde_json::json;
use std::{
    net::{Ipv4Addr, TcpListener},
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

const HELP: &str = "Usage: floe2-web displaytest [OPTIONS]

  --no-open           Use the private session link without launching Firefox
  --port N            Loopback port (0 = random, default)
  --firefox PATH      Explicit Firefox executable (or FLOE_FIREFOX_BIN)
  --session-file FILE New 0600 session JSON; never overwrite
  -h, --help          Show help

Independent synthetic PNG/raw/crop display diagnostics. No layout, indexer,
renderd, Python, default workspace IPC, uploads or image files are used.
Run the test explicitly in the page. Canvas readback is not remote-screen
acceptance; visually inspect the panels. The optional gtktest PNG input is
not migrated yet. Quit the session or close its isolated Firefox to stop.
With --no-open, closing a tab does not stop the server: use Quit or Ctrl+C.
The private bootstrap link expires after 120s; the session lasts at most 8h.";

#[derive(Default)]
pub struct Command {
    help: bool,
    port: u16,
    no_open: bool,
    firefox: Option<PathBuf>,
    session_file: Option<PathBuf>,
}
pub fn parse(args: &[String]) -> Result<Command> {
    let mut c = Command::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut i = 1;
    while i < args.len() {
        let option = args[i].as_str();
        if !seen.insert(option) {
            return Err(Error::input("duplicate displaytest option"));
        }
        match option {
            "--help" | "-h" => c.help = true,
            "--no-open" => c.no_open = true,
            "--port" | "--firefox" | "--session-file" => {
                i += 1;
                let value = args
                    .get(i)
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| Error::input("displaytest option requires a value"))?;
                match option {
                    "--port" => {
                        if !value.bytes().all(|b| b.is_ascii_digit()) {
                            return Err(Error::input("displaytest port must be 0..65535"));
                        }
                        c.port = value
                            .parse()
                            .map_err(|_| Error::input("displaytest port must be 0..65535"))?;
                    }
                    "--firefox" => c.firefox = Some(PathBuf::from(value)),
                    _ => c.session_file = Some(PathBuf::from(value)),
                }
            }
            _ => {
                return Err(Error::input(
                    "unsupported displaytest argument; input PNG is not migrated yet; see --help",
                ))
            }
        }
        i += 1;
    }
    Ok(c)
}
pub fn run(c: Command, cancelled: &Arc<AtomicUsize>) -> Result<i32> {
    if c.help {
        println!("{HELP}");
        return Ok(0);
    }
    let firefox = if c.no_open {
        None
    } else {
        Some(browser::discover(c.firefox.as_deref())?)
    };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, c.port))?;
    listener.set_nonblocking(true)?;
    let (gate, secret) =
        Gateway::with_display_test(listener.local_addr()?).map_err(Error::input)?;
    let url = format!("{}/#bootstrap={}", gate.origin(), secret.expose());
    let session = SessionFile::create(
        c.session_file,
        &json!({
            "url":url,"origin":gate.origin(),"bundle":BUNDLE,"pid":std::process::id(),"mode":"display-test"
        }),
    )?;
    let mut browser = firefox
        .map(|p| browser::Browser::start(&p, &session.directory, &url))
        .transpose()?;
    eprintln!("[floe2-web] display diagnostics: {}", gate.origin());
    eprintln!(
        "[floe2-web] private session link: {} (one use, expires in 120s)",
        session.path.display()
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut browser_error = None;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        transport::serve(listener, gate, async {
            while cancelled.load(Ordering::Relaxed) == 0 {
                if let Some(browser) = &browser {
                    match browser.exited() {
                        Ok(true) => break,
                        Err(e) => {
                            browser_error = Some(e);
                            break;
                        }
                        Ok(false) => (),
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
    })?;
    if let Some(browser) = &mut browser {
        browser.close()?;
    }
    if let Some(e) = browser_error {
        return Err(e);
    }
    drop(session);
    let signal = cancelled.load(Ordering::Relaxed);
    Ok(if signal == 0 { 0 } else { 128 + signal as i32 })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command(args: &[&str]) -> Result<Command> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn independent_diagnostic_has_only_launch_options() {
        assert!(command(&["displaytest"]).is_ok());
        let c = command(&[
            "displaytest",
            "--no-open",
            "--port",
            "0",
            "--session-file",
            "space 한글.json",
        ])
        .unwrap();
        assert!(c.no_open);
        assert_eq!(c.port, 0);
        for args in [
            vec!["displaytest", "input.png"],
            vec!["displaytest", "--jobs", "8"],
            vec!["displaytest", "--port"],
            vec!["displaytest", "--port", "65536"],
            vec!["displaytest", "--port", "-1"],
            vec!["displaytest", "--port", "+1"],
            vec!["displaytest", "--session-file", ""],
            vec!["displaytest", "--no-open", "--no-open"],
        ] {
            assert!(command(&args).is_err(), "{args:?}");
        }
    }
}
