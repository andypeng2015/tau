use super::*;

#[test]
fn agent_completer_offers_subcommands_first() {
    // `/agent` is now a command group; the first argument must guide users
    // to the concrete action instead of switching immediately.
    let completer = build_agent_arg_completer(
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Default::default())),
        Arc::new(Mutex::new(Default::default())),
    );

    let completions = completer(&[""]);

    let values: Vec<_> = completions.iter().map(|item| item.value.as_str()).collect();
    assert_eq!(values, vec!["new", "switch", "suspend", "resume"]);
}

#[test]
fn session_completer_offers_new_subcommand() {
    // `/session new` is the session-level fresh-start command; `/new` is
    // reserved as an alias for `/agent new`.
    let completer = build_session_arg_completer();

    let values: Vec<_> = completer(&[""])
        .into_iter()
        .map(|item| item.value)
        .collect();

    assert_eq!(values, vec!["new"]);
}

#[test]
fn agent_new_takes_no_agent_id_completion() {
    // `/agent new` only clears the selected agent; it must not offer or
    // accept an agent-id argument like switch/suspend/resume do.
    let completer = build_agent_arg_completer(
        Arc::new(Mutex::new(vec!["worker".to_owned()])),
        Arc::new(Mutex::new(std::collections::HashSet::from([
            "worker".to_owned()
        ]))),
        Arc::new(Mutex::new(Default::default())),
    );

    assert!(completer(&["new", ""]).is_empty());
}

#[test]
fn agent_suspend_resume_updates_prompt_routing_state_synchronously() {
    // Regression: `/agent suspend` and `/agent resume` are initiated by the
    // input thread, while the renderer applies the UI command later. Mirror
    // the state immediately so a prompt entered on the next line observes
    // the updated active/suspended sets without racing the renderer thread.
    let live = Arc::new(Mutex::new(std::collections::HashSet::from([
        "worker".to_owned()
    ])));
    let suspended = Arc::new(Mutex::new(std::collections::HashSet::new()));

    mark_agent_suspended(&suspended, "worker");
    assert!(!agent_is_active_in_sets(
        &live.lock().expect("live agents lock poisoned"),
        &suspended.lock().expect("suspended agents lock poisoned"),
        "worker"
    ));

    mark_agent_resumed(&live, &suspended, "worker");
    assert!(agent_is_active_in_sets(
        &live.lock().expect("live agents lock poisoned"),
        &suspended.lock().expect("suspended agents lock poisoned"),
        "worker"
    ));
}

#[test]
fn agent_mention_completer_offers_only_active_agents() {
    // Prompt-text `@agent` completion is for routing to active agents. It
    // must not suggest suspended agents even though `/agent resume` does.
    let known = Arc::new(Mutex::new(vec!["helper".to_owned(), "worker".to_owned()]));
    let live = Arc::new(Mutex::new(std::collections::HashSet::from([
        "helper".to_owned(),
        "worker".to_owned(),
    ])));
    let suspended = Arc::new(Mutex::new(std::collections::HashSet::from([
        "helper".to_owned()
    ])));
    let completer = build_agent_mention_completer(known, live, suspended);

    let values: Vec<_> = completer(&[""])
        .into_iter()
        .map(|item| item.value)
        .collect();

    assert_eq!(values, vec!["worker"]);
}

#[test]
fn agent_completer_filters_active_and_suspended_agents() {
    // Suspended delegate agents should disappear from switch/suspend menus
    // but remain available for explicit resume.
    let known = Arc::new(Mutex::new(vec!["helper".to_owned(), "worker".to_owned()]));
    let live = Arc::new(Mutex::new(std::collections::HashSet::from([
        "helper".to_owned(),
        "worker".to_owned(),
    ])));
    let suspended = Arc::new(Mutex::new(std::collections::HashSet::from([
        "helper".to_owned()
    ])));
    let completer = build_agent_arg_completer(known, live, suspended);

    let switch_values: Vec<_> = completer(&["switch", ""])
        .into_iter()
        .map(|item| item.value)
        .collect();
    let suspend_values: Vec<_> = completer(&["suspend", ""])
        .into_iter()
        .map(|item| item.value)
        .collect();
    let resume_values: Vec<_> = completer(&["resume", ""])
        .into_iter()
        .map(|item| item.value)
        .collect();

    assert_eq!(switch_values, vec!["none", "worker"]);
    assert_eq!(suspend_values, vec!["worker"]);
    assert_eq!(resume_values, vec!["helper"]);
}

fn groups() -> Vec<tau_proto::HarnessRoleGroup> {
    vec![
        tau_proto::HarnessRoleGroup {
            name: "engineer".to_owned(),
            roles: vec![
                "junior-engineer".to_owned(),
                "senior-engineer".to_owned(),
                "staff-engineer".to_owned(),
            ],
        },
        tau_proto::HarnessRoleGroup {
            name: "assistant".to_owned(),
            roles: vec!["assistant".to_owned()],
        },
        tau_proto::HarnessRoleGroup {
            name: "manager".to_owned(),
            roles: vec!["manager".to_owned()],
        },
    ]
}

#[test]
fn group_cycle_returns_to_last_runtime_role_for_group() {
    // Tab moves between groups, but returning to a group should restore the
    // role the user last used in that group during this process.
    let groups = groups();
    let mut memory = HashMap::new();
    memory.insert("engineer".to_owned(), "staff-engineer".to_owned());

    assert_eq!(
        next_role_in_groups(Some("manager"), &groups, false, &memory).as_deref(),
        Some("staff-engineer")
    );
}

#[test]
fn group_cycle_ignores_stale_runtime_group_memory() {
    // Role availability can change after startup, so stale remembered roles
    // must not win over the currently configured group contents.
    let groups = groups();
    let mut memory = HashMap::new();
    memory.insert("engineer".to_owned(), "missing-engineer".to_owned());

    assert_eq!(
        next_role_in_groups(Some("manager"), &groups, false, &memory).as_deref(),
        Some("junior-engineer")
    );
}
