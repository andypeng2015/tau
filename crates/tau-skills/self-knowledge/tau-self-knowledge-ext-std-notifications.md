---
name: tau-self-knowledge-ext-std-notifications
description: Use this extension skill when the user asks about Tau's std-notifications extension, prompt/response sounds, idle notifications, OSC 1337 user vars, terminal bells, idle summaries, or notification commands.
advertise: false
---

# Tau std-notifications extension self-knowledge

`std-notifications` is Tau's built-in notification extension. It runs `tau-ext-std-notifications`, is enabled by default, and reacts to prompt/response/idle events by emitting terminal-facing notification events.


## Features

Default `osc1337` mode emits iTerm2-style `SetUserVar` events:

- On user prompt submit: `user-notification = protoss-probe-ack`.
- On final provider response when no tool call is requested and no main-agent background tools remain: `user-notification = protoss-upgrade-complete`.
- After an idle window following a final response: `user-text-notification` JSON with urgency `normal`, title `Agent idle: <host>:<cwd>`, body `Waiting for user input`, and `app_name: tau`.

If `idle_agent_summary` is true, the idle path first asks the agent for a one-sentence summary and uses it as the notification body, falling back after 10 seconds. If `idle_command` is set, the extension also spawns that command for idle text notifications, appending the title as an argument, piping the body on stdin, and setting `NOTIFY_URGENCY=normal` and `NOTIFY_APP_NAME=tau`.

`bell` mode emits terminal bell events instead of OSC user vars for prompt/response sounds.


## Configuration

Configured under `extensions.std-notifications.config`:

```json5
extensions: {
  "std-notifications": {
    config: {
      mode: "osc1337",          // or "bell"
      idle_seconds: 60,
      idle_agent_summary: false,
      idle_command: ["notify-send", "--app-name=tau"],
    },
  },
}
```

The built-in default config sets `idle_seconds: 60` and `idle_agent_summary: false`. Downstream terminal or desktop tooling is responsible for turning OSC user-var changes into audible or visual notifications.
