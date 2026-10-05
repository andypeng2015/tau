use super::*;

/// All supported explicit sigil combinations must resolve identically, with
/// no discovery or accidental local interpretation of a qualified address.
#[test]
fn cli_message_recipient_forms_preserve_tool_addressing() {
    for recipient in [
        "session/agent",
        "&session/agent",
        "session/@agent",
        "&session/@agent",
    ] {
        assert_eq!(
            resolve_recipient(recipient).expect("address"),
            (
                "session".parse().expect("session"),
                ExternalAgentMessageRecipient::Exact("agent".parse().expect("agent")),
            )
        );
    }
    assert_eq!(
        resolve_recipient("&session").expect("session address"),
        (
            "session".parse().expect("session"),
            ExternalAgentMessageRecipient::BareEntrypoint,
        )
    );
    for recipient in [
        "user",
        "&",
        "/agent",
        "&session/",
        "session/agent/extra",
        "!cli-unowned",
    ] {
        assert!(resolve_recipient(recipient).is_err(), "{recipient}");
    }
}

/// Discovery never guesses among duplicates or silently routes a missing agent.
#[test]
fn cli_message_requires_unique_discovered_owner() {
    assert!(unique_owner(Vec::new()).is_err());
    let session: SessionId = "session".parse().expect("session");
    assert_eq!(
        unique_owner(vec![session.clone()]).expect("unique"),
        session
    );
    assert!(unique_owner(vec![session, "other".parse().expect("session")]).is_err());
}
