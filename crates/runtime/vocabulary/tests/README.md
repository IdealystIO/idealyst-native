# runtime-vocabulary integration suites

These suites drive the runtime (world → scene → vocabulary handlers)
against **`host-mock`** (`crates/dev/host-mock`): a first-class
`runtime_scene::Host` + all-30-`caps::*Ops` recording mock. The
historical per-suite `impl Backend for Mini` + `LegacyBridge` plumbing
was retired ahead of the old-core deletion wave, so this test base
survived that deletion intact.

- `caps_conformance.rs` — the per-caps battery: every one of the 30
  capability traits (plus the seven `Host` structural ops) exercised at
  least once against `HostMock`, run-proving the "all 30 caps
  implemented directly" claim. Successor of the deleted `bridge.rs`
  (the `LegacyBridge` delegation proof, which went with `LegacyBridge`
  itself in the old-core deletion wave).
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
