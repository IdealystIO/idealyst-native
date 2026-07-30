# Changelog

Notable changes per release. Upgrade instructions live in `docs/` —
each entry links to its migration guide.

## 1.0.1

### Added

- **`canvas_vello::register_from_chunk`** — registers the web canvas
  renderer from a lazily-loaded chunk, so the vello painter can stay out
  of the main bundle. Without it the canvas renderer is anchored at
  boot and the lazy split is not expressible.

### Fixed

- **`runtime_core::log_debug!` / `log_info!` / `log_warn!` / `log_error!`
  were unresolvable.** The macros live in `runtime-shared` and were not
  re-exported through the facade, so the spelling apps have used since
  0.5.x stopped resolving.
- **`MediaStream` had no `PartialEq`**, which the world kernel requires
  of any signal payload — so a camera or screen-share stream could not
  be held in state at all. Now compares by pointer identity
  (`Rc::ptr_eq`), the same shape used for other handle-like payloads.
- **`GlueTextArea` did not expose `wrap` / `code_mode`**, which the
  underlying builder supports.

## 1.0.0

**The rendering core was replaced.** The reactive arena, the render
walker, the `Element` enum and the 159-method `Backend` trait are gone,
succeeded by four crates: `runtime-world` (per-world signal arenas with
staged commits), `runtime-scene` (structural element model, `realize`,
`Host`, `Registry`), `runtime-vocabulary` (per-capability backend traits
and the builtin primitive handlers), and `runtime-shared` (the substrate
that outlived the walker — style engine, assets, animation, scheduling,
robot). `runtime-core` remains as the author-facing facade.

→ **[Upgrade guide: 0.5.x to 1.0](docs/migration-0.5-to-1.0.md)**

### Breaking

- **Writes stage until flush.** `set` no longer makes the value visible
  to the next `get()` in the same turn. Read-modify-write must go
  through `update`, whose closure now takes `&T` and returns the new
  value. This changes behaviour *without* a compile error — the upgrade
  guide's step 4 covers how to audit for it.
- **`T: PartialEq` bounds the whole signal handle**, not just the
  guarded `set` — creation and `get` included. Payloads without
  meaningful value equality need a pointer-identity impl.
- `Signal::new(v)` → the free `signal(v)`; `get_untracked()` → `peek()`;
  `batch(…)` and `update_if_changed(…)` removed.
- **`on_cleanup` in a component body now panics** — return the cleanup
  from an effect. Likewise creating signals/effects/memos, or calling
  free theme functions, inside an event handler: handlers run outside
  the world.
- **19 names removed from the `runtime_core` author surface** (of 396;
  the other 377 are unchanged). Full inventory in the upgrade guide —
  External-SDK authoring, custom-navigator authoring, `Owner` /
  detached-scope building, and three payload-serde functions that
  *moved* to `wire::payload_serde` rather than being removed.
- **`idealyst build --primitives=<list>` is a hard error**, and the
  `prim-*` cargo feature families are gone. Per-primitive registration
  in `handlers::register_builtins` replaces them.

### Added

- **Deferred handler registration** — `registry.defer::<P>()` at boot,
  `registry.register_deferred::<P, _>(handler)` from a lazily-loaded
  chunk, plus a `defer_registration` mailbox. Lets a heavy extension SDK
  stay out of the main bundle: measured 1278 KiB → 766 KiB on the
  in-tree fixture. Parking is opt-in per payload kind; an *undeclared*
  unknown payload still panics.
- `runtime_scene::realize_detached` for detached subtree construction.

### Fixed

- **The dev-server sidecar installed no monotonic clock**, so
  `now_micros()` read `0` and every tween resolved against t=0 —
  animations emitted a constant value forever while the wire flooded
  with updates. Affected `idealyst dev` in its default (runtime-server)
  mode only; `--local` was unaffected.
- Test files gated on a deleted cargo feature compiled to **zero tests**
  silently rather than erroring; the affected suites were restored.

### Unchanged

- **Wire protocol stays at `PROTOCOL_VERSION = 17`** — 742 declarations,
  byte-identical to 0.5.2. No dev/app lockstep upgrade needed.
- `ui!`, `jsx!`, `#[component]`, `stylesheet!`, the primitive set, and
  the `idea-ui` component library keep their spelling.

## 0.5.2 and earlier

See the version-pair guides in `docs/`:
[0.1 → 0.2](docs/migration-0.1-to-0.2.md) (navigation moved to the
outlet model) and [0.2 → 0.3](docs/migration-0.2-to-0.3.md) (reactive
surface unification).
