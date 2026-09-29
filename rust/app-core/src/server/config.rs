use super::{identifier, Action, HttpsOrigin, Principal};
use crate::{registered::AccessScope, Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

const CONFIG_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub public_origin: String,
    pub runtime_root: PathBuf,
    pub max_sessions: u16,
    pub deployment: Deployment,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Deployment {
    #[serde(rename = "teebox")]
    TeeBox {
        client_id: String,
        user_namespace: String,
        shared_root: PathBuf,
        work_root: PathBuf,
        index: IndexPolicy,
    },
    PublicDemo {
        data_root: PathBuf,
        samples: Vec<Sample>,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexPolicy {
    pub max_running: u16,
    pub max_entries: u16,
    pub jobs: u16,
}
impl IndexPolicy {
    pub fn validate(&self) -> Result<()> {
        // Leave at least four of the initial 16-slot envelope for views. This
        // is queue admission, NOT an OS CPU/RSS cap or a worker reservation.
        if self.max_running == 0
            || self.max_entries < self.max_running
            || self.max_entries > 256
            || self.jobs == 0
            || u32::from(self.max_running) * u32::from(self.jobs) > 12
        {
            return Err(Error::input(
                "index policy requires 1..256 entries and running * jobs <= 12",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub id: String,
    pub source: String,
}

/// Canonical roots are private: browser fields cannot replace them. Opening
/// still needs the registered-source checks and OS permissions at point of use.
#[derive(Debug)]
pub struct ValidatedConfig {
    config: Config,
    scope: Arc<AccessScope>,
    data_root: PathBuf,
    work_root: Option<PathBuf>,
    runtime_root: PathBuf,
}
impl Config {
    pub fn read(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > CONFIG_BYTES {
            return Err(Error::input(
                "server configuration must be a bounded regular file",
            ));
        }
        let mut bytes = Vec::new();
        file.take(CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > CONFIG_BYTES {
            return Err(Error::input("server configuration too large"));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| Error::input("invalid server configuration schema"))
    }
    /// Read-only preflight: no directory creation, source parsing, index build,
    /// credential access, listener, migration or deletion.
    pub fn validate(self) -> Result<ValidatedConfig> {
        HttpsOrigin::parse(&self.public_origin).map_err(Error::input)?;
        if self.version != 1 || self.max_sessions == 0 || self.max_sessions > 32 {
            return Err(Error::input(
                "server config version must be 1 and max_sessions 1..32",
            ));
        }
        let runtime_root = root(&self.runtime_root)?;
        let (data_root, work_root) = match &self.deployment {
            Deployment::TeeBox {
                client_id,
                user_namespace,
                shared_root,
                work_root,
                index,
            } => {
                identifier(client_id)?;
                identifier(user_namespace)?;
                index.validate()?;
                (root(shared_root)?, Some(root(work_root)?))
            }
            Deployment::PublicDemo { data_root, samples } => {
                if samples.is_empty() || samples.len() > 32 {
                    return Err(Error::input("demo requires 1..32 approved samples"));
                }
                let mut ids = BTreeSet::new();
                for sample in samples {
                    identifier(&sample.id)?;
                    relative(&sample.source)?;
                    if !ids.insert(&sample.id) {
                        return Err(Error::input("duplicate demo sample ID"));
                    }
                }
                (root(data_root)?, None)
            }
        };
        if overlap(&data_root, &runtime_root)
            || work_root
                .as_ref()
                .is_some_and(|work| overlap(work, &data_root) || overlap(work, &runtime_root))
        {
            return Err(Error::input(
                "data, personal work and runtime roots must be disjoint",
            ));
        }
        let scope = AccessScope::new(std::slice::from_ref(&data_root))?;
        let validated = ValidatedConfig {
            config: self,
            scope,
            data_root,
            work_root,
            runtime_root,
        };
        if let Deployment::PublicDemo { samples, .. } = &validated.config.deployment {
            for sample in samples {
                validated.source(&sample.source)?;
            }
        }
        Ok(validated)
    }
}
fn root(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(Error::input(
            "server roots must be explicit absolute directories",
        ));
    }
    let path = fs::canonicalize(path)?;
    if path.parent().is_none() || !path.is_dir() {
        return Err(Error::input("server root must be a non-root directory"));
    }
    Ok(path)
}
fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
fn relative(value: &str) -> Result<&Path> {
    if value.is_empty()
        || value.len() > 4096
        || value.chars().any(char::is_control)
        || value.contains('\\')
        || value.starts_with('/')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(Error::input(
            "source must be a bounded canonical relative path",
        ));
    }
    let path = Path::new(value);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::input(
            "source must stay inside its configured data root",
        ));
    }
    Ok(path)
}
impl ValidatedConfig {
    pub fn public_origin(&self) -> &str {
        &self.config.public_origin
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }
    pub fn mode(&self) -> &'static str {
        match self.config.deployment {
            Deployment::TeeBox { .. } => "teebox",
            Deployment::PublicDemo { .. } => "public_demo",
        }
    }
    pub fn allows(&self, action: Action) -> bool {
        match self.config.deployment {
            Deployment::TeeBox { .. } => matches!(
                action,
                Action::View
                    | Action::Browse
                    | Action::RequestIndex
                    | Action::ReadOwnReview
                    | Action::WriteReview
                    | Action::Share
            ),
            Deployment::PublicDemo { .. } => action == Action::View,
        }
    }
    /// `client` must come from authenticated proxy/launcher context, NEVER a
    /// browser user field. Matching its name here is not authentication.
    pub fn principal(&self, client: &str, subject: &str) -> Result<Principal> {
        match &self.config.deployment {
            Deployment::TeeBox {
                client_id,
                user_namespace,
                ..
            } if client == client_id => Principal::delegated(user_namespace, subject),
            _ => Err(Error::input("client is not permitted to delegate users")),
        }
    }
    fn source(&self, value: &str) -> Result<PathBuf> {
        let path = self.scope.check(&self.data_root.join(relative(value)?))?;
        // Pin the resolved spelling as well: an in-root symlink is not a new
        // dataset identity, and resolving it must not expand the approved root.
        let path = self.scope.check(&fs::canonicalize(path)?)?;
        if !fs::metadata(&path)?.is_file() {
            return Err(Error::input("source must be a regular file"));
        }
        Ok(path)
    }
    /// Input is a data-root-relative path for TeeBox, an allowlisted sample ID
    /// for anonymous demo. Demo requests never choose a filesystem path.
    pub fn resolve_source(&self, selection: &str) -> Result<PathBuf> {
        match &self.config.deployment {
            Deployment::TeeBox { .. } => self.source(selection),
            Deployment::PublicDemo { samples, .. } => {
                let sample = samples
                    .iter()
                    .find(|s| s.id == selection)
                    .ok_or_else(|| Error::input("unknown demo sample"))?;
                self.source(&sample.source)
            }
        }
    }
    /// Read-only server registration with the SAME configured root. Deck TC
    /// and cache dependencies must also stay in scope, including hidden levels.
    pub fn register_source(
        &self,
        selection: &str,
        stop: &std::sync::atomic::AtomicUsize,
    ) -> Result<Arc<crate::registered::RegisteredSource>> {
        crate::check_cancelled(stop)?;
        let path = self.resolve_source(selection)?;
        crate::registered::RegisteredSource::register(Arc::clone(&self.scope), &path, stop)
    }
    /// Stable namespace independent of cache revision. No files are created.
    /// Actual review writes must additionally bind and validate the DRC pack.
    pub fn personal_directory(&self, principal: &Principal, dataset: &str) -> Result<PathBuf> {
        let Deployment::TeeBox { user_namespace, .. } = &self.config.deployment else {
            return Err(Error::input("demo has no personal work storage"));
        };
        if principal.namespace() != user_namespace {
            return Err(Error::input("principal namespace mismatch"));
        }
        identifier(dataset)?;
        let (namespace, subject) = principal.storage_components();
        let directory = self
            .work_root
            .as_ref()
            .expect("tee box work root")
            .join("users")
            .join(namespace)
            .join(subject)
            .join(dataset);
        // A writable namespace must not follow a link into another user's
        // directory, even if that target is still inside the shared work root.
        // Publication must recheck under its own directory/lease protections;
        // preflight alone is not a TOCTOU-safe filesystem write capability.
        let base = self.work_root.as_ref().expect("tee box work root");
        let mut parent = base.clone();
        for part in directory
            .strip_prefix(base)
            .expect("constructed below base")
            .components()
        {
            parent.push(part);
            match fs::symlink_metadata(&parent) {
                Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                    return Err(Error::input(
                        "personal namespace must contain only real directories",
                    ));
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(directory)
    }
}

#[cfg(test)]
mod tests;
