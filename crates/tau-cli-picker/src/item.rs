/// One selectable row shown by the terminal picker.
#[derive(Clone, Debug)]
pub struct PickerItem {
    /// Text rendered for this row.
    pub label: String,
    /// Whether the cursor can land on and select this row.
    pub enabled: bool,
}

impl PickerItem {
    /// Creates a row that can be selected.
    pub fn enabled(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            enabled: true,
        }
    }

    /// Creates a disabled row that is shown but skipped by navigation.
    pub fn disabled(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            enabled: false,
        }
    }
}
