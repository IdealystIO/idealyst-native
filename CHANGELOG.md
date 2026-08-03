# Changelog

Notable changes per release. Upgrade instructions live in `docs/` —
each entry links to its migration guide.

## Unreleased

### Added

- **Wall clock (`runtime_core::time`)** — calendar-time counterpart to
  the monotonic `TimeSource`: `WallClockSource { epoch_millis,
  local_offset_minutes }`, installed per backend at mount. Native
  defaults to a UTC `SystemTime` source via the same
  `install_default_time_source` hook; the web backend installs a
  `js Date`-backed source during bootstrap (real local offset,
  DST-aware per call) and macOS an `NSTimeZone`-backed one. Exists so
  UI can know the user's civil date ("today" in a date picker) —
  something a monotonic delta can never provide.
- **idea-ui date components** — `Calendar` / `RangeCalendar` (inline
  month grids with month/year zoom navigation, min/max/per-day
  disabling), `DatePicker` / `DateTimePicker` / `DateRangePicker`
  (anchored popup pickers on a `Select`-shaped trigger), `DateInput` /
  `DateTimeInput` (typed `Field` entry with lenient token parsing,
  blur canonicalization, and a calendar-button popup), and `TimeInput`.
  Backed by a new chrono-free `idea_ui::date` module: `CivilDate` /
  `CivilTime` / `CivilDateTime` (Hinnant civil-day math), token
  formatting/parsing (`YYYY-MM-DD`, `h:mm A`, …), and overridable
  `DateLabels` for i18n.
- **`Field.on_focus_change`** — optional `Rc<dyn Fn(bool)>` observing
  the input's focus transitions (the Field already bridged `on_focus`
  internally for its focus ring; this forwards the same signal to the
  host). The typed date/time inputs use it to normalize on blur.
- **Smart typing in the typed date/time inputs** — `DateInput` /
  `DateTimeInput` / `TimeInput` now mask appended keystrokes against
  their token format (`idea_ui::date_mask`): delimiters insert
  themselves (`07031994` → `07/03/1994`), a digit no segment can
  extend jumps ahead on its own (month `2` → `02/`), Tab completes an
  ambiguous partial segment in place (month `1` + Tab → `01/`), and an
  uncommittable partial (a bare `0` month) swallows the Tab. Deletions,
  mid-string edits and pasted non-conforming text bypass the mask and
  fall back to the lenient parser.
- **`Field.on_key_down`** — optional `Rc<dyn Fn(&KeyEvent) ->
  KeyOutcome>` forwarded to the inner `text_input`, so composed inputs
  can intercept keys before the platform default (the smart-typing Tab
  handling rides this).
- **`idealyst serve --precompressed`** — serves the `.br` sidecars a
  release `idealyst build --web` already stages (and `.gz` sidecars,
  if present) with `Content-Encoding` + `Vary: Accept-Encoding` when
  the browser accepts them, keeping the original file's Content-Type.
  Mirrors nginx `brotli_static` / Caddy `precompressed`, so release
  bundle transfer sizes and load times can be measured in a local
  browser. Files without a sidecar (and clients without the encoding)
  get the uncompressed bytes as before; the flag is off by default and
  other `serve_static` consumers (`idealyst dev`, `idealyst docs`) are
  unchanged.

### Fixed

- **Premint dump now executes `#[component(lazy)]` bodies.** The
  build-time CSS dump crawled every literal route but stopped at each
  lazy boundary's loading placeholder: its generated wrapper never
  enabled `async-driver` (so `mount_lazy` compiled the placeholder-only
  SSR branch) and pumped no executor (and with the feature unified on by
  an app dep, `spawn_async`'s `pollster::block_on` fallback could hang
  the dump on a pending-forever mount-time future). Styles constructed
  inside lazy screen bodies never minted, and `--premint-only` panicked
  at the uncrawled-sheet diagnostic on first navigation — idea-ui-docs'
  per-page splits hit this on all 50 pages. The dump wrapper now
  enables `async-driver` (matching the shipped web wrapper's posture),
  installs host-mock's queue executor, and pumps+flushes each route to
  a fixed point, resolving lazy generations by nesting depth. Regression
  pair: `premint-dump::lazy_body_styles_mint_after_pump` (mechanism) +
  `build-web`'s `dump_wrapper_resolves_lazy_boundaries` (wiring).

- **wasm-split call-graph misclassification under duplicate mangled
  names.** LLVM under `opt-level=z` emits distinct functions sharing one
  mangled name (small alloc/core/hashbrown monomorphizations; the
  idea-ui-docs release module carried 42 such names, including
  `alloc::fmt::format`). `wasm-split-cli` correlated the
  relocation-bearing pre-bindgen module with the bindgened module via
  name-keyed maps, so same-named copies collided: call-graph edges landed
  on an arbitrary copy, the other was classified chunk-only and gutted
  from the main bundle even though main-resident fmt vtables (function
  pointers in data segments) still referenced its table slot. Release
  builds with enough `#[component(lazy)]` split points then trapped at
  boot with `RuntimeError: function signature mismatch` before the first
  chunk request. One or two split points (the existing fixtures) never
  triggered it; the idea-ui-docs site with a lazy chunk per page did,
  reliably. Relocation targets now resolve by function *index* from the
  linking section (exact), the old→new function mapping unions all
  same-named copies (over-approximating reachability — a duplicate stays
  in main if any copy is main-reachable), and the reparented
  recovered-children edges are actually merged into the call graph
  (previously dropped). `emit_main_module` now also carries a tripwire —
  it refuses to gut a function that a main-reachable data symbol still
  points at, so any future classification regression fails the build
  loudly instead of emitting corrupt output. New fixture:
  `tests/lazy-many-splits` — 30 `#[component(lazy)]` pages behind a
  static fn-pointer catalog, wired into the `prune-regression` browser
  suite.

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
