//! Only fixed UI actions cross from the native menu into the owning WebView.
pub enum Action {
    OpenLayout,
    OpenDrc,
    About,
}
impl Action {
    pub fn script(&self) -> String {
        let button = match self {
            Self::OpenLayout => "browse-open",
            Self::OpenDrc => "drc-open",
            Self::About => "about-open",
        };
        format!("({})('{button}')", include_str!("../ui/menu-action.js"))
    }
}

/// Errors are shown locally, but a malformed bootstrap URL must never be
/// echoed even when it came from invalid launcher arguments.
pub fn error_text(message: &str) -> String {
    if message.contains("#bootstrap=") {
        return "Session credential error (private link omitted). Restart the app without a session URL.".into();
    }
    let mut result: String = message
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(2048)
        .collect();
    if result.is_empty() {
        result.push_str("The desktop session could not continue.");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn errors_are_bounded_and_do_not_echo_bootstrap_credentials() {
        assert_eq!(error_text("missing 한국 설계.oas"), "missing 한국 설계.oas");
        assert_eq!(error_text("bad\u{0}path\nhelp"), "badpath\nhelp");
        assert_eq!(error_text(&"한".repeat(4096)).chars().count(), 2048);
        assert!(!error_text("").is_empty());
        assert!(!error_text("bad http://127.0.0.1:1234/#bootstrap=secret").contains("secret"));
    }
}
