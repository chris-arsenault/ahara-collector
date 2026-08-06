//! Bounded on-disk spools: reading envelopes (JSON lines) wait here until
//! their consumer pulls them. Each module gets its own spool (ADR-0007) —
//! a subdirectory under the spool root — so every consumer drains and acks
//! its own stream without touching the others. Within one spool, readings
//! append to an open segment; full segments close and queue in order; when
//! the byte cap is hit the oldest closed segment is dropped (newest data
//! wins — this is telemetry, not a ledger). Delivery is at-least-once: a
//! batch is one closed segment, deleted only when the puller acknowledges
//! it.
//!
//! Crash tolerance is structural: appends are flushed line-wise, the reader
//! ignores a torn trailing line, and acknowledgement is a file unlink.

use crate::envelope::valid_module;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct Spool {
    inner: Mutex<Inner>,
}

struct Inner {
    dir: PathBuf,
    segment_bytes: u64,
    max_bytes: u64,
    next_seq: u64,
    current_len: u64,
    pub dropped_segments: u64,
}

const CURRENT: &str = "current.jsonl";

fn segment_name(seq: u64) -> String {
    format!("seg-{seq:016}.jsonl")
}

fn parse_segment_name(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("seg-")?.strip_suffix(".jsonl")?;
    if rest.len() == 16 && rest.bytes().all(|b| b.is_ascii_digit()) {
        rest.parse().ok()
    } else {
        None
    }
}

impl Spool {
    pub fn open(dir: &Path, segment_bytes: u64, max_bytes: u64) -> std::io::Result<Spool> {
        fs::create_dir_all(dir)?;
        let mut max_seq = 0;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if let Some(seq) = entry.file_name().to_str().and_then(parse_segment_name) {
                max_seq = max_seq.max(seq);
            }
        }
        let current_len = fs::metadata(dir.join(CURRENT)).map(|m| m.len()).unwrap_or(0);
        Ok(Spool {
            inner: Mutex::new(Inner {
                dir: dir.to_path_buf(),
                segment_bytes,
                max_bytes,
                next_seq: max_seq + 1,
                current_len,
                dropped_segments: 0,
            }),
        })
    }

    /// Append lines (no trailing newlines) to the open segment, rotating and
    /// enforcing the cap as needed. Returns the number of lines written.
    pub fn append(&self, lines: &[String]) -> std::io::Result<usize> {
        if lines.is_empty() {
            return Ok(0);
        }
        let mut inner = self.inner.lock().unwrap();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(inner.dir.join(CURRENT))?;
        let mut written = 0;
        for line in lines {
            if line.is_empty() {
                continue;
            }
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
            inner.current_len += line.len() as u64 + 1;
            written += 1;
        }
        file.flush()?;
        drop(file);
        if inner.current_len >= inner.segment_bytes {
            inner.rotate()?;
        }
        inner.enforce_cap()?;
        Ok(written)
    }

    /// The oldest closed segment as a batch, rotating the open segment when
    /// nothing is closed yet so a slow trickle still drains. `None` when
    /// empty.
    pub fn next_batch(&self) -> std::io::Result<Option<(String, String)>> {
        let mut inner = self.inner.lock().unwrap();
        if inner.oldest_segment()?.is_none() && inner.current_len > 0 {
            inner.rotate()?;
        }
        let Some(name) = inner.oldest_segment()? else {
            return Ok(None);
        };
        let raw = fs::read(inner.dir.join(&name))?;
        let mut text = String::from_utf8_lossy(&raw).into_owned();
        // A torn write can leave a partial trailing line; drop it rather
        // than hand malformed data upstream.
        if !text.is_empty() && !text.ends_with('\n') {
            match text.rfind('\n') {
                Some(idx) => text.truncate(idx + 1),
                None => text.clear(),
            }
        }
        Ok(Some((name, text)))
    }

    /// Delete an acknowledged batch. The id must be a segment filename this
    /// spool produced — anything else is rejected, which also blocks path
    /// traversal from the API caller.
    pub fn ack(&self, batch_id: &str) -> std::io::Result<bool> {
        if parse_segment_name(batch_id).is_none() {
            return Ok(false);
        }
        let inner = self.inner.lock().unwrap();
        match fs::remove_file(inner.dir.join(batch_id)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub fn stats(&self) -> SpoolStats {
        let inner = self.inner.lock().unwrap();
        let mut total = inner.current_len;
        let mut segments = 0;
        if let Ok(entries) = fs::read_dir(&inner.dir) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_str()
                    .and_then(parse_segment_name)
                    .is_some()
                {
                    segments += 1;
                    total += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }
        SpoolStats {
            total_bytes: total,
            closed_segments: segments,
            dropped_segments: inner.dropped_segments,
        }
    }
}

pub struct SpoolStats {
    pub total_bytes: u64,
    pub closed_segments: u64,
    pub dropped_segments: u64,
}

/// One spool per module under a shared root. Spools open on demand when a
/// module first writes; existing subdirectories reopen at startup so
/// pending batches survive a restart even if their producer stays idle.
/// The byte caps apply per module.
pub struct SpoolSet {
    dir: PathBuf,
    segment_bytes: u64,
    max_bytes: u64,
    spools: Mutex<BTreeMap<String, Arc<Spool>>>,
}

impl SpoolSet {
    pub fn open(dir: &Path, segment_bytes: u64, max_bytes: u64) -> std::io::Result<SpoolSet> {
        fs::create_dir_all(dir)?;
        let mut spools = BTreeMap::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                if valid_module(name) {
                    spools.insert(
                        name.to_string(),
                        Arc::new(Spool::open(&entry.path(), segment_bytes, max_bytes)?),
                    );
                }
            }
        }
        Ok(SpoolSet {
            dir: dir.to_path_buf(),
            segment_bytes,
            max_bytes,
            spools: Mutex::new(spools),
        })
    }

    /// The module's spool, created if it does not exist yet. Rejects
    /// invalid module names (they would become directory names).
    pub fn for_module(&self, module: &str) -> std::io::Result<Arc<Spool>> {
        if !valid_module(module) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid module name {module:?}"),
            ));
        }
        let mut spools = self.spools.lock().unwrap();
        if let Some(spool) = spools.get(module) {
            return Ok(Arc::clone(spool));
        }
        let spool = Arc::new(Spool::open(
            &self.dir.join(module),
            self.segment_bytes,
            self.max_bytes,
        )?);
        spools.insert(module.to_string(), Arc::clone(&spool));
        Ok(spool)
    }

    /// The module's spool if it already exists — the read path never
    /// creates directories.
    pub fn get(&self, module: &str) -> Option<Arc<Spool>> {
        self.spools.lock().unwrap().get(module).cloned()
    }

    /// Aggregate stats across every module's spool.
    pub fn stats(&self) -> SpoolStats {
        let spools: Vec<Arc<Spool>> = self.spools.lock().unwrap().values().cloned().collect();
        let mut total = SpoolStats {
            total_bytes: 0,
            closed_segments: 0,
            dropped_segments: 0,
        };
        for spool in spools {
            let stats = spool.stats();
            total.total_bytes += stats.total_bytes;
            total.closed_segments += stats.closed_segments;
            total.dropped_segments += stats.dropped_segments;
        }
        total
    }
}

