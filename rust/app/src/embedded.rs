//! In-process desktop host boundary. No WebView dependency enters this crate.
use crate::web_view;
use floe_app_core::{Error, Result};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize},
    Arc,
};

/// Package diagnostic only: validate every original notice chunk against the
/// identity compiled into this host. No source, worker, listener or UI starts.
pub fn check_notices() -> Result<usize> {
    let cancel = AtomicUsize::new(0);
    let catalog = crate::selfcheck::notice_catalog(&cancel)?
        .ok_or_else(|| Error::input("no notice index was packaged into this build"))?;
    let mut start = Some(0);
    while let Some(at) = start {
        let listing = catalog.list(at)?;
        for file in listing.files {
            for page in 0..file.pages {
                catalog.page(file.id, page, &cancel)?;
            }
        }
        start = listing.next;
    }
    Ok(catalog.len())
}

/// A one-use credential transferred directly to the owning UI thread.
/// Deliberately not Debug/Serialize: never put this in logs, argv or a file.
pub struct Ready {
    pub origin: String,
    pub url: String,
}

pub struct Session {
    command: web_view::Command,
    launch: Option<web_view::EmbeddedLaunch>,
}
impl Session {
    /// Legacy independent host boundary (including the frozen WK host).
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut words = vec!["view".to_owned()];
        words.extend_from_slice(args);
        Ok(Self {
            command: web_view::parse_embedded(&words, true)?,
            launch: None,
        })
    }

    /// Claim the Electron namespace before folder choice or service startup.
    /// None means an existing owner accepted the request, not that it rendered.
    /// The only owner notification is a coalesced present bit, never a URL.
    pub fn electron(
        args: &[String],
        cancelled: &AtomicUsize,
        present: Arc<AtomicBool>,
    ) -> Result<Option<Self>> {
        let mut words = vec!["view".to_owned()];
        words.extend_from_slice(args);
        let command = web_view::parse_embedded(&words, false)?;
        Ok(
            web_view::claim_electron(&command, cancelled, present)?.map(|launch| Self {
                command,
                launch: Some(launch),
            }),
        )
    }

    /// A native launcher may ask for a folder before starting an empty session.
    /// Explicit source/root arguments always win; there is no live scope change.
    pub fn needs_initial_directory(&self) -> bool {
        self.command.needs_initial_directory()
    }

    pub fn set_initial_directory(&mut self, directory: &Path) -> Result<()> {
        self.command.set_initial_directory(directory)
    }

    /// Run on a worker thread. The host owns cancellation and must wait for
    /// this call to finish before exiting; render/index cleanup stays here.
    /// `ready` must enqueue the credential and return, not wait synchronously
    /// for an HTTP response: the HTTP event loop starts after the callback.
    pub fn run(
        self,
        cancelled: &Arc<AtomicUsize>,
        ready: impl FnOnce(Ready) -> Result<()> + Send + 'static,
    ) -> Result<i32> {
        web_view::run_embedded(self.command, cancelled, Box::new(ready), self.launch)
    }
}

/// Top-level navigation is restricted to the original owning root document.
/// No remote URL, file URL, guest window or alternate loopback port is trusted.
pub fn navigation_allowed(origin: &str, candidate: &str) -> bool {
    valid_origin(origin)
        && candidate
            .strip_prefix(origin)
            .is_some_and(|tail| tail == "/" || tail.strip_prefix("/#").is_some())
}

fn valid_origin(origin: &str) -> bool {
    origin.strip_prefix("http://127.0.0.1:").is_some_and(|p| {
        !p.is_empty()
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u16>()
                .is_ok_and(|port| port > 0 && port.to_string() == p)
    })
}

pub fn validate_ready(ready: &Ready) -> Result<()> {
    let valid = ready
        .url
        .strip_prefix(&format!("{}/#bootstrap=", ready.origin))
        .is_some_and(|token| {
            token.len() == 64
                && token
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
    if !valid_origin(&ready.origin) || !valid {
        return Err(Error::input("invalid embedded session credential"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_rejects_browser_and_credential_file_options() {
        for args in [
            vec!["--no-open"],
            vec!["--firefox", "/tmp/firefox"],
            vec!["--session-file", "/tmp/session.json"],
            vec!["--help"],
        ] {
            assert!(
                Session::parse(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_err()
            );
        }
        assert!(Session::parse(&[]).is_ok());
        assert!(
            Session::parse(&["--detail".into(), "high".into(), "한국 설계.oas".into()]).is_ok()
        );
    }

    #[test]
    fn only_the_owned_root_document_can_navigate() {
        let origin = "http://127.0.0.1:32123";
        for suffix in ["/", "/#", "/#bootstrap=abc"] {
            assert!(navigation_allowed(origin, &format!("{origin}{suffix}")));
        }
        for url in [
            "https://example.com/",
            "file:///tmp/source",
            "javascript:alert(1)",
            "http://localhost:32123/",
            "http://127.0.0.1:32124/",
            "http://127.0.0.1:32123.evil/",
            "http://127.0.0.1:32123@evil/",
            "http://127.0.0.1:32123/api/v1/session",
            "http://127.0.0.1:32123//evil",
            "http://127.0.0.1:32123/?query=1",
            "about:blank",
        ] {
            assert!(!navigation_allowed(origin, url), "{url}");
        }
        assert!(!navigation_allowed(
            "https://example.com",
            "https://example.com/"
        ));
    }

    #[test]
    fn native_folder_choice_is_explicit_validated_and_one_use() {
        let mut session = Session::parse(&[]).unwrap();
        assert!(session.needs_initial_directory());
        assert!(session.set_initial_directory(Path::new("/")).is_err());
        assert!(session
            .set_initial_directory(Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/Cargo.toml"
            )))
            .is_err());
        assert!(session
            .set_initial_directory(Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/Cargo.toml/missing"
            )))
            .is_err());
        assert!(session.needs_initial_directory());
        session
            .set_initial_directory(Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap();
        assert!(!session.needs_initial_directory());
        assert!(session.set_initial_directory(Path::new("/tmp")).is_err());
        for args in [vec!["design.oas"], vec!["--root", "/tmp"]] {
            let mut explicit =
                Session::parse(&args.into_iter().map(String::from).collect::<Vec<_>>()).unwrap();
            assert!(!explicit.needs_initial_directory());
            assert!(explicit.set_initial_directory(Path::new("/tmp")).is_err());
        }
    }

    #[test]
    fn bootstrap_validation_never_echoes_a_credential() {
        let origin = "http://127.0.0.1:32123";
        let mut ready = Ready {
            origin: origin.into(),
            url: format!("{origin}/#bootstrap={}", "a".repeat(64)),
        };
        assert!(validate_ready(&ready).is_ok());
        ready.url.push('x');
        let error = validate_ready(&ready).unwrap_err().to_string();
        assert!(!error.contains(&"a".repeat(64)));
        ready.origin = "http://127.0.0.1:0".into();
        assert!(validate_ready(&ready).is_err());
    }
}
