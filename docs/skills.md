# Skills

Skills are short, focused playbooks that teach the agent how to handle a tool, workflow, framework, or project convention. Tau discovers skills at session start, advertises a small relevant subset in the system prompt, and exposes the rest through the `skill` tool.


## File layout

A skill is a Markdown file with YAML frontmatter. The common layout is:

```text
.agents/skills/<skill-name>/SKILL.md
```

Tau also accepts root-level Markdown files inside a skills directory, but `SKILL.md` inside a named directory is preferred because it gives the skill a stable base directory for related files.


## Discovery scope

Tau currently scans these skills directories, in priority order:

1. `<cwd>/.agents/skills`
2. `<cwd>/.agents.local/skills`
3. `~/.agents/skills`
4. `~/.agents.local/skills`
5. `~/.config/agents/skills`
6. `~/.config/agents.local/skills`

The first skill with a given name wins. Later duplicates are ignored and reported as collisions.

Project-scoped skills (`<cwd>/.agents/skills` and `<cwd>/.agents.local/skills`) default to being advertised to the agent at session start. User-scoped skills default to staying hidden until searched. A skill can override either default with an explicit `advertise:` header.


## Frontmatter

Supported headers are top-level YAML scalar values. Strings, booleans, and numbers are accepted; lists, maps, and `null` are ignored.

```markdown
---
name: rust-workspace
description: How to work in this Rust workspace
advertise: true
---
# Instructions

Run `cargo clippy --all-targets` after Rust edits.
```

Headers:

- `name`: Optional. Defaults to the parent directory name. Must use lowercase ASCII letters, digits, and hyphens only; it must not start or end with a hyphen or contain `--`.
- `description`: Required. A concise one-line summary used for prompt advertisement and search.
- `advertise`: Optional. `true`, `True`, `TRUE`, and `1` advertise the skill immediately. Any other explicit value, including `false`, keeps it out of the initial prompt. If omitted, the directory scope default applies.


## The initial prompt

Advertised skills appear in the system prompt as `<available_skills>` entries containing only the skill name and description. The full skill body is not loaded until the agent calls the `skill` tool with `action: load`.

This keeps prompt size bounded while still making project-relevant skills immediately visible.


## Search and load

The `skill` tool has two actions:

- `search`: Finds skills by case-insensitive substring match against names and descriptions.
- `load`: Loads a skill by exact name and returns the Markdown body with frontmatter stripped.

Example search:

```json
{
  "action": "search",
  "query": ["rust", "style"]
}
```

Multiple query terms are merged. Results include `hit_count` and are sorted by highest hit count, then by name.

By default, search does not read skill bodies. Set `search_content: true` to include body text:

```json
{
  "action": "search",
  "query": "clippy",
  "search_content": true
}
```

If `load` names an unknown skill, Tau returns an error with suggestions based on the requested name split on `-` and `_`.
