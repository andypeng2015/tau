# This built-in layer is the right place for Nickel schema/default metadata.
# User `harness.ncl` files normally stay plain records and values; they do not
# need to repeat these contracts. Nickel propagates the metadata through `&`
# merges, while semantic annotations such as `default` and `not_exported` must
# stay on the fields that depend on them.
let PromptSection = {
    order | Number,
    text | String,
    enabled | Bool | default = true,
    ..
} in
let SkillPrompt = {
    name | String,
    description | String,
} in
let escapeXml = fun text =>
    text
    |> std.string.replace "&" "&amp;"
    |> std.string.replace "<" "&lt;"
    |> std.string.replace ">" "&gt;"
    |> std.string.replace "\"" "&quot;"
    |> std.string.replace "'" "&apos;" in
let renderPromptSections = fun sections =>
    sections
    |> std.record.to_array
    |> std.array.sort (fun left right =>
        let byOrder = std.number.compare left.value.order right.value.order in
        if byOrder == 'Equal then
            std.string.compare left.field right.field
        else
            byOrder)
    |> std.array.filter (fun section => section.value.enabled)
    |> std.array.map (fun section => section.value.text)
    |> std.array.filter (fun text => text != "")
    |> std.string.join "\n\n" in
let renderSkillsSection = fun skills =>
    if std.array.length skills == 0 then
        ""
    else
        let renderedSkills =
            skills
            |> std.array.map (fun skill =>
                "  <skill>\n    <name>%{escapeXml skill.name}</name>\n    <description>%{escapeXml skill.description}</description>\n  </skill>")
            |> std.string.join "\n" in
        "Skills provide specialized instructions for specific tasks.\nBelow are skills you should be initially aware of. Use the skill tool to load a skill, and search for more skills.\n\n<available_skills>\n%{renderedSkills}\n</available_skills>" in
