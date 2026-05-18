//! Building blocks for the per-turn prompt: the system prompt body, the
//! AGENTS.md context message, and the conversation assembly that turns a
//! [`tau_core::SessionTree`] into item-based prompt context.

use tau_core::SessionEntry;
use tau_proto::{CborValue, ContextItem, PromptHook};

use crate::discovery::{DiscoveredAgentsFile, DiscoveredSkill};

/// Renders Tau's lazy Nickel system-prompt template plus harness-owned prompt
/// hooks.
///
/// Must be deterministic and stable across turns of the same session
/// — see the linear-prefix invariant in `send_prompt_to_agent`.
/// Tools and skills are sorted by name (HashMap iteration would
/// otherwise drift). The current date is intentionally omitted:
/// including it would invalidate the prompt cache every midnight
/// UTC. cwd is threaded in from the caller so the caller owns the source of
/// truth.
#[derive(Clone, Debug)]
pub(crate) struct SystemPromptRenderer {
    nickel_source: String,
}

impl SystemPromptRenderer {
    /// Creates a renderer for the composed harness Nickel source.
    pub(crate) fn new(nickel_source: String) -> Self {
        Self { nickel_source }
    }

    /// Evaluates the merged role/system prompt record with runtime prompt
    /// context.
    pub(crate) fn render(
        &self,
        skills: &std::collections::HashMap<tau_proto::SkillName, DiscoveredSkill>,
        cwd: &str,
        role_name: &str,
        tool_prompt_hook: &PromptHook,
    ) -> Result<String, tau_config::settings::SettingsError> {
        let ctx = format!(
            "{{ roleName | force = {}, cwd | force = {}, skills | force = {}, toolPromptHooksText | force = {} }}",
            nickel_string_literal(role_name),
            nickel_string_literal(cwd),
            nickel_skills_literal(skills),
            nickel_string_literal(&render_tool_prompt_hooks_section(tool_prompt_hook)),
        );
        let expr = format!(
            "\
let config = ({}) in
let ctx = {ctx} in
let role = std.record.get_or ctx.roleName {{}} config.roles in
let smartPrompt = config.roles.smart.systemPrompt in
# Custom roles may omit systemPrompt. In that case (and for an unknown role
# name), render the built-in smart prompt with the selected roleName in ctx.
# Built-in roles are expected to define their own systemPrompt records.
let rolePrompt = std.record.get_or \"systemPrompt\" smartPrompt role in
(rolePrompt & ctx).text",
            self.nickel_source
        );
        let mut context = nickel_lang::Context::new()
            .with_source_name("composed harness systemPrompt".to_owned());
        let value = context
            .eval_deep_for_export(&expr)
            .map_err(format_nickel_error)?;
        value
            .to_serde()
            .map_err(|err| tau_config::settings::SettingsError::Deserialize(err.to_string()))
    }
}

#[cfg(test)]
pub(crate) fn render_builtin_system_prompt(
    skills: &std::collections::HashMap<tau_proto::SkillName, DiscoveredSkill>,
    cwd: &str,
    include_foreman_prompt: bool,
    tool_prompt_hook: &PromptHook,
) -> String {
    let loaded = tau_config::settings::load_harness_settings_with_source()
        .expect("built-in harness settings should load");
    let role_name = if include_foreman_prompt {
        "foreman"
    } else {
        "smart"
    };
    SystemPromptRenderer::new(loaded.nickel_source)
        .render(skills, cwd, role_name, tool_prompt_hook)
        .expect("built-in system prompt should render")
}

fn nickel_skills_literal(
    skills: &std::collections::HashMap<tau_proto::SkillName, DiscoveredSkill>,
) -> String {
    let mut prompt_skills: Vec<_> = skills.iter().filter(|(_, s)| s.add_to_prompt).collect();
    prompt_skills.sort_by(|(a, _), (b, _)| a.as_str().cmp(b.as_str()));
    let mut literal = String::from("[");
    for (index, (name, skill)) in prompt_skills.iter().enumerate() {
        if index > 0 {
            literal.push_str(", ");
        }
        let description = tau_skills::truncate_description(&skill.description);
        literal.push_str("{ name = ");
        literal.push_str(&nickel_string_literal(name.as_str()));
        literal.push_str(", description = ");
        literal.push_str(&nickel_string_literal(description.as_ref()));
        literal.push_str(" }");
    }
    literal.push(']');
    literal
}

