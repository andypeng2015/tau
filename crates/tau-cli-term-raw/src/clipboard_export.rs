//! One attachment-local OSC5522 write transaction, driven by the sole writer.
//!
//! Packet admission under SharedState is the cancellation linearization point.
//! An admitted packet can finish after cancellation; no later packet can start.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

#[cfg(test)]
mod tests;

/// Non-renewable budget covering download, transmission, and acknowledgment.
const LIFETIME: Duration = Duration::from_secs(60);
/// Divisible by three: concatenated nonfinal base64 chunks have no padding.
const RAW_CHUNK: usize = 4095;

/// One reserved export; bytes appear only after complete verified download.
pub(super) struct ClipboardExport {
    /// Process-unique local reservation, never reused after revocation.
    pub(super) token: u64,
    /// Wire correlation includes process lifetime, attachment reservation.
    id: String,
    /// Fixed operation deadline.
    pub(super) deadline: Instant,
    /// Original bounded bytes, never logged.
    bytes: Vec<u8>,
    /// Small count-one file envelope; original bytes remain separately owned.
    header: Vec<u8>,
    /// Explicit file intent selects a distinct packet and reply family.
    file: bool,
    /// Base64 media declaration; never raw terminal text.
    mime: String,
    /// Whether default UTF-8 inference permits a plain-text alias.
    plain_alias: bool,
    /// Next raw byte offset.
    offset: usize,
    /// Next packet, or terminal acknowledgment wait.
    phase: Phase,
    /// One nonblocking retirement wake for the verified-download worker.
    pub(super) retired: Option<mpsc::SyncSender<()>>,
}

impl Drop for ClipboardExport {
    fn drop(&mut self) {
        if let Some(wake) = &self.retired {
            let _ = wake.try_send(());
        }
    }
}

/// Ordered phases; Commit is admitted only from the owning writer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Download,
    Start,
    Data,
    Alias,
    Commit,
    Acknowledgment,
}

