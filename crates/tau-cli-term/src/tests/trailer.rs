use std::sync::{Arc, Mutex};

use crate::{EditorContext, PROMPT_TRAILER_MARKER, append_prompt_trailer, strip_prompt_trailer};

fn ctx(ec: EditorContext) -> Arc<Mutex<EditorContext>> {
    Arc::new(Mutex::new(ec))
}

#[test]
fn no_context_returns_buffer_unchanged() {
    let out = append_prompt_trailer("hello", &ctx(EditorContext::default()));
    assert_eq!(out, "hello");
}

#[test]
fn roundtrip_strips_trailer_with_current_response() {
    let edited = append_prompt_trailer(
        "draft body",
        &ctx(EditorContext {
            current_response: Some("agent draft".to_owned()),
            last_response: None,
            previous_prompt: None,
        }),
    );
    assert!(edited.contains(PROMPT_TRAILER_MARKER));
    assert!(edited.contains("agent draft"));
    assert_eq!(strip_prompt_trailer(&edited), "draft body");
}

#[test]
fn roundtrip_strips_trailer_with_all_sections() {
    let edited = append_prompt_trailer(
        "user body",
        &ctx(EditorContext {
            current_response: Some("in progress".to_owned()),
            last_response: Some("last".to_owned()),
            previous_prompt: Some("prev".to_owned()),
        }),
    );
    assert!(edited.contains("Current response in progress"));
    assert!(edited.contains("Last response"));
    assert!(edited.contains("Previous prompt"));
    assert_eq!(strip_prompt_trailer(&edited), "user body");
}

#[test]
fn empty_section_strings_are_skipped() {
    let edited = append_prompt_trailer(
        "body",
        &ctx(EditorContext {
            current_response: Some(String::new()),
            last_response: Some("kept".to_owned()),
            previous_prompt: Some(String::new()),
        }),
    );
    assert!(!edited.contains("Current response in progress"));
    assert!(edited.contains("Last response"));
    assert!(!edited.contains("Previous prompt"));
}

#[test]
fn strip_without_marker_is_identity() {
    assert_eq!(strip_prompt_trailer("just text"), "just text");
}

#[test]
fn marker_inside_user_text_is_kept() {
    // Ensures only the generated marker line strips trailer context. If the
    // marker text appears in user-owned content, including after the generated
    // marker line was deleted in $EDITOR, it must remain part of the prompt.
    let mut user_text = String::from("body with marker: ");
    user_text.push_str(PROMPT_TRAILER_MARKER);
    user_text.push_str(" and more");
    let stripped = strip_prompt_trailer(&user_text);
    assert_eq!(stripped, user_text);
}

#[test]
fn deleting_marker_line_keeps_entire_file_as_prompt() {
    // Prevents edited prompt text from being discarded when the user removes
    // the generated marker line but leaves the informational trailer text.
    let edited = append_prompt_trailer(
        "my reply",
        &ctx(EditorContext {
            current_response: Some(format!("agent mentioned {PROMPT_TRAILER_MARKER}")),
            last_response: Some("last".to_owned()),
            previous_prompt: None,
        }),
    );
    let without_marker_line = edited
        .lines()
        .filter(|line| *line != PROMPT_TRAILER_MARKER)
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(
        strip_prompt_trailer(&without_marker_line),
        without_marker_line
    );
}
