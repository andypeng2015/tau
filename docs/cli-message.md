# Send a message from the shell

```sh
tau message '&my-session' 'Please check the build'
printf '%s' 'What failed?' | tau message --wait-response 'my-session/@engineer'
```

`tau message [--wait-response] <recipient> [message]` sends one literal UTF-8
message. Omit the message argument to read stdin through EOF. Messages and final
responses are limited to 64 KiB.

Recipient forms match the `message` tool:

- `agent-id`: local to `TAU_SESSION_ID` when set; otherwise the unique live agent
  found across running sessions. Ambiguity fails rather than choosing a session.
- `&session-id`: the session's configured inter-session receiver.
- `[&]session-id/[@]agent-id`: an exact agent in an exact session; both sigils are
  independently optional.

Quote addresses containing `&` to protect them from the shell. A bare session
uses the existing receiver policy, including its configured receiver auto-start.
The CLI itself creates no sender agent, model request, or temporary session.
There is no `tau query` alias.

Without `--wait-response`, success means the recipient's receive fact committed;
stdout is empty. Its envelope tells the recipient not to reply. With the flag,
the recipient gets an opaque address for **one final response only**, delivered
using its `message` tool. Progress and acknowledgements must not be sent there.
The address works only inside the receiving harness, not through delegated
cross-harness replies.

Stdout contains exactly the final response, with no added newline. Delivery
acknowledgement and errors go to stderr. Delivery rejection, unsupported old
harnesses, connection loss, and output failures exit unsuccessfully. Waits have
no mandatory deadline; use ordinary shell timeout tools if desired.

Ctrl-C/process exit closes the response address but does not cancel already
delivered recipient work. A late or duplicate reply fails. If the connection
closes before delivery acknowledgement, delivery is uncertain; the command does
not retry or resend. Wait-response requires a receiving harness with protocol
10.4 or newer, as does one-way CLI delivery.
