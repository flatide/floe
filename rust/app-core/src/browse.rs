//! Bounded file-picker catalogue, separate from registered layout metadata.
//! Construct on a blocking actor using roots supplied by the trusted launcher.
//! Browser requests use issued handles only; no path strings or file contents
//! are exposed. This module does not index, register, render, or write files.
#[cfg(test)]
mod tests;
mod unix;
use crate::{check_cancelled, registered::AccessScope, Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc},
};
use unix::Stamp;

pub const PAGE_ROWS: usize = 128;
const HANDLES: usize = 2048;
const MATCHES: usize = 100_000;
const EXAMINED: usize = 1_000_000;
const DEPTH: usize = 64;
const PATH_BYTES: usize = 4096;

fn id() -> Result<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| Error::new(ErrorKind::Io, "picker entropy unavailable"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn changed() -> Error {
    Error::new(
        ErrorKind::Cache,
        "picker entry changed; refresh the directory",
    )
}
fn expired() -> Error {
    Error::new(
        ErrorKind::Cache,
        "picker handle expired; reopen the directory",
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Crumb {
    pub handle: String,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    pub handle: Option<String>,
    pub name: String,
    pub kind: &'static str,
    pub bytes: Option<String>,
    pub modified_seconds: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Page {
    pub snapshot: String,
    pub directory: String,
    pub breadcrumbs: Vec<Crumb>,
    pub start: usize,
    pub next: Option<usize>,
    pub total: usize,
    pub rows: Vec<Row>,
    pub skipped_names: usize,
    pub skipped_links: usize,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
    Layouts,
    Jobdecks,
    AllFiles,
    DrcFiles,
}
impl Filter {
    fn matches(self, name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        match self {
            Self::Layouts => n.ends_with(".oas") || n.ends_with(".oasis"),
            Self::Jobdecks => n.ends_with(".jb"),
            Self::AllFiles | Self::DrcFiles => true,
        }
    }
}
struct Root {
    path: PathBuf,
    file: File,
    stamp: Stamp,
    crumb: Crumb,
}
#[derive(Clone)]
struct Step {
    name: OsString,
    stamp: Stamp,
}
#[derive(Clone)]
struct Node {
    root: usize,
    steps: Vec<Step>,
}
struct Listing {
    id: String,
    node: Node,
    stamp: Stamp,
    file: File,
    names: Vec<(String, bool)>,
    skipped_names: usize,
    skipped_links: usize,
    pages: VecDeque<Page>,
}
pub struct Browser {
    scope: Arc<AccessScope>,
    roots: Vec<Root>,
    handles: BTreeMap<String, Node>,
    order: VecDeque<String>,
    listing: Option<Listing>,
    max_matches: usize,
    max_examined: usize,
}
impl Browser {
    /// Only local launcher configuration may call this with paths. The same
    /// roots bound later registration/dependency resolution, not only listing.
    pub fn new(paths: &[PathBuf]) -> Result<Self> {
        let scope = AccessScope::new(paths)?;
        let mut roots = Vec::new();
        for path in paths {
            let path = std::fs::canonicalize(path)?;
            scope.check(&path)?;
            if roots.iter().any(|r: &Root| r.path == path) {
                continue;
            }
            let file = unix::root(&path)?;
            let stamp = unix::stamp(&file)?;
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| Error::input("picker root needs a UTF-8 name"))?;
            let crumb = Crumb {
                handle: id()?,
                name: format!("{} · {name}", roots.len() + 1),
            };
            roots.push(Root {
                path,
                file,
                stamp,
                crumb,
            });
        }
        Ok(Self {
            scope,
            roots,
            handles: BTreeMap::new(),
            order: VecDeque::new(),
            listing: None,
            max_matches: MATCHES,
            max_examined: EXAMINED,
        })
    }
    pub fn roots(&self) -> Vec<Crumb> {
        self.roots.iter().map(|r| r.crumb.clone()).collect()
    }
    fn node(&self, handle: &str) -> Result<Node> {
        if let Some(root) = self.roots.iter().position(|r| r.crumb.handle == handle) {
            return Ok(Node {
                root,
                steps: vec![],
            });
        }
        self.handles.get(handle).cloned().ok_or_else(expired)
    }
    fn issue(&mut self, node: Node) -> Result<String> {
        if node.steps.is_empty() {
            return Ok(self.roots[node.root].crumb.handle.clone());
        }
        let token = id()?;
        if self.order.len() == HANDLES {
            self.handles.remove(&self.order.pop_front().unwrap());
        }
        self.order.push_back(token.clone());
        self.handles.insert(token.clone(), node);
        Ok(token)
    }
    fn open(&self, node: &Node, stop: &AtomicUsize) -> Result<File> {
        check_cancelled(stop)?;
        let root = &self.roots[node.root];
        // Never switch to a newly substituted root, even if it has the same
        // lexical pathname. Reopen from / without following any component.
        let mut file = unix::root(&root.path)?;
        if !root.stamp.same_node(unix::stamp(&file)?)
            || !root.stamp.same_node(unix::stamp(&root.file)?)
        {
            return Err(changed());
        }
        for step in &node.steps {
            check_cancelled(stop)?;
            file = unix::open(&file, &step.name, step.stamp.is_dir())?;
            if !step.stamp.same_node(unix::stamp(&file)?) {
                return Err(changed());
            }
        }
        Ok(file)
    }
    /// One cancellable scan per directory/filter. Memory is bounded; an overly
    /// broad directory fails explicitly (use a narrower name filter), never
    /// returns an apparently complete truncated catalogue. Sort matches once,
    /// not a full re-scan for each 128-row page.
    pub fn list(
        &mut self,
        directory: &str,
        filter: Filter,
        query: &str,
        stop: &AtomicUsize,
    ) -> Result<Page> {
        if query.len() > 128 || query.contains(['/', '\0']) || query.chars().any(char::is_control) {
            return Err(Error::input("invalid picker name filter"));
        }
        let node = self.node(directory)?;
        let file = self.open(&node, stop)?;
        let stamp = unix::stamp(&file)?;
        if !stamp.is_dir() {
            return Err(Error::input("picker entry is not a directory"));
        }
        let mut reader = unix::Reader::new(&file)?;
        let (mut names, mut examined, mut skipped_names, mut skipped_links) = (Vec::new(), 0, 0, 0);
        let query = query.to_lowercase();
        while let Some(name) = reader.next()? {
            check_cancelled(stop)?;
            if name == "." || name == ".." {
                continue;
            }
            examined += 1;
            if examined > self.max_examined {
                return Err(Error::new(
                    ErrorKind::Busy,
                    "picker scan limit; use a smaller approved root",
                ));
            }
            let Some(text) = name.to_str() else {
                skipped_names += 1;
                continue;
            };
            // GTK's custom chooser hides caches/ICE sidecars even in All files.
            // Do not advertise private/session dotfiles or descend into caches.
            let lower = text.to_lowercase();
            if text.starts_with('.')
                || lower.ends_with(".floe")
                || lower.ends_with(".ice") && !matches!(filter, Filter::DrcFiles)
                || !lower.contains(&query)
            {
                continue;
            }
            let s = match unix::at(&file, &name) {
                Ok(s) => s,
                Err(_) => {
                    skipped_names += 1;
                    continue;
                }
            };
            if !(s.is_dir() || s.is_file()) {
                skipped_links += 1;
                continue;
            }
            if lower.ends_with(".ice") && s.is_dir() {
                continue;
            }
            if !s.is_dir() && !filter.matches(text) {
                continue;
            }
            if names.len() == self.max_matches {
                return Err(Error::new(
                    ErrorKind::Busy,
                    "picker match limit; narrow the name filter",
                ));
            }
            names.push((text.to_owned(), s.is_dir()));
        }
        check_cancelled(stop)?;
        if stamp != unix::stamp(&file)? {
            return Err(changed());
        }
        // Cache the folded key once; large catalogues must not allocate a
        // lowercase string for every comparison in the sort.
        names.sort_by_cached_key(|(name, dir)| (!*dir, name.to_lowercase(), name.clone()));
        check_cancelled(stop)?;
        if stamp != unix::stamp(&file)? {
            return Err(changed());
        }
        let token = id()?;
        self.listing = Some(Listing {
            id: token.clone(),
            node,
            stamp,
            file,
            names,
            skipped_names,
            skipped_links,
            pages: VecDeque::new(),
        });
        self.page(&token, 0, stop)
    }
    pub fn page(&mut self, snapshot: &str, start: usize, stop: &AtomicUsize) -> Result<Page> {
        let mut listing = self.listing.take().ok_or_else(expired)?;
        let result = self.page_inner(&mut listing, snapshot, start, stop);
        self.listing = Some(listing);
        result
    }
    fn page_inner(
        &mut self,
        l: &mut Listing,
        snapshot: &str,
        start: usize,
        stop: &AtomicUsize,
    ) -> Result<Page> {
        check_cancelled(stop)?;
        if l.id != snapshot {
            return Err(expired());
        }
        if !start.is_multiple_of(PAGE_ROWS) || (start != 0 && start >= l.names.len()) {
            return Err(Error::input("invalid picker page"));
        }
        if l.stamp != unix::stamp(&l.file)?
            || !l.stamp.same_node(unix::stamp(&self.open(&l.node, stop)?)?)
        {
            return Err(changed());
        }
        if let Some(p) = l.pages.iter().find(|p| p.start == start) {
            // A replay must not return handles already evicted by other pages.
            if p.rows
                .iter()
                .filter_map(|r| r.handle.as_ref())
                .all(|id| self.handles.contains_key(id))
                && p.breadcrumbs.iter().all(|c| self.node(&c.handle).is_ok())
            {
                return Ok(p.clone());
            }
        }
        let mut breadcrumbs = vec![self.roots[l.node.root].crumb.clone()];
        let mut parent = Node {
            root: l.node.root,
            steps: vec![],
        };
        for step in &l.node.steps {
            parent.steps.push(step.clone());
            breadcrumbs.push(Crumb {
                handle: self.issue(parent.clone())?,
                name: step.name.to_str().unwrap().to_owned(),
            });
        }
        let mut rows = Vec::new();
        let end = start.saturating_add(PAGE_ROWS).min(l.names.len());
        for (name, directory) in &l.names[start..end] {
            check_cancelled(stop)?;
            let stamp = unix::at(&l.file, OsString::from(name).as_os_str())?;
            if stamp.is_dir() != *directory || !(stamp.is_dir() || stamp.is_file()) {
                return Err(changed());
            }
            let mut node = l.node.clone();
            node.steps.push(Step {
                name: name.into(),
                stamp,
            });
            let path_bytes = node.steps.iter().map(|s| s.name.len() + 1).sum::<usize>()
                + self.roots[node.root].path.as_os_str().len();
            let handle = if node.steps.len() > DEPTH || path_bytes > PATH_BYTES {
                None
            } else {
                Some(self.issue(node)?)
            };
            rows.push(Row {
                handle,
                name: name.clone(),
                kind: if *directory { "directory" } else { "file" },
                bytes: if *directory {
                    None
                } else {
                    Some(stamp.size.max(0).to_string())
                },
                modified_seconds: Some(stamp.modified.0.to_string()),
            });
        }
        if l.stamp != unix::stamp(&l.file)? {
            return Err(changed());
        }
        let page = Page {
            snapshot: l.id.clone(),
            directory: breadcrumbs.last().unwrap().handle.clone(),
            breadcrumbs,
            start,
            next: if end < l.names.len() { Some(end) } else { None },
            total: l.names.len(),
            rows,
            skipped_names: l.skipped_names,
            skipped_links: l.skipped_links,
        };
        if l.pages.len() == 2 {
            l.pages.pop_front();
        }
        l.pages.push_back(page.clone());
        Ok(page)
    }
    /// Return a server-only selection witness; never serialize it. Revalidate
    /// immediately before the registration transaction commits. It pins the
    /// chosen inode but does not pretend subsequent native path I/O is an OS
    /// sandbox or an immutable dataset revision.
    pub fn select(&self, handle: &str, stop: &AtomicUsize) -> Result<SelectedFile> {
        let node = self.node(handle)?;
        let last = node
            .steps
            .last()
            .ok_or_else(|| Error::input("select a regular file"))?;
        if !last.stamp.is_file() {
            return Err(Error::input("select a regular file"));
        }
        let file = self.open(&node, stop)?;
        if last.stamp != unix::stamp(&file)? {
            return Err(changed());
        }
        let root = &self.roots[node.root];
        let mut path = root.path.clone();
        for step in &node.steps {
            path.push(&step.name);
        }
        self.scope.check(&path)?;
        Ok(SelectedFile {
            path,
            scope: Arc::clone(&self.scope),
            file,
            stamp: last.stamp,
            root: root.path.clone(),
            root_stamp: root.stamp,
            steps: node.steps,
        })
    }
}

pub struct SelectedFile {
    path: PathBuf,
    scope: Arc<AccessScope>,
    file: File,
    stamp: Stamp,
    root: PathBuf,
    root_stamp: Stamp,
    steps: Vec<Step>,
}
impl SelectedFile {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn scope(&self) -> Arc<AccessScope> {
        Arc::clone(&self.scope)
    }
    pub fn validate(&self, stop: &AtomicUsize) -> Result<()> {
        check_cancelled(stop)?;
        self.scope.check(&self.path)?;
        let mut file = unix::root(&self.root)?;
        if !self.root_stamp.same_node(unix::stamp(&file)?) {
            return Err(changed());
        }
        for step in &self.steps {
            check_cancelled(stop)?;
            file = unix::open(&file, &step.name, step.stamp.is_dir())?;
            if !step.stamp.same_node(unix::stamp(&file)?) {
                return Err(changed());
            }
        }
        if self.stamp != unix::stamp(&file)? || self.stamp != unix::stamp(&self.file)? {
            return Err(changed());
        }
        Ok(())
    }
}
