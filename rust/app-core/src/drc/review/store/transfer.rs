//! Portable sidecar transfer. No path discovery, upload authority, output file
//! creation, or implicit publication. Export sinks must be unpublished stages.
use super::*;
use std::os::unix::fs::FileExt;

#[derive(Debug, PartialEq, Eq)]
pub enum ExportContents {
    Notes { groups: usize, members: usize },
    Waives { waived: u64 },
}
#[derive(Debug, PartialEq, Eq)]
pub struct ExportInfo {
    pub bytes: u64,
    pub contents: ExportContents,
    pub legacy_unverified: bool,
    pub import_report: ImportReport,
}
struct Counted<W> {
    output: W,
    bytes: u64,
}
impl<W: Write> Write for Counted<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.output.write(bytes)?;
        self.bytes = self
            .bytes
            .checked_add(n as u64)
            .ok_or_else(|| std::io::Error::other("review export byte overflow"))?;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}
/// Positional reads never share/modify the cursor of a captured or imported
/// descriptor. Hash the entire original stream, including untrusted counters.
struct Scan<'a> {
    file: &'a File,
    at: u64,
    hash: Sha1,
}
impl<'a> Scan<'a> {
    fn new(file: &'a File) -> Self {
        Self {
            file,
            at: 0,
            hash: Sha1::new(),
        }
    }
}
impl Read for Scan<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let n = self.file.read_at(bytes, self.at)?;
        self.at = self
            .at
            .checked_add(n as u64)
            .ok_or_else(|| std::io::Error::other("review input offset overflow"))?;
        self.hash.update(&bytes[..n]);
        Ok(n)
    }
}
pub(super) struct ImportedWaives {
    file: File,
    stamp: Stamp,
    security: Security,
    digest: [u8; 20],
}
impl ImportedWaives {
    pub(super) fn unchanged(&self) -> Result<()> {
        if Stamp::of(&self.file.metadata()?) != self.stamp
            || Security::read(&self.file)? != self.security
        {
            return Err(conflict());
        }
        Ok(())
    }
    pub(super) fn write(
        &self,
        output: impl Write,
        layout: &Layout,
        stop: &AtomicUsize,
    ) -> Result<()> {
        self.unchanged()?;
        let mut scan = Scan::new(&self.file);
        rewrite_waives(&mut scan, output, layout, &[], stop)?;
        if <[u8; 20]>::from(scan.hash.finalize()) != self.digest {
            return Err(conflict());
        }
        self.unchanged()?;
        check_cancelled(stop)
    }
}
impl Snapshot {
    /// Serialize the expected review, never a newly adopted external sidecar.
    /// May leave a prefix in the sink on failure: do not expose/publish it until
    /// Ok and your own context/cancellation/commit checks. Does not sync a sink.
    /// Empty notes emit a fingerprinted empty file, never deletion authority.
    pub fn export(&self, output: impl Write, stop: &AtomicUsize) -> Result<ExportInfo> {
        self.current(false, stop)?;
        let mut output = Counted { output, bytes: 0 };
        let contents = match self.store.kind {
            Kind::Notes => {
                let text = self.note_text(stop)?;
                for bytes in text.as_bytes().chunks(64 * 1024) {
                    check_cancelled(stop)?;
                    output.write_all(bytes)?;
                }
                let notes = self.notes.as_ref().unwrap();
                ExportContents::Notes {
                    groups: notes.groups().count(),
                    members: notes.member_count(),
                }
            }
            Kind::Waives => {
                let stats = if let Some(c) = &self.before {
                    let mut input = Scan::new(&c.file);
                    let stats =
                        rewrite_waives(&mut input, &mut output, &self.store.layout, &[], stop)?;
                    if <[u8; 20]>::from(input.hash.finalize()) != c.digest {
                        return Err(conflict());
                    }
                    c.unchanged()?;
                    stats
                } else {
                    let pack = self.store.pack.lock().unwrap();
                    let stats = rewrite_waives(
                        pack.review_seed()?,
                        &mut output,
                        &self.store.layout,
                        &[],
                        stop,
                    )?;
                    pack.unchanged_at()?;
                    stats
                };
                ExportContents::Waives {
                    waived: stats.waived,
                }
            }
        };
        self.current(false, stop)?;
        Ok(ExportInfo {
            bytes: output.bytes,
            contents,
            legacy_unverified: self.legacy_unverified(),
            import_report: self.report.clone(),
        })
    }
    /// Whole-review REPLACE, not a bounded list of selected edits. The caller
    /// owns/authorizes the descriptor; paths/permissions are not inferred here.
    /// Validate/count once before preview and again during approved publication.
    /// Holds a descriptor + O(checks) stats, not an O(total errors) status Vec.
    /// Blocking filesystem I/O is only cancellable between 64KiB chunks.
    pub fn prepare_waives_import(
        self,
        file: File,
        stop: &AtomicUsize,
    ) -> Result<(Draft, WaiveStats)> {
        if self.store.kind != Kind::Waives {
            return Err(Error::input("not a waive snapshot"));
        }
        self.current(false, stop)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != self.store.max_bytes() {
            return Err(Error::input(
                "waive import must be a regular file of the expected length",
            ));
        }
        let stamp = Stamp::of(&metadata);
        let security = Security::read(&file)?;
        let mut scan = Scan::new(&file);
        let stats = rewrite_waives(&mut scan, std::io::sink(), &self.store.layout, &[], stop)?;
        let digest = scan.hash.finalize().into();
        let input = ImportedWaives {
            file,
            stamp,
            security,
            digest,
        };
        input.unchanged()?;
        self.current(false, stop)?;
        Ok((
            Draft {
                snapshot: self,
                change: Change::WaivesImport(input),
                expires: Instant::now() + TTL,
                accept_legacy: false,
                imported: true,
            },
            stats,
        ))
    }
}
