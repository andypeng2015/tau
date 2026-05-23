//! Prompt interception support types.

use serde::{Deserialize, Serialize};

/// Interception priority. Lower numeric values run first.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct InterceptionPriority(i64);

impl InterceptionPriority {
    /// Create a priority from its numeric ordering value.
    #[must_use]
    pub const fn new(v: i64) -> Self {
        Self(v)
    }

    /// Return the numeric ordering value.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}
