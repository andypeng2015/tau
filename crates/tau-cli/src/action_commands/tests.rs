use tau_actions::{
    ACTION_SCHEMA_VERSION, ActionArg, ActionArgKind, ActionChoice, ActionCommand, ActionSchema,
};

use super::*;

fn schema(root: &str, action_id: &str) -> ActionSchema {
    ActionSchema {
        version: ACTION_SCHEMA_VERSION,
        roots: vec![ActionCommand {
            name: root.to_owned(),
            description: format!("{root} actions"),
            action_id: None,
            args: Vec::new(),
            children: vec![ActionCommand {
                name: "list".to_owned(),
                description: "List items".to_owned(),
                action_id: Some(action_id.to_owned()),
                args: Vec::new(),
                children: Vec::new(),
            }],
        }],
    }
}

fn published(root: &str, action_id: &str, instance_id: u64) -> ActionSchemaPublished {
    ActionSchemaPublished {
        extension_name: "std-email".into(),
        instance_id: instance_id.into(),
        schema: schema(root, action_id),
    }
}

fn nested_schema() -> ActionSchema {
    ActionSchema {
        version: ACTION_SCHEMA_VERSION,
        roots: vec![ActionCommand {
            name: "/email".to_owned(),
            description: "Email approvals".to_owned(),
            action_id: None,
            args: Vec::new(),
            children: vec![
                ActionCommand {
                    name: "in".to_owned(),
                    description: "Incoming approvals".to_owned(),
                    action_id: None,
                    args: Vec::new(),
                    children: vec![
                        ActionCommand {
                            name: "open".to_owned(),
                            description: "Open incoming approval".to_owned(),
                            action_id: Some("email.in.open".to_owned()),
                            args: vec![ActionArg {
                                name: "id".to_owned(),
                                description: "Approval id".to_owned(),
                                required: true,
                                suggestions: Vec::new(),
                                kind: ActionArgKind::String,
                            }],
                            children: Vec::new(),
                        },
                        ActionCommand {
                            name: "approve".to_owned(),
                            description: "Approve incoming approvals".to_owned(),
                            action_id: Some("email.in.approve".to_owned()),
                            args: vec![ActionArg {
                                name: "ids".to_owned(),
                                description: "Approval ids".to_owned(),
                                required: true,
                                suggestions: vec![ActionChoice {
                                    value: "all".to_owned(),
                                    description: "All approvals".to_owned(),
                                }],
                                kind: ActionArgKind::RestString,
                            }],
                            children: Vec::new(),
                        },
                    ],
                },
                ActionCommand {
                    name: "out".to_owned(),
                    description: "Outgoing approvals".to_owned(),
                    action_id: None,
                    args: Vec::new(),
                    children: vec![ActionCommand {
                        name: "mode".to_owned(),
                        description: "Set outgoing mode".to_owned(),
                        action_id: Some("email.out.mode".to_owned()),
                        args: vec![ActionArg {
                            name: "mode".to_owned(),
                            description: "Mode".to_owned(),
                            required: true,
                            suggestions: Vec::new(),
                            kind: ActionArgKind::Enum {
                                values: vec![
                                    ActionChoice {
                                        value: "approve".to_owned(),
                                        description: "Approve sends".to_owned(),
                                    },
                                    ActionChoice {
                                        value: "block".to_owned(),
                                        description: "Block sends".to_owned(),
                                    },
                                ],
                            },
                        }],
                        children: Vec::new(),
                    }],
                },
            ],
        }],
    }
}

fn nested_published() -> ActionSchemaPublished {
    ActionSchemaPublished {
        extension_name: "std-email".into(),
        instance_id: 1.into(),
        schema: nested_schema(),
    }
}

#[test]
fn parses_known_dynamic_action_line() {
    let state = ActionCommandState::new(["/quit"]);
    state.apply_schema_published(&published("/email", "email.list", 1));

    let dispatch = state
        .parse_line("/email list")
        .expect("known root")
        .expect("valid action");

    assert_eq!(dispatch.extension_name, ExtensionName::from("std-email"));
    assert_eq!(dispatch.instance_id, ExtensionInstanceId::from(1));
    assert_eq!(dispatch.parsed.action_id, "email.list");
}

#[test]
fn completes_dynamic_action_subcommands_and_enum_args() {
    // Extension-published action schemas are command trees, not just root
    // commands. The completer must expose nested namespaces such as
    // `/email in` and `/email out` after the root has been typed.
    let state = ActionCommandState::new(["/quit"]);
    state.apply_schema_published(&nested_published());
    let data = tau_cli_term::CompletionData::new();
    let (commands, arg_completers) = state.dynamic_completions();
    data.set_dynamic_commands_and_arg_completers(commands, arg_completers);

    let labels = |buffer: &str| -> Vec<String> {
        tau_cli_term::completion::build_candidates(&[], &data, buffer, buffer.len())
            .into_iter()
            .map(|candidate| candidate.label)
            .collect()
    };

    assert_eq!(labels("/email "), vec!["in".to_owned(), "out".to_owned()]);
    assert_eq!(labels("/email i"), vec!["in".to_owned()]);
    assert_eq!(
        labels("/email in "),
        vec!["open".to_owned(), "approve".to_owned()]
    );
    assert_eq!(labels("/email in approve "), vec!["all".to_owned()]);
    assert_eq!(labels("/email out "), vec!["mode".to_owned()]);
    assert_eq!(
        labels("/email out mode "),
        vec!["approve".to_owned(), "block".to_owned()]
    );
}

#[test]
fn ignores_roots_that_collide_with_builtin_commands() {
    let state = ActionCommandState::new(["/quit"]);
    state.apply_schema_published(&published("/quit", "quit.dynamic", 1));

    assert!(!state.is_known_action_line("/quit list"));
    assert!(state.dynamic_completions().0.is_empty());
}

#[test]
fn removes_schema_for_exited_extension() {
    let state = ActionCommandState::new(["/quit"]);
    state.apply_schema_published(&published("/email", "email.list", 2));

    state.remove_extension(&ExtensionName::from("std-email"), 2.into());

    assert!(state.parse_line("/email list").is_none());
}
