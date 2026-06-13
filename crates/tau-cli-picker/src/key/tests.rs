use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{LogicalKey, PickerKey, logical_to_action, terminal_key_to_logical};

/// Verifies the central logical-key mapping so terminal and byte-stream readers
/// continue to share the same controls.
#[test]
fn logical_mapping_is_single_source_of_truth() {
    assert_eq!(logical_to_action(LogicalKey::Up), PickerKey::Up);
    assert_eq!(logical_to_action(LogicalKey::Down), PickerKey::Down);
    assert_eq!(logical_to_action(LogicalKey::Tab), PickerKey::Down);
    assert_eq!(logical_to_action(LogicalKey::BackTab), PickerKey::Up);
    assert_eq!(logical_to_action(LogicalKey::Enter), PickerKey::Enter);
    assert_eq!(logical_to_action(LogicalKey::Esc), PickerKey::Cancelled);
    assert_eq!(logical_to_action(LogicalKey::CtrlC), PickerKey::Cancelled);
    assert_eq!(logical_to_action(LogicalKey::CtrlD), PickerKey::Cancelled);
    assert_eq!(logical_to_action(LogicalKey::Char('j')), PickerKey::Down);
    assert_eq!(logical_to_action(LogicalKey::Char('k')), PickerKey::Up);
    assert_eq!(
        logical_to_action(LogicalKey::Char('q')),
        PickerKey::Cancelled
    );
    assert_eq!(logical_to_action(LogicalKey::Char(' ')), PickerKey::Ignored);
}

/// Protects terminal-event Ctrl-C/Ctrl-D decoding so terminal input keeps the
/// same cancellation behavior as the byte-stream test reader.
#[test]
fn terminal_control_chars_decode_to_logical_cancellation_keys() {
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

    assert_eq!(terminal_key_to_logical(ctrl_c), LogicalKey::CtrlC);
    assert_eq!(terminal_key_to_logical(ctrl_d), LogicalKey::CtrlD);
}

/// Ensures only documented plain character shortcuts are honored; unrelated
/// Ctrl/Alt modified characters should not navigate or cancel the picker.
#[test]
fn terminal_modified_character_shortcuts_are_ignored() {
    for key in ['j', 'k', 'q'] {
        let ctrl_key = KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL);
        let alt_key = KeyEvent::new(KeyCode::Char(key), KeyModifiers::ALT);

        assert_eq!(terminal_key_to_logical(ctrl_key), LogicalKey::Unknown);
        assert_eq!(terminal_key_to_logical(alt_key), LogicalKey::Unknown);
    }
}
