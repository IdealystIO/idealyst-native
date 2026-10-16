# runtime-vocabulary integration suites

These suites drive the runtime (world → scene → vocabulary handlers)
against **`host-mock`** (`crates/dev/host-mock`): a first-class
`runtime_scene::Host` + all-31-`caps::*Ops` recording mock.

- `caps_conformance.rs` — the per-caps battery: every capability trait
  except `GridOps` (plus the `Host` structural ops) exercised at least once
  against `HostMock`. `grid_caps_wired.rs` separately checks that every
  backend shipping a two-axis grid engine actually wires `GridOps`.
- `vocab.rs`, `navigator.rs`, `lazy.rs`, `portal_presence.rs`,
  `virtualizer_graphics.rs` — handler/behavior suites on the host-mock
  harness (op-log shapes, teardown probes, callback captures, pumped
  scheduler/executor where timing windows matter).
- `walker_ports.rs` — ports of the behavioral coverage from
  `crates/dev/mock-backend`'s old-walker integration tests (navigator
  URL sync, nested teardown, keepalive, theme cohort, …); the header
  comment maps each port to its origin test.

Op-log format, capability flags, callback captures, and the pump
utilities are documented in `host-mock`'s crate docs
(`crates/dev/host-mock/src/lib.rs`).
