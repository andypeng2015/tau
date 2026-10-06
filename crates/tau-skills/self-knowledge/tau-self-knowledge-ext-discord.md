---
name: tau-self-knowledge-ext-discord
description: Use when configuring or troubleshooting the separately maintained Tau Discord bridge, including receiver designation, tool authority, routing, or migration.
advertise: false
---

# Tau Discord bridge self-knowledge

`tau-ext-discord` is a separately maintained MessageBridge executable. Its
project owns the exact credential, route, and transport schema. Consult its
`docs/setup.md` for private maintainer guild-channel setup, `SECURITY.md` for
trust boundaries, and `docs/design.md` and `docs/runtime.md` for the full contract
and limits. Do not infer compatibility from the Cargo package version:
registry SDK/proto 0.7.0 advertises wire 9.0 and is rejected by Tau 10.4;
the verified extension revision `f5fdea420430773c81b77dc4fbf862db8214f29a`
pins registry SDK/proto 0.10.0, which advertises wire 10.2. Tau 10.4 admits
that revision best-effort with a minor-skew warning. This is not a claim about
the newest available SDK or full live Discord connectivity.

## Private maintainer channel

Use a normal private guild **text channel**, not strict send-only mode (which
supports a fixed 1:1 DM). Channel permissions and bridge admission are separate:
explicitly allowlist every authorized human with `allowed_user_ids`. Admitted
messages remain untrusted external content.

The owning setup guide supplies a complete names-only harness skeleton. Its
extension-owned `config` uses:

```yaml
bot_token_secret: discord_bot_token
identity_key_secret: discord_identity_key
role: coordinator
register_on_start: true
message_content: false
allowed_user_ids: [HUMAN_MAINTAINER_ID]
conversations:
  - alias: maintainer_channel
    guild_id: GUILD_ID
    channel_id: PRIVATE_TEXT_CHANNEL_ID
    receive: mentions_only
    proactive_send: true
```

Replace ID placeholders with exact nonzero u64 integers. Omit `send_only`
(defaults false). This admits personal bot mentions in the configured channel,
not ordinary chatter, and does not require privileged guild message-content
intent. Do not broaden it to all messages without explicit operator/provider
approval.

Declare `discord_bot_token: {}` and `discord_identity_key: {}` under the outer
instance's `secrets`. Provision them through Tau's managed-secret sources before
harness startup; see `tau-self-knowledge-secrets`. Never put values in config,
docs, or shell history. The `.yaml` secret files contain raw UTF-8 text, not YAML
mappings. Use a bot token and a separate stable identity key.

Receiver eligibility is not send-tool authorization. For coordinator-only text
tools, deny `discord:*` tags globally, then enable only the final names
`maintainer_discord_register`, `maintainer_discord_conversations`, and
`maintainer_discord_send` on the existing coordinator role when the outer
instance has `tool_prefix: maintainer`. Audit inherited and other explicit
enables, which can restore tools after tag denial. The instance name does not
prefix tools. Do not enable the whole group: it includes reactions. Ordinary
text needs no attachment or Artifact egress grant.

## Startup versus live delivery

A credential-free, network-isolated smoke test of the verified extension reached
Ready on Tau `91d51d60` (wire 10.4) with null and empty config. That proves
protocol admission and lifecycle startup only. Ready or successful registration
does not prove asynchronous Discord authentication/Gateway startup, channel
permissions, receive admission, or actual delivery. Normal sends require current
registration and a live writable transport; use `discord_conversations` (with
the configured prefix) to discover aliases rather than supplying native IDs.

All writes suppress user, role, everyone/here, and reply-notification mentions.
Mention-like text is not a supported notifying ping. Do not automatically retry
failed or uncertain writes: a send may have taken effect without a confirmed
receipt.

## Receiver designation

In receive mode, `discord_register {}` means “make the authenticated calling agent
the receiving agent.” It has no arbitrary agent-ID argument. The tool takes empty
arguments; `{"enabled":false}` and direct `discord_unregister` invocations are
rejected rather than creating a pause control. A configured `role` restricts
eligible callers by immutable creation role.

```yaml
register_on_start: true # register extension automatically
role: coordinator # role to deliver to; optional, constrains registration and automatic delivery
```

`register_on_start` defaults to `false`. A saved eligible manual designation still
resumes after restart, but Tau neither first-selects nor creates a receiver. With
`true`, the bridge keeps an eligible saved receiver or selects the oldest eligible
existing agent. Lazy creation requires admitted external input and an explicit
`role`; without a role, automatic selection is existing-only. Busy receivers remain
sticky, and same-agent registration is a no-op.

The bridge stores a versioned receive designation in session ExtensionData under
`receive-designation.json`; it is not an operator filesystem path. Ephemeral
sessions retain that choice only in memory. The old
`restore_receive_registration` configuration key is rejected for either value with
migration guidance. A missing designation can use one unambiguous historical
registration as migration evidence, but a saved designation wins.

An actual handoff may send a best-effort notice to a still-loaded prior receiver.
Already reported input and its ACK correlation remain with their original agent.
Old source, write, and Artifact authority does not transfer. Separate send,
reaction, attachment, discovery, and send-only grants remain independent of receive
designation.