impl ClipboardExport {
    /// Reserves authority before download so lifecycle changes revoke late
    /// starts.
    pub(super) fn new(now: Instant) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let token = NEXT.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            token,
            id: format!("tau-write-{}-{nanos}-{token}", std::process::id()),
            deadline: now + LIFETIME,
            bytes: Vec::new(),
            header: Vec::new(),
            file: false,
            mime: String::new(),
            plain_alias: false,
            offset: 0,
            phase: Phase::Download,
            retired: None,
        }
    }

    /// Installs verified bytes once, with bounded MIME metadata.
    pub(super) fn supply(
        &mut self,
        bytes: Vec<u8>,
        mime: &str,
        plain_alias: bool,
    ) -> Result<(), &'static str> {
        if self.phase != Phase::Download
            || bytes.len() > 16 * 1024 * 1024
            || mime.is_empty()
            || mime.len() > 256
            || !mime.is_ascii()
        {
            return Err("Invalid clipboard export payload");
        }
        self.bytes = bytes;
        self.mime = STANDARD.encode(mime);
        self.plain_alias = plain_alias;
        self.phase = Phase::Start;
        Ok(())
    }

    /// Installs one verified original with a bounded basename and no MIME hint.
    pub(super) fn supply_file(
        &mut self,
        bytes: Vec<u8>,
        name: &crate::ClipboardFileName,
    ) -> Result<(), &'static str> {
        self.supply(bytes, "application/vnd.tau.file-export-v1", false)?;
        self.file = true;
        self.header.extend_from_slice(b"TAUFILES\0");
        self.header.extend_from_slice(&1_u32.to_be_bytes());
        self.header
            .extend_from_slice(&(name.as_str().len() as u16).to_be_bytes());
        self.header.extend_from_slice(&0_u16.to_be_bytes());
        self.header
            .extend_from_slice(&(self.bytes.len() as u64).to_be_bytes());
        self.header.extend_from_slice(name.as_str().as_bytes());
        Ok(())
    }

    fn write_type(&self) -> &'static str {
        if self.file { "write-file" } else { "write" }
    }

    fn data_type(&self) -> &'static str {
        if self.file {
            "write-file-data"
        } else {
            "wdata"
        }
    }

    /// Indicates bounded pending output, excluding download and ACK waits.
    pub(super) fn ready(&self) -> bool {
        !matches!(self.phase, Phase::Download | Phase::Acknowledgment)
    }

    /// Admits and advances one packet while the terminal state lock is held.
    ///
    /// The caller must check ownership, cancellation and deadline immediately
    /// before this call. I/O occurs without holding that lock.
    pub(super) fn admit_packet(&mut self) -> Option<Vec<u8>> {
        let body = match self.phase {
            Phase::Download | Phase::Acknowledgment => return None,
            Phase::Start => {
                self.phase = Phase::Data;
                format!("type={}:id={}", self.write_type(), self.id)
            }
            Phase::Data => {
                let length = self.header.len() + self.bytes.len();
                let end = (self.offset + RAW_CHUNK).min(length);
                let mut chunk = Vec::with_capacity(end - self.offset);
                if self.offset < self.header.len() {
                    chunk.extend_from_slice(&self.header[self.offset..end.min(self.header.len())]);
                }
                if end > self.header.len() {
                    chunk.extend_from_slice(
                        &self.bytes[self.offset.saturating_sub(self.header.len())
                            ..end - self.header.len()],
                    );
                }
                let data = STANDARD.encode(chunk);
                self.offset = end;
                if end == length {
                    self.phase = if self.plain_alias {
                        Phase::Alias
                    } else {
                        Phase::Commit
                    };
                }
                format!(
                    "type={}:id={}:mime={};{data}",
                    self.data_type(),
                    self.id,
                    self.mime
                )
            }
            Phase::Alias => {
                self.phase = Phase::Commit;
                format!(
                    "type=walias:id={}:mime={};{}",
                    self.id,
                    self.mime,
                    STANDARD.encode("text/plain")
                )
            }
            Phase::Commit => {
                self.phase = Phase::Acknowledgment;
                // No further packet needs the body; release it before waiting.
                self.bytes = Vec::new();
                self.header = Vec::new();
                format!("type={}:id={}", self.data_type(), self.id)
            }
        };
        Some(format!("\x1b]5522;{body}\x1b\\").into_bytes())
    }

    /// Gives honest cancellation semantics even when the final packet is in
    /// flight.
    pub(super) fn cancelled_notice(&self) -> &'static str {
        if self.phase == Phase::Acknowledgment {
            "Clipboard export stopped; terminal outcome unknown (commit may have arrived)."
        } else {
            "Clipboard export cancelled before commit."
        }
    }

    /// Accepts only bounded, correlated writer responses, never ingress
    /// replies.
    pub(super) fn receive(&self, body: &[u8]) -> Option<&'static str> {
        if body.len() > 1024 || Instant::now() >= self.deadline {
            return None;
        }
        let text = std::str::from_utf8(body).ok()?.strip_prefix("5522;")?;
        let metadata = if self.file {
            text
        } else {
            text.strip_suffix(';').unwrap_or(text)
        };
        let mut fields = HashMap::new();
        for field in metadata.split(':') {
            let (name, value) = field.split_once('=')?;
            if fields.insert(name, value).is_some() {
                return None;
            }
        }
        if (self.file && fields.len() != 3)
            || fields.get("type") != Some(&self.write_type())
            || fields.get("id") != Some(&self.id.as_str())
        {
            return None;
        }
        match *fields.get("status")? {
            "DONE" if self.phase == Phase::Acknowledgment => Some(if self.file {
                "Terminal offered a copied file to the local clipboard (valid for one hour); destination paste not confirmed."
            } else {
                "Terminal acknowledged clipboard export."
            }),
            "EPERM" => Some("Terminal denied clipboard export permission; no fallback attempted."),
            "ENOSYS" => Some("Native clipboard export is unavailable in this terminal."),
            "EBUSY" => Some("Terminal clipboard is busy; export stopped. Retry explicitly."),
            "EINVAL" | "EFBIG" => Some("Terminal rejected clipboard export data."),
            "EIO" => Some("Terminal clipboard I/O failed; export outcome unknown."),
            _ => None,
        }
    }
}
