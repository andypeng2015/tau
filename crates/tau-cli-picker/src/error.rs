use std::{fmt, io};

/// Errors returned while rendering or driving an interactive picker.
#[derive(Debug)]
pub enum PickerError {
    /// Underlying terminal or stream I/O failed.
    Io(io::Error),
    /// The picker was invoked with no items.
    Empty,
    /// The picker had items, but none were selectable.
    NoEnabledItems,
    /// The user cancelled the picker with Escape, Ctrl-C, or Ctrl-D.
    Cancelled,
}

impl fmt::Display for PickerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(source) => write!(f, "I/O error: {source}"),
            Self::Empty => f.write_str("picker has no items"),
            Self::NoEnabledItems => f.write_str("picker has no enabled items"),
            Self::Cancelled => f.write_str("picker cancelled"),
        }
    }
}

impl std::error::Error for PickerError {}

impl From<io::Error> for PickerError {
    fn from(source: io::Error) -> Self {
        Self::Io(source)
    }
}
