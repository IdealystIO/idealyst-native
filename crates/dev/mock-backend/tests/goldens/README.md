# Frozen wire snapshot

`static_tree_catchup_snapshot.json` is the **canonical catch-up
snapshot** the recorder produced for the static-tree scene under the
pre-runtime-v2 **walker**, serialized and indented.

The wire protocol is the compatibility contract between the dev server
and every runtime-server client. The gate this file backs is the
strongest form of that contract: *a late-joining client must not be able
to tell which implementation recorded the session.*
[`../wire_behavior.rs`](../wire_behavior.rs) compares the current
recorder's snapshot (`WireHarness::mount` over
`dev_server::newcore::SceneSession`) against this file exactly.

## Why it exists

The original gate ran both implementations in one process and compared
them, which needed a hand-built `runtime_core::Element` literal for the
reference tree — the single most `Element`-coupled fixture in the
dev-chain suites. Both died with the walker. Freezing the snapshot
before the deletion kept the wire-identity gate alive: the `Element`
literal and the in-process comparison are gone, and the frozen JSON
remains the reference.

## Regenerating

**There is no regeneration path.** The freeze call site went away with
the walker, and the only thing a re-baseline could do is record the
CURRENT implementation's output — permanently discarding the walker's
testimony, with no way to recover it. So:

- A failure here means the wire emission drifted. That is a
  **protocol-compatibility break** for every existing client, not a test
  nit. Fix the code.
- Never re-baseline to make a red test green.
- A deliberate protocol change (a new `Command`, a changed field) is the
  one legitimate reason to hand-edit this file — bump the protocol
  version in the same change and review the JSON diff as its substance.
