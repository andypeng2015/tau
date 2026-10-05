//! Opaque copied-file bytes, never paths to open on the Tau execution host.

use std::ops::Range;
use std::sync::Arc;

/// Versioned private MIME carried by the ordinary OSC5522 read protocol.
pub(crate) const FILES_MIME: &str = "application/vnd.tau.files-v1";
/// Existing aggregate clipboard admission bound, including bundle framing.
const MAX_BYTES: usize = 16 * 1024 * 1024;
/// Nine-byte v1 marker; its NUL is part of the literal encoding.
const MAGIC: &[u8] = b"TAUFILES\0";

/// One validated file's metadata and body positions in the shared original.
struct FileSpan {
    /// UTF-8 basename, without separators or control characters.
    filename: Range<usize>,
    /// Optional ASCII type/subtype claim, not content validation.
    mime: Range<usize>,
    /// Exact opaque file bytes.
    body: Range<usize>,
}

/// Fully validated copied files retained for explicit artifact-upload retries.
///
/// The terminal resolves workstation files. Tau never opens clipboard paths,
/// extracts archives, executes content, or infers authority from media hints.
pub struct ClipboardFiles {
    /// Complete bounded encoding; spans avoid copying each file's body.
    encoded: Arc<[u8]>,
    /// Original order, including duplicate basenames.
    files: Vec<FileSpan>,
}

impl ClipboardFiles {
    /// Validates a complete v1 bundle before admitting any file for upload.
    pub fn parse(encoded: Arc<[u8]>) -> Result<Self, &'static str> {
        if encoded.len() > MAX_BYTES || encoded.get(..MAGIC.len()) != Some(MAGIC) {
            return Err("Invalid copied-file bundle; draft unchanged.");
        }
        let mut position = MAGIC.len();
        let count = u32::from_be_bytes(
            take(&encoded, &mut position, 4)?
                .try_into()
                .expect("four-byte file count"),
        );
        // Every entry needs twelve length bytes and at least one name byte.
        // This is a framing bound, not a discretionary file-count policy.
        if count == 0 || u64::from(count) > ((encoded.len() - position) / 13) as u64 {
            return Err("Invalid copied-file count; draft unchanged.");
        }
        let mut files = Vec::new();
        for _ in 0..count {
            let filename_length = u16::from_be_bytes(
                take(&encoded, &mut position, 2)?
                    .try_into()
                    .expect("two-byte name length"),
            );
            let mime_length = u16::from_be_bytes(
                take(&encoded, &mut position, 2)?
                    .try_into()
                    .expect("two-byte MIME length"),
            );
            let body_length = u64::from_be_bytes(
                take(&encoded, &mut position, 8)?
                    .try_into()
                    .expect("eight-byte body length"),
            );
            let filename = span(&encoded, &mut position, usize::from(filename_length))?;
            let mime = span(&encoded, &mut position, usize::from(mime_length))?;
            let body = span(
                &encoded,
                &mut position,
                usize::try_from(body_length)
                    .map_err(|_| "Invalid copied-file size; draft unchanged.")?,
            )?;
            let name = std::str::from_utf8(&encoded[filename.clone()])
                .map_err(|_| "Invalid copied-file name; draft unchanged.")?;
            if name.is_empty()
                || matches!(name, "." | "..")
                || name
                    .chars()
                    .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
            {
                return Err("Invalid copied-file basename; draft unchanged.");
            }
            if !valid_mime(&encoded[mime.clone()]) {
                return Err("Invalid copied-file media hint; draft unchanged.");
            }
            files.push(FileSpan {
                filename,
                mime,
                body,
            });
        }
        if position != encoded.len() {
            return Err("Trailing copied-file bytes; draft unchanged.");
        }
        Ok(Self { encoded, files })
    }

    /// Borrows the complete encoding for aggregate admission checks.
    pub fn encoded(&self) -> &[u8] {
        &self.encoded
    }

    /// Returns the number of files in original order.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Reports whether there are no files; successful parsing always has one.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Borrows exact original bytes and untrusted per-use hints.
    pub fn file(&self, index: usize) -> Option<PasteArtifact<'_>> {
        let file = self.files.get(index)?;
        Some(PasteArtifact {
            bytes: &self.encoded[file.body.clone()],
            filename: Some(
                std::str::from_utf8(&self.encoded[file.filename.clone()])
                    .expect("validated UTF-8 basename"),
            ),
            mime_type: (!file.mime.is_empty()).then(|| {
                std::str::from_utf8(&self.encoded[file.mime.clone()])
                    .expect("validated ASCII media hint")
            }),
        })
    }
}

/// One original and its optional untrusted metadata, not filesystem authority.
pub struct PasteArtifact<'a> {
    /// Exact original bytes uploaded without transformation.
    pub bytes: &'a [u8],
    /// Basename hint; consumers must sanitize it before textual presentation.
    pub filename: Option<&'a str>,
    /// Optional type/subtype claim, never media validation.
    pub mime_type: Option<&'a str>,
}

/// Advances only when the complete requested range is present.
fn span(bytes: &[u8], position: &mut usize, length: usize) -> Result<Range<usize>, &'static str> {
    let end = position
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or("Truncated copied-file bundle; draft unchanged.")?;
    let result = *position..end;
    *position = end;
    Ok(result)
}

/// Reads one framing value without unchecked offset arithmetic.
fn take<'a>(
    bytes: &'a [u8],
    position: &mut usize,
    length: usize,
) -> Result<&'a [u8], &'static str> {
    Ok(&bytes[span(bytes, position, length)?])
}

/// Optional hints use MIME type/subtype tokens, without parameters or controls.
pub(crate) fn valid_mime(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    let Some(slash) = bytes.iter().position(|byte| *byte == b'/') else {
        return false;
    };
    let token = |value: &[u8]| {
        !value.is_empty()
            && value
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(byte))
    };
    token(&bytes[..slash]) && token(&bytes[slash + 1..])
}

#[cfg(test)]
mod tests;
