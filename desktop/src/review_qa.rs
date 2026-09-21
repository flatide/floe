//! Explicit native QA only. No caller-supplied source, reviewer or output path.
use floe_app_core::{
    drc::{build, Pack},
    index::{IndexOptions, PreparedIndex},
    managed::{Limits, Resources},
    native::{Discovery, Indexer},
    registered::AccessScope,
    Error, Result,
};
use floe_oasis::doc::{RectRec, Rep};
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const REVIEWER: &str = "native-recovery-test";
const NOTE: &str = "Synthetic native recovery — 한글";
const DB: &str = "TOP 1000\nSYNTHETIC.SPACE\n2 2 1 Sep 21 00:00:00 2026\nSynthetic native recovery only.\np 1 4\n10000 10000\n11000 10000\n11000 11000\n10000 11000\np 2 4\n20000 10000\n21000 10000\n21000 11000\n20000 11000\n";

pub fn requested(args: &[String]) -> bool {
    args == ["--smoke-test-review-recovery"]
}

struct PreparationStop {
    flag: Arc<AtomicUsize>,
    signals: Vec<signal_hook::SigId>,
}
impl PreparationStop {
    fn new() -> Result<Self> {
        let mut stop = Self {
            flag: Arc::new(AtomicUsize::new(0)),
            signals: Vec::new(),
        };
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            stop.signals.push(signal_hook::flag::register_usize(
                signal,
                stop.flag.clone(),
                signal as usize,
            )?);
        }
        Ok(stop)
    }
    fn cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed) != 0
    }
}
impl Drop for PreparationStop {
    fn drop(&mut self) {
        for id in self.signals.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

pub struct Fixture {
    root: PathBuf,
    source: PathBuf,
    pack: PathBuf,
    before: Vec<(PathBuf, Vec<u8>)>,
}
impl Fixture {
    pub fn create() -> Result<Self> {
        let stop = PreparationStop::new()?;
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|_| Error::input("QA random directory failed"))?;
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let root = std::env::temp_dir()
            .canonicalize()?
            .join(format!("floe-native-review-{suffix}"));
        DirBuilder::new().mode(0o700).create(&root)?;
        // Retain this small synthetic folder, including on failure, for read-back.
        // This is never a session credential directory and contains no login URL.
        eprintln!(
            "[desktop-review-qa] synthetic artifacts: {}",
            root.display()
        );
        let source = root.join("synthetic.oas");
        let db = root.join("synthetic.db");
        let bytes = floe_oasis::write::write_cell(
            "TOP",
            1000.,
            &mut [RectRec {
                layer: 1,
                dt: 0,
                x: 0,
                y: 0,
                w: 40_000,
                h: 30_000,
                rep: Rep::One,
            }],
            &mut [],
        )
        .map_err(|_| Error::input("QA OASIS generation failed"))?;
        write_new(&source, &bytes)?;
        write_new(&db, DB.as_bytes())?;
        let indexer = Indexer::discover(&Discovery::local()?)?;
        let options = IndexOptions {
            jobs: 2,
            ..Default::default()
        };
        let mut index = PreparedIndex::prepare(&source, &options, indexer.clone(), &stop.flag)?
            .start_captured(&stop.flag)?;
        let end = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(code) = index.poll()? {
                if code != 0 {
                    return Err(Error::input("QA OASIS index failed"));
                }
                break;
            }
            if stop.cancelled() || Instant::now() >= end {
                index.cancel(signal_hook::consts::SIGTERM)?;
                return Err(Error::input("QA OASIS index cancelled or timed out"));
            }
            thread::sleep(Duration::from_millis(10));
        }
        if stop.cancelled() {
            return Err(Error::input("QA preparation cancelled"));
        }
        let resources = Resources::new(Limits::default())?;
        let scope = AccessScope::new(std::slice::from_ref(&root))?;
        let mut job = build::Build::start(
            &resources,
            scope,
            &db,
            build::Options {
                jobs: 2,
                force: false,
            },
            indexer,
        )?;
        let end = Instant::now() + Duration::from_secs(120);
        while !job.is_finished() {
            if stop.cancelled() || Instant::now() >= end {
                job.cancel();
                job.close()?;
                return Err(Error::input("QA DRC build cancelled or timed out"));
            }
            thread::sleep(Duration::from_millis(10));
        }
        let state = job.snapshot();
        let pack = job.output().to_owned();
        job.close()?;
        if state.phase != build::Phase::Succeeded {
            return Err(Error::input("QA DRC build failed"));
        }
        let mut before = Vec::new();
        snapshot(&root, &mut before)?;
        if stop.cancelled() {
            return Err(Error::input("QA preparation cancelled"));
        }
        Ok(Self {
            root,
            source,
            pack,
            before,
        })
    }
    pub fn arguments(&self) -> Result<Vec<String>> {
        let path = |p: &Path| {
            p.to_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::input("QA temporary directory must be UTF-8"))
        };
        Ok(vec![
            path(&self.source)?,
            "--drc".into(),
            path(&self.pack)?,
            "--drc-reviewer".into(),
            REVIEWER.into(),
            "--drc-edit-waives".into(),
            "--jobs".into(),
            "2".into(),
            "--raster-jobs".into(),
            "2".into(),
            "--budget-mb".into(),
            "256".into(),
        ])
    }
    pub fn verify(&self) -> Result<()> {
        for (path, bytes) in &self.before {
            if fs::read(path)? != *bytes {
                return Err(Error::input("native QA changed a source/pack/cache input"));
            }
        }
        let notes = self.root.join(format!(".synthetic.db.notes.{REVIEWER}.fe"));
        let waives = self.root.join(format!(".synthetic.db.waive.{REVIEWER}"));
        let text = fs::read_to_string(&notes)?;
        if text
            .lines()
            .filter(|s| s.starts_with("floe_note="))
            .collect::<Vec<_>>()
            != vec![format!("floe_note=0|{NOTE}")]
        {
            return Err(Error::input("native QA note read-back differs"));
        }
        for path in [&notes, &waives] {
            if fs::metadata(path)?.permissions().mode() & 0o777 != 0o600 {
                return Err(Error::input("native QA review permissions differ"));
            }
        }
        let mut pack = Pack::open(&self.pack, &AtomicUsize::new(0))?;
        pack.attach_waives(&waives)?;
        if pack.waived_count(0)? != 1 {
            return Err(Error::input("native QA waive read-back differs"));
        }
        Ok(())
    }
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}
fn snapshot(root: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            snapshot(&entry.path(), out)?;
        } else if entry.file_type()?.is_file() {
            if out.len() >= 128 || entry.metadata()?.len() > 4 * 1024 * 1024 {
                return Err(Error::input("unexpectedly large QA fixture"));
            }
            out.push((entry.path(), fs::read(entry.path())?));
        } else {
            return Err(Error::input("QA fixture contains a non-regular input"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qa_never_accepts_user_paths_reviewers_or_more_flags() {
        assert!(requested(&["--smoke-test-review-recovery".into()]));
        for words in [
            vec![],
            vec!["view", "--smoke-test-review-recovery"],
            vec!["--smoke-test-review-recovery", "user.oas"],
            vec!["--smoke-test-review-recovery", "--root", "/"],
            vec!["--smoke-test-review-recovery", "--drc-reviewer", "existing"],
        ] {
            assert!(!requested(
                &words.into_iter().map(str::to_owned).collect::<Vec<_>>()
            ));
        }
    }

    #[test]
    fn fixture_writes_never_replace_an_existing_file() {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let file = std::env::temp_dir().join(format!("floe-review-write-{suffix}"));
        write_new(&file, b"synthetic").unwrap();
        assert!(write_new(&file, b"replacement").is_err());
        assert_eq!(fs::read(&file).unwrap(), b"synthetic");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_file(file).unwrap();
    }
}
