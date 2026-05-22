use super::*;

#[test]
fn error_chip_text_keeps_full_first_line_for_renderer_abbreviation() {
    let message = "failed to access /home/dpc/agent/.agents/skills: No such file or directory (os error 2)\nignored detail";
    let failure = ToolFailure::new(message);

    assert_eq!(
        failure.display.status_text,
        "failed to access /home/dpc/agent/.agents/skills: No such file or directory (os error 2)"
    );
    assert!(!failure.display.status_text.contains("err:"));
    assert!(!failure.display.status_text.contains('…'));
}
