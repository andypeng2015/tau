//! System-prompt fragment support types.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// System-prompt fragment priority. Lower numeric values render first.
///
/// The built-in system templates use coarse priority bands: priorities below
/// 100 render before generated context sections such as skills, while 100 and
/// above render afterward. Use below-100 values for role/persona instructions
/// and high values for epilogue-style context. For example, the shell extension
/// publishes the current working directory at priority 900 so it stays near the
/// end of the prompt.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct PromptPriority(u16);

impl PromptPriority {
    /// Create a prompt priority from its numeric ordering value.
    #[must_use]
    pub const fn new(v: u16) -> Self {
        Self(v)
    }

    /// Return the numeric ordering value.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Text fragment inserted into a composed system prompt.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PromptContent(String);

impl PromptContent {
    /// Create prompt content from owned or borrowed text.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Borrow the prompt text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the newtype and return the owned prompt text.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }

    /// Return whether the prompt text is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::ops::Deref for PromptContent {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl From<String> for PromptContent {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for PromptContent {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl AsRef<str> for PromptContent {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Ordered collection of rendered prompt fragments.
pub type PromptFragments = BTreeSet<(PromptPriority, String, PromptContent)>;

/// One prompt fragment template contributed by a tool or extension.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct PromptFragment {
    /// Stable fragment name, preferably namespaced by producer.
    pub name: String,
    /// Priority controlling coarse placement among fragments. Lower values
    /// render first.
    pub priority: PromptPriority,
    /// Handlebars template rendered into prompt text.
    pub template: PromptContent,
}

impl PromptFragment {
    /// Create one prompt fragment template.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        priority: PromptPriority,
        template: impl Into<PromptContent>,
    ) -> Self {
        Self {
            name: name.into(),
            priority,
            template: template.into(),
        }
    }
}
