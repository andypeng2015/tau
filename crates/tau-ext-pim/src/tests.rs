use std::collections::BTreeMap;

use serde::Deserialize;
use tau_proto::{EventName, EventSelector, ToolName};

use super::*;

#[test]
fn self_knowledge_pim_example_matches_extension_config_shape() {
    #[derive(Deserialize)]
    struct HarnessExample {
        extensions: BTreeMap<String, ExtensionExample>,
    }

    #[derive(Deserialize)]
    struct ExtensionExample {
        config: PimExtensionConfig,
    }

    let mut harness: HarnessExample =
        serde_yaml_ng::from_str(include_str!("../config/self-knowledge.harness.yaml"))
            .expect("self-knowledge PIM example parses as YAML");
    let pim = harness
        .extensions
        .remove("std-pim")
        .expect("std-pim example exists")
        .config;

    pim.email
        .expect("email example")
        .validate()
        .expect("email config validates");
    pim.calendar
        .expect("calendar example")
        .validate()
        .expect("calendar config validates");
}

#[test]
fn action_schema_contains_email_and_calendar_roots() {
    let roots = action_schema()
        .roots
        .into_iter()
        .map(|root| root.name)
        .collect::<Vec<_>>();

    assert_eq!(roots, vec!["/email", "/calendar"]);
}

/// PIM subscribes to `tool.started` to receive its own email/calendar
/// calls, but the harness event stream can also contain starts for
/// tools owned by other extensions. Those foreign calls must be ignored
/// instead of producing terminal tool errors that race with the real
/// provider result.
#[test]
fn ignores_tool_started_for_tools_owned_by_other_extensions() {
    let mut runtime = RuntimeState::default();
    let invoke = tau_proto::ToolStarted {
        call_id: tau_proto::ToolCallId::new("call-read"),
        tool_name: tau_proto::ToolName::new("read"),
        arguments: CborValue::Map(vec![]),
        agent_id: tau_proto::AgentId::new("agent-1"),
        originator: tau_proto::PromptOriginator::User,
    };

    assert!(runtime.dispatch_tool(invoke).is_none());
}

#[test]
fn handshake_registers_email_and_calendar_tools() {
    let mut bytes = Vec::new();
    tau_extension::Handshake::tool("tau-ext-pim")
        .subscribe([
            tau_proto::EventName::TOOL_STARTED,
            tau_proto::EventName::ACTION_INVOKE,
        ])
        .register_tool_with_prompt_fragment(
            email::email_tool_spec(),
            Some(email::email_prompt_fragment()),
        )
        .register_tool_with_prompt_fragment(
            calendar::calendar_tool_spec(),
            Some(calendar::calendar_prompt_fragment()),
        )
        .publish_actions(action_schema())
        .ready_message("pim extension ready")
        .run(&mut FrameWriter::new(&mut bytes))
        .expect("handshake writes");

    let mut reader = FrameReader::new(bytes.as_slice());
    let mut tools = Vec::new();
    let mut saw_subscription = false;
    while let Some(frame) = reader.read_frame().expect("frame decodes") {
        match frame {
            Frame::Message(Message::Subscribe(subscribe)) => {
                saw_subscription = subscribe.selectors
                    == vec![
                        EventSelector::Exact(EventName::TOOL_STARTED),
                        EventSelector::Exact(EventName::ACTION_INVOKE),
                    ];
            }
            Frame::Event(Event::ToolRegister(register)) => tools.push(register.tool.name),
            _ => {}
        }
    }

    assert!(saw_subscription);
    assert_eq!(
        tools,
        vec![
            ToolName::new(email::TOOL_NAME),
            ToolName::new(calendar::TOOL_NAME)
        ]
    );
}
