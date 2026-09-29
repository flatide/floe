//! Server deployment policies, separate from local CLI authority. These types
//! do not authenticate HTTP callers, launch workers or grant arbitrary paths.
mod config;
pub mod indexing;
mod origin;
pub mod secret;
use crate::{Error, Result};
pub use config::{Config, Deployment, IndexPolicy, Sample, ValidatedConfig};
pub use origin::HttpsOrigin;
use serde::{Deserialize, Serialize};

/// Login client and delegated subject are different identities. A trusted
/// TeeBox adapter must authenticate the client BEFORE constructing a principal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    namespace: String,
    subject: String,
}
impl Principal {
    /// Public-demo marker, not an authenticated person or a write capability.
    pub fn anonymous_demo() -> Self {
        Self {
            namespace: "public-demo".into(),
            subject: "visitor".into(),
        }
    }
    pub fn delegated(namespace: &str, subject: &str) -> Result<Self> {
        identifier(namespace)?;
        if subject.is_empty() || subject.len() > 96 || subject.chars().any(char::is_control) {
            return Err(Error::input(
                "user ID must be 1..96 UTF-8 bytes without controls",
            ));
        }
        Ok(Self {
            namespace: namespace.into(),
            subject: subject.into(),
        })
    }
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    pub fn subject(&self) -> &str {
        &self.subject
    }
    /// Injective encoding, not a display name, hash, authentication secret or
    /// OS username. Separate components prevent delimiter/Unicode collisions.
    pub fn storage_components(&self) -> (String, String) {
        let hex = |s: &str| s.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        (hex(&self.namespace), hex(&self.subject))
    }
}
pub(super) fn identifier(s: &str) -> Result<()> {
    if s.is_empty()
        || s.len() > 64
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::input(
            "identifier must be 1..64 ASCII letters, digits, underscore or hyphen",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    View,
    Browse,
    RequestIndex,
    ReadOwnReview,
    WriteReview,
    Share,
    ExportGeometry,
    PublishDefaults,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subject_is_not_a_path_or_machine_account() {
        let p = Principal::delegated("teebox", "../사용자/one@example").unwrap();
        let (issuer, subject) = p.storage_components();
        assert!(issuer
            .bytes()
            .chain(subject.bytes())
            .all(|b| b.is_ascii_hexdigit()));
        assert_ne!(
            Principal::delegated("a-b", "c")
                .unwrap()
                .storage_components(),
            Principal::delegated("a", "b-c")
                .unwrap()
                .storage_components()
        );
        assert_ne!(
            Principal::delegated("teebox", "Alice").unwrap(),
            Principal::delegated("teebox", "alice").unwrap()
        );
        assert!(Principal::delegated("teebox", "a\nb").is_err());
        assert!(Principal::delegated("teebox", &"한".repeat(33)).is_err());
        assert!(Principal::delegated("../teebox", "u").is_err());
    }
}
