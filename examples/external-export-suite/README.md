# external-export-suite

Two Idealyst components exported to **every supported front-end framework**.

- [`src/lib.rs`](src/lib.rs) — the components:
  - `Greeter` — a reactive **string** prop + a **void** callback.
  - `Stepper` — reactive **string** + **number** props + a callback that
    **carries a value**.
  - Between them they cover every prop kind that can cross the JS boundary.
- [`consumers/`](consumers/) — a standalone app per framework (vanilla,
  React, Vue, Svelte, Angular) that uses both components.

## Build

```bash
# Generate the Web Component suite into dist/external/
idealyst export examples/external-export-suite

# Run the no-build consumers (vanilla + Vue) — served statically:
idealyst serve examples/external-export-suite --port 8080
#   → http://localhost:8080/consumers/vanilla/
#   → http://localhost:8080/consumers/vue/
```

The React / Svelte / Angular consumers run with their own toolchains — see
[`consumers/README.md`](consumers/README.md).

See [`docs/external-export.md`](../../docs/external-export.md) for how the
export pipeline works.
