# Agent messaging tool

The harness-owned `message` tool lets an agent send an asynchronous short text note to the user or to another agent. Every sent message is recorded as an `agent.message` event and shown in the UI as:

```text
Message from <sender> to <recipient>:
<message>
```

## Send to the user

Use the special recipient id `user`:

```text
message({"recipient_id":"user","message":"I found the root cause and am checking the fix now."})
```

On success the tool result is:

```text
Message sent
```

## Send to another agent

Start the other agent with `delegate`. The instant background placeholder includes `self_agent_id` and `sub_agent_id` headers. The final delegate result carries the same ids alongside the sub-agent `output`:

```text
tau_internal: true
self_agent_id: engineer_parent
sub_agent_id: engineer_ab12cd34

Tool call `call_123` is running in the background.
```

Use `sub_agent_id` as `recipient_id`:

```text
message({"recipient_id":"engineer_ab12cd34","message":"Please also inspect crates/tau-cli/src/event_renderer.rs."})
```

The UI still displays the message. The recipient agent also receives a hidden internal prompt with the message body XML-escaped inside a `<message>` wrapper.

## Invalid recipients and arguments

A non-`user` recipient must be a live or pending `agent_id`. Otherwise the tool fails and no `agent.message` event is emitted.

If the id was never known, the tool reports an unknown recipient:

```text
message({"recipient_id":"engineer_missing","message":"hello"})
```

```text
unknown message recipient: `engineer_missing`
```

If the id belonged to an agent that has already finished or was canceled before it could start, the tool reports a stopped recipient:

```text
message({"recipient_id":"engineer_done","message":"hello"})
```

```text
stopped message recipient: `engineer_done`
```

Tool arguments are schema-validated before dispatch. Unknown extra fields are rejected before any logical tool invocation is logged.
