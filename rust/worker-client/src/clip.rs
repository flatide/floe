//! Exact, synchronous export on an idle, dedicated worker. No display scene,
//! styles or render generation is needed and no daemon path escapes the client.
use crate::{files, wire_path, Error, ErrorKind, Fields, Layers, Result, WorkerClient};
use std::fmt::Write;
use std::fs::File;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct ClipRequest {
    pub bbox: [i64; 4],
    pub layers: Layers,
    pub jobs: u16,
    pub cell_name: String,
}
impl ClipRequest {
    pub fn validate(&self) -> Result<()> {
        if self.bbox[0] >= self.bbox[2] || self.bbox[1] >= self.bbox[3] {
            return Err(Error::input("clip box must have positive width and height"));
        }
        if !(1..=256).contains(&self.jobs) {
            return Err(Error::input("clip jobs must be in 1..256"));
        }
        if self.cell_name.is_empty()
            || self.cell_name.len() > 4096
            || self.cell_name.chars().any(char::is_control)
        {
            return Err(Error::input(
                "clip cell name needs 1..4096 UTF-8 bytes without controls",
            ));
        }
        self.layers.wire()?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct ClipArtifact {
    /// Owned, unlinked regular file, positioned at zero. It survives worker
    /// shutdown and can be copied in bounded chunks rather than a second Vec.
    pub file: File,
    pub size_bytes: u64,
    pub fields: Fields,
}

impl WorkerClient {
    pub fn clip(&mut self, request: &ClipRequest) -> Result<ClipArtifact> {
        request.validate()?;
        let opened = self
            .opened
            .as_ref()
            .filter(|_| self.child.is_some())
            .ok_or_else(|| Error::new(ErrorKind::State, "clip requires an open worker"))?;
        if opened.is_deck {
            return Err(Error::input(
                "jobdeck clip is unsupported; clip a source layout directly",
            ));
        }
        if self.active.is_some()
            || !self.issued.is_empty()
            || !self.queries.is_empty()
            || !self.query_cancels.is_empty()
        {
            return Err(Error::new(
                ErrorKind::Busy,
                "clip requires an idle dedicated worker",
            ));
        }
        let unit = opened.unit;
        let sequence = self
            .clip_sequence
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or_else(|| Error::new(ErrorKind::State, "clip sequence exhausted"))?;
        let path = self.workspace.0.join(format!("clip-{sequence}.oas"));
        let mut name = String::with_capacity(request.cell_name.len() * 2);
        for b in request.cell_name.bytes() {
            write!(&mut name, "{b:02x}").unwrap();
        }
        let [x0, y0, x1, y1] = request.bbox;
        let command =
            format!(
            "clip seq={sequence} box={x0},{y0},{x1},{y1} layers={} jobs={} cell_hex={name} out={}",
            request.layers.wire()?, request.jobs, wire_path(&path)?
        );
        let result = (|| {
            self.check_shutdown()?;
            self.send(command)?;
            self.clip_sequence = sequence;
            let deadline = Instant::now() + self.config.clip_timeout;
            loop {
                self.check_shutdown()?;
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(Error::new(ErrorKind::Timeout, "clip deadline exceeded"));
                }
                let Some(line) = self.receive(remaining.min(Duration::from_millis(20)))? else {
                    continue;
                };
                let fields = line.fields;
                if !matches!(line.kind.as_str(), "clip" | "error") || fields.u64("seq")? != sequence
                {
                    return Err(Error::protocol("clip response kind/sequence mismatch"));
                }
                if line.kind == "error" {
                    if fields.required("code")? != "clip" {
                        return Err(Error::protocol("clip error code mismatch"));
                    }
                    return Err(Error::new(ErrorKind::Worker, fields.required("message")?));
                }
                let size_bytes = fields.u64("size_bytes")?;
                if fields.u64("rects")?.checked_add(fields.u64("polys")?)
                    != Some(fields.u64("records")?)
                {
                    return Err(Error::protocol("clip record counts mismatch"));
                }
                for key in [
                    "ms",
                    "plan_us",
                    "read_us",
                    "decode_us",
                    "clip_us",
                    "write_us",
                ] {
                    fields.u64(key)?;
                }
                self.check_shutdown()?;
                let file = files::consume_clip(&path, size_bytes, unit, &request.cell_name)?;
                return Ok(ClipArtifact {
                    file,
                    size_bytes,
                    fields,
                });
            }
        })();
        // A timed-out or malformed command must not leave a late response/file
        // available to a subsequent operation. Close also cancels native work.
        self.close_on_error(result)
    }
}