let SystemPromptTemplate = {
    roleName | String,
    cwd | String,
    skills | Array SkillPrompt,
    toolPromptHooksText | String,
    sections | { _ : PromptSection },
    text | String,
    ..
} in
let baseSystemPromptTemplate = {
    roleName | String,
    cwd | String,
    skills | Array SkillPrompt,
    toolPromptHooksText | String,
    sections | { _ : PromptSection } = {
        intro.order | default = 10,
        intro.text | default = m%"
You are an expert coding assistant operating inside Tau, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.

You can call multiple tools in a single response. If you intend to call multiple tools and there are no dependencies between the calls, make all independent tool calls in parallel in the same response. Maximize use of parallel tool calls where possible to increase efficiency. However, if some tool calls depend on previous calls to inform dependent values, do NOT call these tools in parallel and instead call them sequentially. For instance, if one operation must complete before another starts, run these operations sequentially instead.
"%,
        dedup.order | default = 20,
        dedup.text | default = "Tau deduplicates tool result outputs. `[tau-dedup]` results are not errors, but an optimization pointing at an earlier identical result.",
        availableSkills.order | default = 30,
        availableSkills.text | default = renderSkillsSection skills,
        currentWorkingDirectory.order | default = 40,
        currentWorkingDirectory.text | default = "Current working directory: %{cwd}",
        toolPromptHooks.order | default = 70,
        toolPromptHooks.text | default =
            if toolPromptHooksText == "" then
                ""
            else
                toolPromptHooksText,
    },
    text | default = renderPromptSections sections,
} in
let BuiltInRole = {
    description | String,
    systemPrompt | not_exported | SystemPromptTemplate = baseSystemPromptTemplate,
    ..
} in
# Built-in HarnessSettings defaults. Harness-owned roles and tool profiles are
# provided as ordinary config so user partial overrides keep built-in fields.
{
    session_retention_days | default = 60,
    extensions = {
        "provider-openai" = {
            suffix = ["ext", "ext-provider-openai"],
            role = "provider",
            enable | default = true,
            config = {},
        },
        "core-shell" = {
            suffix = ["ext", "ext-shell"],
            role = "tool",
            enable | default = true,
            config = {},
        },
        "test-dummy" = {
            suffix = ["ext", "ext-test-dummy"],
            role = "tool",
            enable | default = false,
            config = {},
        },
        "core-delegate" = {
            suffix = ["ext", "ext-core-delegate"],
            role = "tool",
            enable | default = true,
            config = {},
        },
        "std-notifications" = {
            suffix = ["ext", "ext-std-notifications"],
            role = "tool",
            enable | default = true,
            config = {
                idle_seconds | default = 60,
                idle_agent_summary | default = false,
            },
        },
        "std-websearch-exa" = {
            suffix = ["ext", "ext-websearch-exa"],
            role = "tool",
            enable | default = true,
            config = {},
        },
    },
    # Non-exported lazy role system-prompt templates live with the built-in
    # role definitions. Rust exports the settings with eval_deep_for_export
    # (which skips not_exported fields), then merges one selected role prompt
    # record with runtime context such as cwd, discovered skills, and extension
    # tool prompt hooks.
    roles = {
        smart | BuiltInRole = {
            description | default = "Individual contributor using state of the art model. Good default for most tasks.",
        },
        rush | BuiltInRole = {
            description | default = "Individual contributor using fast and cheaper model for smaller well-defined tasks.",
            effort | default = "low",
            thinkingSummary | default = "off",
        },
        deep | BuiltInRole = {
            description | default = "Deep reasoning expert, using potentially slower and more expensive model. Good for research and very complex tasks.",
            effort | default = "xhigh",
            thinkingSummary | default = "detailed",
        },
        foreman | BuiltInRole = {
            description | default = "Role focused on splitting and delegation of tasks to other sub-agents",
            systemPrompt | not_exported = (
                let defaultSubTaskRoles =
                    roles
                    |> std.record.fields
                    |> std.array.filter (fun roleName => roleName != "foreman") in
                let renderSubTaskRoleList = fun roleNames =>
                    roleNames
                    |> std.array.map (fun roleName =>
                        let role = std.record.get_or roleName {} roles in
                        let description = std.record.get_or "description" roleName role in
                        "* `%{roleName}` - \"%{description}\"")
                    |> std.string.join "\n" in
                baseSystemPromptTemplate & {
                    # The default foreman prompt derives its sub-task role list from
                    # the current role table, excluding foreman itself. Sibling role
                    # description overrides still flow into the lazy prompt without
                    # Rust passing role names at runtime. Keeping this list in a
                    # section lets user config tweak the selected roles or disable
                    # just one part of the built-in prompt.
                    sections = {
                        role.order | default = 50,
                        role.text | default = m%"
You are a foreman/orchestrator agent. Your job is to plan, coordinate, and synthesize work by delegating to sub-agents instead of doing all non-trivial work yourself.

Default workflow:

* For tiny, simple, or purely clerical tasks, you may work directly.
* For non-trivial tasks, use the `delegate` tool as the default path. Split the work into sub-agent steps for research/scoping, implementation, and review/validation. Split or repeat steps as needed for the task scope and difficulty.
* Delegate those steps instead of performing every detail yourself.
* Pass each sub-agent complete, self-contained instructions: the goal, relevant context, exact paths/symbols/snippets, constraints, tests/validation to run, and expected answer format.
* When useful, choose an explicit role for delegated work from the available sub-task roles list.
* Synthesize sub-agent results, resolve discrepancies, do only tiny or clerical follow-up directly, and report the final outcome.
"%,
                        availableSubTaskRoles.order | default = 60,
                        availableSubTaskRoles.roles | Array String | default = defaultSubTaskRoles,
                        availableSubTaskRoles.text | default =
                            if std.array.length availableSubTaskRoles.roles == 0 then
                                ""
                            else
                                "## Available sub-task roles\n\n%{renderSubTaskRoleList availableSubTaskRoles.roles}",
                    },
                }),
        },
    },
    toolsProfiles = {
        gpt = {
            apply_patch | default = true,
            edit | default = false,
            find | default = false,
            gpt_shell | default = true,
            grep | default = false,
            ls | default = false,
            read | default = false,
            shell | default = false,
            write | default = false,
        },
    },
}
