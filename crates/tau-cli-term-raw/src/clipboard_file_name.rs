//! Basename validation for explicit local copied-file exports.

/// A bounded UTF-8 basename, never a path on either host.
pub struct ClipboardFileName {
    /// Validated per-invocation name, not persistent artifact metadata.
    name: String,
}

impl ClipboardFileName {
    /// Rejects unsafe names rather than silently changing the requested name.
    pub fn new(name: String) -> Result<Self, &'static str> {
        if name.is_empty()
            || name.len() > 255
            || matches!(name.as_str(), "." | "..")
            || name
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
        {
            return Err("File name must be a safe UTF-8 basename of 1–255 bytes");
        }
        Ok(Self { name })
    }

    /// Borrows the exact name to encode in the copied-file bundle.
    pub fn as_str(&self) -> &str {
        &self.name
    }
}