impl Inner {
    fn rotate(&mut self) -> std::io::Result<()> {
        if self.current_len == 0 {
            return Ok(());
        }
        let name = segment_name(self.next_seq);
        fs::rename(self.dir.join(CURRENT), self.dir.join(name))?;
        self.next_seq += 1;
        self.current_len = 0;
        Ok(())
    }

    fn oldest_segment(&self) -> std::io::Result<Option<String>> {
        let mut oldest: Option<(u64, String)> = None;
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                if let Some(seq) = parse_segment_name(name) {
                    if oldest.as_ref().is_none_or(|(s, _)| seq < *s) {
                        oldest = Some((seq, name.to_string()));
                    }
                }
            }
        }
        Ok(oldest.map(|(_, name)| name))
    }

    /// Drop oldest closed segments until under the cap. The open segment is
    /// never dropped (it is bounded by segment_bytes, which the validator
    /// guarantees is at most half the cap).
    fn enforce_cap(&mut self) -> std::io::Result<()> {
        loop {
            let mut total = self.current_len;
            let mut segments: Vec<(u64, String, u64)> = Vec::new();
            for entry in fs::read_dir(&self.dir)? {
                let entry = entry?;
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(seq) = parse_segment_name(name) {
                        let len = entry.metadata()?.len();
                        total += len;
                        segments.push((seq, name.to_string(), len));
                    }
                }
            }
            if total <= self.max_bytes || segments.is_empty() {
                return Ok(());
            }
            segments.sort_by_key(|(seq, _, _)| *seq);
            let (_, name, _) = &segments[0];
            fs::remove_file(self.dir.join(name))?;
            self.dropped_segments += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ahara-spool-test-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn appends_and_drains_in_order() {
        let dir = temp_dir("order");
        let spool = Spool::open(&dir, 64, 4096).unwrap();
        spool.append(&["m v=1i 1".into(), "m v=2i 2".into()]).unwrap();
        // Small segment size: the two lines exceeded 64 bytes? No — force a
        // rotation through next_batch instead.
        let (id, body) = spool.next_batch().unwrap().unwrap();
        assert_eq!(body, "m v=1i 1\nm v=2i 2\n");
        // Un-acked batches are re-served.
        let (again, _) = spool.next_batch().unwrap().unwrap();
        assert_eq!(id, again);
        assert!(spool.ack(&id).unwrap());
        assert!(spool.next_batch().unwrap().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotates_at_segment_size() {
        let dir = temp_dir("rotate");
        let spool = Spool::open(&dir, 32, 4096).unwrap();
        spool.append(&["a".repeat(40)]).unwrap();
        spool.append(&["b".repeat(40)]).unwrap();
        let stats = spool.stats();
        assert_eq!(stats.closed_segments, 2);
        let (first, body) = spool.next_batch().unwrap().unwrap();
        assert!(body.starts_with('a'));
        spool.ack(&first).unwrap();
        let (_, body) = spool.next_batch().unwrap().unwrap();
        assert!(body.starts_with('b'));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn drops_oldest_when_over_cap() {
        let dir = temp_dir("cap");
        // Cap of 200 bytes, segments of 50: writing 400 bytes must shed the
        // oldest segments.
        let spool = Spool::open(&dir, 50, 200).unwrap();
        for i in 0..8 {
            spool.append(&[format!("{}{}", i, "x".repeat(48))]).unwrap();
        }
        let stats = spool.stats();
        assert!(stats.total_bytes <= 200, "total {}", stats.total_bytes);
        assert!(stats.dropped_segments > 0);
        // The oldest surviving batch is not batch 0.
        let (_, body) = spool.next_batch().unwrap().unwrap();
        assert!(!body.starts_with('0'));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_torn_trailing_line() {
        let dir = temp_dir("torn");
        let spool = Spool::open(&dir, 1024, 4096).unwrap();
        spool.append(&["m v=1i 1".into()]).unwrap();
        // Simulate a torn write on the open segment.
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(dir.join(CURRENT))
            .unwrap();
        file.write_all(b"m v=2i").unwrap();
        drop(file);
        let reopened = Spool::open(&dir, 1024, 4096).unwrap();
        let (_, body) = reopened.next_batch().unwrap().unwrap();
        assert_eq!(body, "m v=1i 1\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_bad_ack_ids() {
        let dir = temp_dir("ack");
        let spool = Spool::open(&dir, 64, 4096).unwrap();
        assert!(!spool.ack("../../etc/passwd").unwrap());
        assert!(!spool.ack("seg-notanumber.jsonl").unwrap());
        assert!(!spool.ack("seg-0000000000000009.jsonl").unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spool_set_isolates_modules() {
        let dir = temp_dir("set");
        let set = SpoolSet::open(&dir, 1024, 4096).unwrap();
        set.for_module("envSensors")
            .unwrap()
            .append(&[r#"{"module":"envSensors"}"#.into()])
            .unwrap();
        set.for_module("kasa")
            .unwrap()
            .append(&[r#"{"module":"kasa"}"#.into()])
            .unwrap();

        // Draining and acking one module leaves the other untouched.
        let env = set.get("envSensors").unwrap();
        let (batch, body) = env.next_batch().unwrap().unwrap();
        assert!(body.contains("envSensors"));
        assert!(env.ack(&batch).unwrap());
        assert!(env.next_batch().unwrap().is_none());
        let kasa = set.get("kasa").unwrap();
        assert!(kasa.next_batch().unwrap().unwrap().1.contains("kasa"));

        // Unknown module on the read path: absent, not created.
        assert!(set.get("radio-433").is_none());
        assert!(!dir.join("radio-433").exists());
        // Invalid names never become directories.
        assert!(set.for_module("../escape").is_err());

        // Aggregate stats cover every module.
        assert!(set.stats().total_bytes > 0);

        // Reopen picks up existing module directories.
        drop(set);
        let reopened = SpoolSet::open(&dir, 1024, 4096).unwrap();
        assert!(reopened.get("kasa").is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resumes_sequence_after_reopen() {
        let dir = temp_dir("resume");
        {
            let spool = Spool::open(&dir, 8, 4096).unwrap();
            spool.append(&["aaaaaaaaaa".into()]).unwrap();
        }
        let spool = Spool::open(&dir, 8, 4096).unwrap();
        spool.append(&["bbbbbbbbbb".into()]).unwrap();
        let (first, _) = spool.next_batch().unwrap().unwrap();
        spool.ack(&first).unwrap();
        let (second, body) = spool.next_batch().unwrap().unwrap();
        assert!(body.starts_with('b'));
        assert_ne!(first, second);
        let _ = fs::remove_dir_all(&dir);
    }
}
