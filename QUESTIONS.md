# Open architectural questions — runtime-server per-client sessions

State after your second pass of feedback. Resolved items collapsed,
open items called out at the top.

---

## STILL OPEN — pick up next time

### A. Robot bridge per session (your #5)

You confirmed this is important. Not implemented in this change.

Today the Robot bridge populates a single thread-local registry in
`framework_core`. With multiple session threads in the sidecar, each
thread has its own thread-local — so each session naturally has its
own registry already. What's missing is the **routing layer**:

- The MCP / Robot poll currently runs once on the dev-server's serve
  thread and reaches into "the" registry.
- It needs to either run on each session thread (per-session bridge
  handle held by the session), or route an MCP call's session
  parameter to the right session thread for execution.

Sketch I'd propose:

1. Each session thread in the sidecar registers itself with a
   `RegistrySwitchboard` keyed by session id, exposing a handle that
   can run a closure on that thread (via an mpsc channel taking
   `Box<dyn FnOnce(&Registry) + Send>`).
2. MCP grows an optional `session: String` parameter on tool calls;
   `None` → "primary" (first session, for back-compat / simple cases).
3. `serve_with_robot_bridge` becomes per-session: the bridge poll
   targets the named session's switchboard slot.

Not blocking the rest of the work but it's the natural next milestone.

### B. In-client session-picker dev tool (your "future improvement")

Stub the design but don't build it yet. Likely shape:

- A small overlay primitive (rendered by the *client* not the server)
  that shows: current session id, list of other live sessions
  available on the server, and a "switch to" button.
- New wire messages: `AppToDev::ListSessions` /
  `DevToApp::SessionList(Vec<SessionDescriptor { id, identity, … })>`
  and `AppToDev::SwitchSession(String)` (the only sanctioned way to
  change session — and the server still has the final say).
- The mDNS TXT record stays as-is (just "find the server"); browsing
  sessions is a wire-protocol concern. (Your #8 — confirmed.)

### C. iOS / Android device labels

I left `device_label: None` on both native shells. The platform glue
should eventually surface the device's real label (UIDevice's
`model` + `name` on iOS, `Build.MODEL` + `Build.MANUFACTURER` on
Android) so the server's logs say "Pixel 8" rather than just
"Android". Trivial follow-up; bumped because it's nicer-to-have than
load-bearing.

---

## RESOLVED — what shipped

### #1 No URL for session selection (your feedback)
Ripped out. The web client now sends `WirePlatform::Web` + UA string
as identity but no longer reads `?session=` from the URL. Future
session selection will be in-client (Q B above).

### #2 Session lifecycle (you agreed not a concern)
Untouched.

### #3 Sessions stay warm on last disconnect (you agreed)
Already implemented — the host doesn't tear down a session when its
last client drops. SessionEnded only fires when the sidecar reports
it (panic / explicit shutdown).

### #4 Hot-patch coordination (your "client is responsible")
Already implemented. The server broadcasts post-patch state; clients
re-snapshot when epoch advances. No confirm-handshake.

### #6 No rate limit (you agreed dev tool)
Untouched.

### #7 Client identity in handshake, server issues id (your feedback)
Done. `AppToDev::Hello.identity: ClientIdentity { platform,
device_label }` carries the client's self-description. The server
*always* mints the session id (server-prefixed by platform:
`web_xxxxxxxx`, `ios_xxxxxxxx`, etc.). Clients never name sessions.

### #8 mDNS stays simple (you agreed)
TXT record carries `aas_sessions=multi` as a "this server supports
sessions" tag, but no per-session enumeration. Anything beyond
discovery lives in the wire protocol (see Q B above).

### #9 Sidecar respawn (already addressed last pass)
`SessionTracker` keeps the live session-id set Send + Sync; generated
host wrapper replays `CreateSession` after every respawn so existing
clients pick up where they left off.

### #10 iOS/Android FFI surface (revised this pass)
**Reverted**. The old `*_with_session` entrypoints are gone. There's
no client-side session opt-in anymore — iOS/Android just populate
`WirePlatform::Ios` / `WirePlatform::Android` automatically. Future
work to add device-label plumbing through the FFI (see Q C).

### #11 URL session params on web (your feedback)
Ripped out. See #1.

### #12 Multi-vs-single session is a server flag (your feedback)
Added `SessionMode` enum to `dev-server` + `SessionMode::from_env()`.
Generated host wrapper reads `IDEALYST_AAS_MULTI_SESSION` — default
is `PerClient`; `=0` flips to `Shared` (legacy synced-devices). Two
integration tests pin both modes.
