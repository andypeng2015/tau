use super::*;

#[test]
fn help_lists_builtin_provider_specific_commands() {
    // `tau provider` is intentionally only a dispatcher now: each built-in
    // provider owns its setup UX and external providers can expose their own CLI.
    assert!(HELP_TEXT.contains("tau provider chatgpt login"));
    assert!(HELP_TEXT.contains("tau provider chat-completions add"));
}

#[test]
fn unknown_provider_is_rejected_before_dispatch() {
    // Keep typos at the dispatcher boundary instead of silently falling through
    // to a generic provider wizard.
    let args = vec!["missing".to_owned(), "login".to_owned()];

    let error = run(&args).expect_err("unknown provider should fail");

    assert!(error.to_string().contains("unknown provider: missing"));
}