fn render_tool_prompt_hooks_section(tool_prompt_hook: &PromptHook) -> String {
    let mut prompt = String::new();
    append_prompt_hook(&mut prompt, tool_prompt_hook);
    prompt
}

fn append_prompt_hook(prompt: &mut String, hook: &PromptHook) {
    let mut first = true;
    for (_, content) in hook {
        if content.is_empty() {
            continue;
        }
        if !first {
            prompt.push_str("\n\n");
        }
        prompt.push_str(content.as_str());
        first = false;
    }
}

fn nickel_string_literal(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 2);
    escaped.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{:04x}", ch as u32);
            }
            ch => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}

fn format_nickel_error(error: nickel_lang::Error) -> tau_config::settings::SettingsError {
    let mut out = Vec::new();
    if error
        .format(&mut out, nickel_lang::ErrorFormat::Text)
        .is_ok()
    {
        tau_config::settings::SettingsError::Nickel(String::from_utf8_lossy(&out).into_owned())
    } else {
        tau_config::settings::SettingsError::Nickel(format!("{error:?}"))
    }
}

pub(crate) fn render_agents_context_message<'a>(
    files: impl IntoIterator<Item = &'a DiscoveredAgentsFile>,
) -> String {
    let mut text = String::from(
        "# AGENTS.md instructions\n\n\
The following instructions were loaded from AGENTS.md files.\n\
More specific files usually override broader ones.\n\n",
    );

    for file in files {
        text.push_str(&format!(
            "<AGENTS_FILE path=\"{}\">\n",
            file.file_path.display()
        ));
        text.push_str(&file.content);
        if !file.content.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("</AGENTS_FILE>\n\n");
    }

    text
}

/// Returns the current date as YYYY-MM-DD without chrono.
pub(crate) fn chrono_free_date() -> String {
    // Use UNIX timestamp to derive date.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    // Simple days-since-epoch to Y-M-D (good enough, no leap second edge cases).
    let mut y = 1970_i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0;
    for md in &month_days {
        if remaining < *md {
            break;
        }
        remaining -= md;
        m += 1;
    }
    format!("{y}-{:02}-{:02}", m + 1, remaining + 1)
}

/// Converts the branch ending at `head` into LLM prompt context
/// items. Each conversation tracks its own head; with multiple
/// side conversations interleaving tree mutations (one delegate's
/// teardown snapping `tree.head` to the default conv, another
/// delegate's tool result arriving moments later), `tree.head()` is
/// not reliable as the prompt-assembly cursor — use the conv's own
/// head instead.
pub(crate) struct AssembledPromptContext {
    pub(crate) context_items: Vec<ContextItem>,
}

pub(crate) fn assemble_conversation_from(
    tree: &tau_core::SessionTree,
    head: Option<tau_core::NodeId>,
) -> Vec<ContextItem> {
    assemble_prompt_context_from(tree, head).context_items
}

pub(crate) fn assemble_prompt_context_from(
    tree: &tau_core::SessionTree,
    head: Option<tau_core::NodeId>,
) -> AssembledPromptContext {
    let mut context_items: Vec<ContextItem> = Vec::new();

    for entry in tree.branch_from(head) {
        match entry {
            SessionEntry::UserInput { items } => {
                context_items.extend(items.iter().cloned());
            }
            SessionEntry::AssistantResponse { output_items, .. } => {
                context_items.extend(output_items.iter().cloned());
            }
            SessionEntry::ToolResults { items } => {
                context_items.extend(items.iter().cloned().map(ContextItem::ToolResult));
            }
            SessionEntry::Compaction { replacement_window } => {
                context_items = replacement_window.clone();
            }
        }
    }

    AssembledPromptContext { context_items }
}

/// Extract a boolean value from a CBOR map by key.
pub(crate) fn cbor_map_bool(map: &CborValue, key: &str) -> Option<bool> {
    match map {
        CborValue::Map(entries) => entries.iter().find_map(|(k, v)| match (k, v) {
            (CborValue::Text(k), CborValue::Bool(b)) if k == key => Some(*b),
            _ => None,
        }),
        _ => None,
    }
}

/// Converts a CBOR value to human-readable text for tool results.
#[cfg(test)]
pub(crate) fn cbor_to_text(v: &tau_proto::CborValue) -> String {
    use tau_proto::CborValue;
    match v {
        CborValue::Null => String::new(),
        CborValue::Bool(b) => b.to_string(),
        CborValue::Integer(i) => {
            let n: i128 = (*i).into();
            n.to_string()
        }
        CborValue::Float(f) => f.to_string(),
        CborValue::Text(s) => s.clone(),
        CborValue::Bytes(b) => format!("<{} bytes>", b.len()),
        CborValue::Array(arr) => arr.iter().map(cbor_to_text).collect::<Vec<_>>().join("\n"),
        CborValue::Map(entries) => {
            // For maps, extract text values cleanly.
            let mut parts = Vec::new();
            for (k, val) in entries {
                let key = match k {
                    CborValue::Text(s) => s.clone(),
                    other => cbor_to_text(other),
                };
                let value = cbor_to_text(val);
                if value.contains('\n') || key == "line-numbered content" {
                    parts.push(format!("{key}:\n{value}"));
                } else {
                    parts.push(format!("{key}: {value}"));
                }
            }
            parts.join("\n")
        }
        CborValue::Tag(_, inner) => cbor_to_text(inner),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use tau_proto::{
        ContentPart, ContextItem, ContextRole, Event, MessageItem, ToolError, ToolResultStatus,
    };

    use super::*;

    fn assistant_message(text: &str) -> ContextItem {
        ContextItem::Message(MessageItem {
            role: ContextRole::Assistant,
            content: vec![ContentPart::Text {
                text: text.to_owned(),
            }],
            phase: None,
        })
    }

    #[test]
    fn render_builtin_system_prompt_includes_cwd() {
        let skills = std::collections::HashMap::new();
        let prompt = render_builtin_system_prompt(
            &skills,
            "/tmp/work",
            false,
            &tau_proto::PromptHook::new(),
        );
        assert!(prompt.contains("expert coding assistant"));
        assert!(prompt.contains("Current working directory: /tmp/work"));
        assert!(!prompt.ends_with('\n'));
    }

    #[test]
    fn render_builtin_system_prompt_encourages_parallel_tool_calls() {
        let skills = std::collections::HashMap::new();
        let prompt = render_builtin_system_prompt(
            &skills,
            "/tmp/work",
            false,
            &tau_proto::PromptHook::new(),
        );
        assert!(prompt.contains("parallel"));
        assert!(prompt.contains("make all independent tool calls in parallel"));
    }

    #[test]
    fn system_prompt_renderer_includes_runtime_sections_from_nickel_template() {
        // Runtime-only prompt data is passed to Nickel as an escaped record and
        // composed by the non-exported template, not exported as role fields.
        let mut skills = std::collections::HashMap::new();
        skills.insert(
            tau_proto::SkillName::from("escape-me"),
            DiscoveredSkill {
                source_id: "skills".into(),
                description: "Use <xml> & quotes".to_owned(),
                source: crate::discovery::DiscoveredSkillSource::File(std::path::PathBuf::from(
                    "/skills/escape-me/SKILL.md",
                )),
                add_to_prompt: true,
            },
        );
        let mut tool_hook = tau_proto::PromptHook::new();
        tool_hook.insert((
            tau_proto::PromptPriority::new(10),
            tau_proto::PromptContent::new("TOOL HOOK"),
        ));

        let loaded = tau_config::settings::load_harness_settings_with_source()
            .expect("load harness settings");
        let prompt = SystemPromptRenderer::new(loaded.nickel_source)
            .render(&skills, "/tmp/work", "smart", &tool_hook)
            .expect("render prompt");

        assert!(prompt.contains("Current working directory: /tmp/work"));
        assert!(prompt.contains("<name>escape-me</name>"));
        assert!(prompt.contains("Use &lt;xml&gt; &amp; quotes"));
        assert!(prompt.contains(crate::dedup::DEDUP_MARKER));
        assert!(prompt.contains("TOOL HOOK"));
    }

    #[test]
    fn built_in_system_prompt_requires_runtime_context_fields() {
        // Runtime context such as cwd must be provided by the renderer. The
        // built-in template should not silently fall back to empty defaults.
        let loaded = tau_config::settings::load_harness_settings_with_source()
            .expect("load harness settings");
        let expr = format!(
            "\
let config = ({}) in
config.roles.smart.systemPrompt.text",
            loaded.nickel_source
        );
        let mut context = nickel_lang::Context::new()
            .with_source_name("built-in systemPrompt without runtime ctx".to_owned());

        assert!(context.eval_deep_for_export(&expr).is_err());
    }

    /// The built-in foreman prompt carries Tau's delegation workflow and is
    /// followed by the available sub-task roles list.
    #[test]
    fn render_builtin_system_prompt_includes_foreman_delegation_context() {
        let skills = std::collections::HashMap::new();
        let prompt =
            render_builtin_system_prompt(&skills, "/tmp/work", true, &tau_proto::PromptHook::new());

        let role = prompt
            .find("You are a foreman/orchestrator agent")
            .expect("built-in foreman prompt");
        let roles = prompt
            .find("## Available sub-task roles")
            .expect("available roles");
        assert!(role < roles);
        assert!(prompt.contains("use the `delegate` tool"));
        assert!(prompt.contains("* `smart` - \"Individual contributor using state of the art model. Good default for most tasks.\""));
        assert!(prompt.contains("* `deep` - \"Deep reasoning expert"));
        assert!(prompt.contains("* `rush` - \"Individual contributor using fast"));
        assert!(!prompt.contains("* `foreman` - \""));
    }

    #[test]
    fn system_prompt_renderer_uses_configured_sub_task_roles_list() {
        // The foreman prompt should let user config choose exactly which
        // sub-task roles are advertised, instead of hard-coding a static list.
        let td = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            td.path().join("harness.ncl"),
            r#"{
                    roles = {
                        reviewer = {
                            description = "Focused reviewer",
                        },
                        foreman = {
                            systemPrompt = {
                                sections = {
                                    availableSubTaskRoles = {
                                        roles = ["reviewer", "rush"],
                                    },
                                },
                            },
                        },
                    },
                }"#,
        )
        .expect("write");
        let dirs = tau_config::settings::TauDirs {
            config_dir: Some(td.path().to_path_buf()),
            state_dir: None,
        };
        let loaded = tau_config::settings::load_harness_settings_with_source_in(&dirs)
            .expect("load harness settings");

        let prompt = SystemPromptRenderer::new(loaded.nickel_source)
            .render(
                &std::collections::HashMap::new(),
                "/roles",
                "foreman",
                &tau_proto::PromptHook::new(),
            )
            .expect("render prompt");

        assert!(prompt.contains("* `reviewer` - \"Focused reviewer\""));
        assert!(prompt.contains("* `rush` - \"Individual contributor using fast and cheaper model for smaller well-defined tasks.\""));
        assert!(!prompt.contains("* `smart` - \""));
        assert!(!prompt.contains("* `deep` - \""));
        assert!(!prompt.contains("* `foreman` - \""));
    }

    #[test]
    fn system_prompt_renderer_uses_user_overridden_template_and_sibling_roles() {
        // User harness.ncl can replace a non-exported role prompt record. The
        // template is merged with runtime context, and still closes over merged
        // sibling role config.
        let td = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            td.path().join("harness.ncl"),
            r#"{
                    roles = {
                        smart = {
                            description = "custom smart role",
                            systemPrompt = {
                                roleName | String,
                                cwd | String,
                                text = "role=%{roleName}; cwd=%{cwd}; smart=%{roles.smart.description}\n",
                            },
                        },
                    },
                }"#,
        )
        .expect("write");
        let dirs = tau_config::settings::TauDirs {
            config_dir: Some(td.path().to_path_buf()),
            state_dir: None,
        };
        let loaded = tau_config::settings::load_harness_settings_with_source_in(&dirs)
            .expect("load harness settings");
        assert_eq!(
            loaded.settings.roles["smart"].description.as_deref(),
            Some("custom smart role")
        );

        let prompt = SystemPromptRenderer::new(loaded.nickel_source)
            .render(
                &std::collections::HashMap::new(),
                "/override",
                "smart",
                &tau_proto::PromptHook::new(),
            )
            .expect("render prompt");

        assert_eq!(
            prompt,
            "role=smart; cwd=/override; smart=custom smart role\n"
        );
    }

    #[test]
    fn system_prompt_renderer_merges_user_section_overrides_into_built_in_template() {
        // Users should be able to tweak one named system-prompt section with a
        // plain record, without repeating built-in contracts or replacing the
        // whole template record. This protects the section-based Nickel layout
        // from regressing back to whole-record override semantics.
        let td = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            td.path().join("harness.ncl"),
            r#"{
                    roles = {
                        smart = {
                            description = "section smart role",
                            systemPrompt = {
                                sections = {
                                    intro = { enabled = false },
                                    dedup = { enabled = false },
                                    availableSkills = { enabled = false },
                                    currentWorkingDirectory = {
                                        order = 35,
                                    },
                                    custom = {
                                        order = 45,
                                        text = "; smart=%{roles.smart.description}",
                                    },
                                },
                            },
                        },
                    },
                }"#,
        )
        .expect("write");
        let dirs = tau_config::settings::TauDirs {
            config_dir: Some(td.path().to_path_buf()),
            state_dir: None,
        };
        let loaded = tau_config::settings::load_harness_settings_with_source_in(&dirs)
            .expect("load harness settings");

        let prompt = SystemPromptRenderer::new(loaded.nickel_source)
            .render(
                &std::collections::HashMap::new(),
                "/sections",
                "smart",
                &tau_proto::PromptHook::new(),
            )
            .expect("render prompt");

        assert_eq!(
            prompt,
            "Current working directory: /sections\n\n; smart=section smart role"
        );
    }

    #[test]
    fn system_prompt_renderer_falls_back_to_smart_prompt_for_custom_role_without_template() {
        // Custom roles are allowed to provide only model metadata. When they do
        // not define a non-exported prompt template, render with smart's
        // prompt record while preserving the selected runtime role name.
        let td = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            td.path().join("harness.ncl"),
            r#"{
                    roles = {
                        custom = {
                            description = "metadata-only custom role",
                        },
                    },
                }"#,
        )
        .expect("write");
        let dirs = tau_config::settings::TauDirs {
            config_dir: Some(td.path().to_path_buf()),
            state_dir: None,
        };
        let loaded = tau_config::settings::load_harness_settings_with_source_in(&dirs)
            .expect("load harness settings");

        let prompt = SystemPromptRenderer::new(loaded.nickel_source)
            .render(
                &std::collections::HashMap::new(),
                "/custom",
                "custom",
                &tau_proto::PromptHook::new(),
            )
            .expect("render prompt");

        assert!(prompt.contains("You are an expert coding assistant operating inside Tau"));
        assert!(prompt.contains("Current working directory: /custom"));
        assert!(!prompt.contains("You are a foreman/orchestrator agent"));
    }

    /// Non-foreman roles receive the base Tau prompt and tool hooks without
    /// the built-in delegation workflow.
    #[test]
    fn render_builtin_system_prompt_omits_foreman_context_for_smart_roles() {
        let skills = std::collections::HashMap::new();
        let prompt = render_builtin_system_prompt(
            &skills,
            "/tmp/work",
            false,
            &tau_proto::PromptHook::new(),
        );

        assert!(!prompt.contains("You are a foreman/orchestrator agent"));
        assert!(!prompt.contains("Available sub-task roles"));
    }

    /// Tool prompt hooks remain a harness-owned layer after Tau's base prompt.
    /// This pins priority ordering without allowing role-configured prompt
    /// text to interleave with tool instructions.
    #[test]
    fn render_builtin_system_prompt_composes_tool_prompt_hooks_in_order() {
        let skills = std::collections::HashMap::new();
        let mut tool_hook = tau_proto::PromptHook::new();
        tool_hook.insert((
            tau_proto::PromptPriority::new(20),
            tau_proto::PromptContent::new("TOOL LATE"),
        ));
        tool_hook.insert((
            tau_proto::PromptPriority::new(10),
            tau_proto::PromptContent::new("TOOL EARLY"),
        ));

        let prompt = render_builtin_system_prompt(&skills, "/tmp/work", false, &tool_hook);

        let base = prompt
            .find("Current working directory: /tmp/work")
            .expect("base Tau system prompt should render cwd");
        let early = prompt
            .find("TOOL EARLY")
            .expect("earlier-priority tool prompt should be rendered");
        let late = prompt
            .find("TOOL LATE")
            .expect("later-priority tool prompt should be rendered");
        assert!(base < early);
        assert!(early < late);
    }

    /// Empty hook entries are ignored without adding a blank prompt section.
    #[test]
    fn render_builtin_system_prompt_ignores_empty_tool_prompt_hook_sections() {
        let skills = std::collections::HashMap::new();
        let without_hook = render_builtin_system_prompt(
            &skills,
            "/tmp/work",
            false,
            &tau_proto::PromptHook::new(),
        );
        let mut empty_hook = tau_proto::PromptHook::new();
        empty_hook.insert((
            tau_proto::PromptPriority::new(10),
            tau_proto::PromptContent::new(""),
        ));
        let with_empty_hook =
            render_builtin_system_prompt(&skills, "/tmp/work", false, &empty_hook);

        assert_eq!(with_empty_hook, without_hook);
    }

    #[test]
    fn cbor_to_text_puts_line_numbered_content_on_next_line() {
        let text = cbor_to_text(&CborValue::Map(vec![(
            CborValue::Text("line-numbered content".to_owned()),
            CborValue::Text("1 only".to_owned()),
        )]));

        assert_eq!(text, "line-numbered content:\n1 only");
    }

    /// Tool errors must surface their `details` payload to the LLM,
    /// not just the bare `message`. The shell extension stuffs
    /// stdout/stderr/exit_code into `details` on failure; without
    /// this, the model sees only "command exited with status 1" and
    /// has to re-run the command with `2>&1 | tail` to recover the
    /// diagnostic output.
    #[test]
    fn assemble_conversation_includes_tool_error_details() {
        let mut tree = tau_core::SessionTree::from_events("session-1".into(), &[]);
        tree.apply_event(&Event::UiPromptSubmitted(tau_proto::UiPromptSubmitted {
            text: "build firefox".to_owned(),
            session_id: "session-1".into(),
            originator: tau_proto::PromptOriginator::default(),
            ctx_id: None,
        }));
        tree.apply_event(&Event::ProviderResponseFinished(
            tau_proto::ProviderResponseFinished {
                session_prompt_id: "sp-tools".into(),
                output_items: vec![ContextItem::ToolCall(tau_proto::ToolCallItem {
                    call_id: "call-1".into(),
                    name: tau_proto::ToolName::new("shell"),
                    tool_type: tau_proto::ToolType::Function,
                    arguments: CborValue::Null,
                })],
                stop_reason: tau_proto::ProviderStopReason::ToolCalls,
                originator: tau_proto::PromptOriginator::User,
                usage: None,
                backend: None,
                provider_response_id: None,
                ws_pool_delta: None,
            },
        ));
        let details = CborValue::Map(vec![
            (
                CborValue::Text("stdout".to_owned()),
                CborValue::Text("compiling".to_owned()),
            ),
            (
                CborValue::Text("stderr".to_owned()),
                CborValue::Text("patch 73cbb9ff failed to apply".to_owned()),
            ),
            (
                CborValue::Text("status".to_owned()),
                CborValue::Integer(1.into()),
            ),
        ]);
        tree.apply_event(&Event::ToolError(ToolError {
            call_id: "call-1".into(),
            tool_name: tau_proto::ToolName::new("shell"),
            tool_type: tau_proto::ToolType::Function,
            message: "command exited with status 1".to_owned(),
            details: Some(details),
            display: None,
            originator: tau_proto::PromptOriginator::User,
        }));

        let items = assemble_conversation_from(&tree, tree.head());
        let tool_result = items
            .iter()
            .find_map(|item| match item {
                ContextItem::ToolResult(result)
                    if matches!(result.status, ToolResultStatus::Error { .. }) =>
                {
                    Some(result)
                }
                _ => None,
            })
            .expect("error tool result should be present");

        let ToolResultStatus::Error { message } = &tool_result.status else {
            panic!("expected error tool result status")
        };
        let detail_text = cbor_to_text(&tool_result.output);

        assert!(
            message.contains("command exited with status 1"),
            "missing message: {message}"
        );
        assert!(
            detail_text.contains("patch 73cbb9ff failed to apply"),
            "missing stderr: {detail_text}"
        );
        assert!(
            detail_text.contains("compiling"),
            "missing stdout: {detail_text}"
        );
    }

    /// `phase` captured on a prior assistant turn must show up on
    /// the `ConversationMessage` we hand to the backend on the next
    /// prompt. This is the link in the chain that lets the
    /// Responses backend stamp the wire field without round-tripping
    /// through a separate side channel.
    #[test]
    fn assemble_conversation_preserves_agent_phase() {
        let mut tree = tau_core::SessionTree::from_events("session-1".into(), &[]);
        tree.apply_event(&Event::UiPromptSubmitted(tau_proto::UiPromptSubmitted {
            text: "hi".to_owned(),
            session_id: "session-1".into(),
            originator: tau_proto::PromptOriginator::default(),
            ctx_id: None,
        }));
        tree.apply_event(&Event::ProviderResponseFinished(
            tau_proto::ProviderResponseFinished {
                session_prompt_id: "sp-1".into(),
                output_items: vec![ContextItem::Message(MessageItem {
                    role: ContextRole::Assistant,
                    content: vec![ContentPart::Text {
                        text: "draft answer".to_owned(),
                    }],
                    phase: Some(tau_proto::MessagePhase::Commentary),
                })],
                stop_reason: tau_proto::ProviderStopReason::EndTurn,
                originator: tau_proto::PromptOriginator::User,
                usage: None,
                backend: None,
                provider_response_id: None,
                ws_pool_delta: None,
            },
        ));

        let items = assemble_conversation_from(&tree, tree.head());
        let assistant = items
            .iter()
            .find_map(|item| match item {
                ContextItem::Message(message) if message.role == ContextRole::Assistant => {
                    Some(message)
                }
                _ => None,
            })
            .expect("assistant message");
        assert_eq!(assistant.phase, Some(tau_proto::MessagePhase::Commentary));
    }

    #[test]
    fn assemble_conversation_restarts_from_compacted_summary() {
        let mut tree = tau_core::SessionTree::from_events("session-1".into(), &[]);
        tree.apply_event(&Event::UiPromptSubmitted(tau_proto::UiPromptSubmitted {
            text: "first question".to_owned(),
            session_id: "session-1".into(),
            originator: tau_proto::PromptOriginator::default(),
            ctx_id: None,
        }));
        tree.apply_event(&Event::ProviderResponseFinished(
            tau_proto::ProviderResponseFinished {
                session_prompt_id: "sp-1".into(),
                output_items: vec![assistant_message("first answer")],
                stop_reason: tau_proto::ProviderStopReason::EndTurn,
                originator: tau_proto::PromptOriginator::User,
                usage: None,
                backend: None,
                provider_response_id: None,
                ws_pool_delta: None,
            },
        ));
        tree.apply_event(&Event::SessionCompacted(tau_proto::SessionCompacted {
            session_id: "session-1".into(),
            originator: tau_proto::PromptOriginator::User,
            original_input_tokens: None,
            compacted_input_tokens: None,
            replacement_window: vec![ContextItem::Message(MessageItem {
                role: ContextRole::Assistant,
                content: vec![ContentPart::Text {
                    text: "Summary of earlier conversation:\n- User is debugging compaction\n- Keep edits focused"
                        .to_owned(),
                }],
                phase: None,
            })],
        }));
        tree.apply_event(&Event::UiPromptSubmitted(tau_proto::UiPromptSubmitted {
            text: "continue".to_owned(),
            session_id: "session-1".into(),
            originator: tau_proto::PromptOriginator::default(),
            ctx_id: None,
        }));

        let items = assemble_conversation_from(&tree, tree.head());
        assert_eq!(items.len(), 2, "pre-compaction history must be dropped");
        assert!(matches!(
            &items[0],
            ContextItem::Message(MessageItem { content, .. })
                if matches!(&content[0], ContentPart::Text { text }
                    if text.contains("Summary of earlier conversation:")
                        && text.contains("debugging compaction"))
        ));
        assert!(matches!(
            &items[1],
            ContextItem::Message(MessageItem { content, .. })
                if matches!(&content[0], ContentPart::Text { text } if text == "continue")
        ));
    }

    /// Encrypted-reasoning replay: when `ProviderResponseFinished` carries
    /// `reasoning_items`, the next assembled prompt's assistant
    /// message must front-load them as `ContentBlock::Reasoning` blocks
    /// before any text. The responses backend then emits them as
    /// top-level `input[]` items (covered by
    /// `build_request_replays_reasoning_item_as_top_level_input`);
    /// this test pins the persistence half of that pipeline so a
    /// future fold refactor can't silently drop them on the floor.
    #[test]
    fn assemble_conversation_replays_reasoning_items_before_text() {
        let mut tree = tau_core::SessionTree::from_events("session-1".into(), &[]);
        tree.apply_event(&Event::UiPromptSubmitted(tau_proto::UiPromptSubmitted {
            text: "hi".to_owned(),
            session_id: "session-1".into(),
            originator: tau_proto::PromptOriginator::default(),
            ctx_id: None,
        }));
        let blob = serde_json::json!({
            "type": "reasoning",
            "id": "rs_xyz",
            "encrypted_content": "OPAQUE",
        })
        .to_string();
        tree.apply_event(&Event::ProviderResponseFinished(
            tau_proto::ProviderResponseFinished {
                session_prompt_id: "sp-1".into(),
                output_items: vec![
                    ContextItem::Reasoning(
                        serde_json::from_str(&blob).expect("opaque reasoning item"),
                    ),
                    assistant_message("here's what I found"),
                ],
                stop_reason: tau_proto::ProviderStopReason::EndTurn,
                originator: tau_proto::PromptOriginator::User,
                usage: None,
                backend: None,
                provider_response_id: None,
                ws_pool_delta: None,
            },
        ));

        let items = assemble_conversation_from(&tree, tree.head());
        assert!(matches!(&items[1], ContextItem::Reasoning(_)));
        assert!(matches!(
            &items[2],
            ContextItem::Message(MessageItem { content, .. })
                if matches!(&content[0], ContentPart::Text { text } if text == "here's what I found")
        ));
    }

    /// Tool-only turn (no message text) with reasoning_items must
    /// still persist as an `AgentMessage` entry — otherwise the
    /// reasoning blob would be lost and reasoning continuity breaks
    /// on any subsequent full-transcript replay. The assembled
    /// assistant message has no Text block but does have the
    /// Reasoning block, ready for the responses backend to emit it
    /// before any function_call items that follow.
    #[test]
    fn assemble_conversation_persists_reasoning_on_tool_only_turn() {
        let mut tree = tau_core::SessionTree::from_events("session-1".into(), &[]);
        tree.apply_event(&Event::UiPromptSubmitted(tau_proto::UiPromptSubmitted {
            text: "go".to_owned(),
            session_id: "session-1".into(),
            originator: tau_proto::PromptOriginator::default(),
            ctx_id: None,
        }));
        let blob = serde_json::json!({
            "type": "reasoning",
            "id": "rs_tool_turn",
            "encrypted_content": "OPAQUE",
        })
        .to_string();
        tree.apply_event(&Event::ProviderResponseFinished(
            tau_proto::ProviderResponseFinished {
                session_prompt_id: "sp-1".into(),
                output_items: vec![ContextItem::Reasoning(
                    serde_json::from_str(&blob).expect("opaque reasoning item"),
                )],
                stop_reason: tau_proto::ProviderStopReason::EndTurn,
                originator: tau_proto::PromptOriginator::User,
                usage: None,
                backend: None,
                provider_response_id: None,
                ws_pool_delta: None,
            },
        ));

        let items = assemble_conversation_from(&tree, tree.head());
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[1], ContextItem::Reasoning(_)));
    }
}
