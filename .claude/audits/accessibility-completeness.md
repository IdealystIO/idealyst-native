---
name: accessibility-completeness
description: The a11y surface (Role, PrimitiveKind, AccessibilityProps, WireAccessibilityProps, default_role) stays in lockstep across runtime-shared, the vocabulary's author surface and handlers, every backend, and the wire protocol.
targets:
  - crates/runtime/shared/src/accessibility.rs
  - crates/runtime/vocabulary/src/caps/a11y.rs
  - crates/runtime/vocabulary/src/glue.rs
  - crates/runtime/vocabulary/src/handlers
  - crates/runtime/vocabulary/src/backend.rs
  - crates/runtime/macros-parse/src/ast.rs
  - crates/runtime/macros-expand/src/ui.rs
  - crates/runtime/macros-expand/src/jsx.rs
  - crates/dev/wire/src/lib.rs
  - crates/backend/web
  - crates/backend/ios/mobile
  - crates/backend/android/mobile
  - crates/backend/macos
  - crates/gpu-backend/engine
  - crates/dev/server
  - crates/dev/client
severity: high
---

# Accessibility surface completeness

## Background

The a11y types live in `runtime_shared::accessibility`
(`crates/runtime/shared/src/accessibility.rs`: `Role`, `PrimitiveKind`,
`AccessibilityProps`, `default_role`) and thread through every primitive
payload, every mount handler, the `caps::A11yOps` capability
(`crates/runtime/vocabulary/src/caps/a11y.rs`), every backend's
`src/newcore.rs`, and the dev/app wire protocol. A new `Role` variant, a new
primitive, a new `create_*` capability method, or a new backend impl all
have to update several files at once or assistive technology silently
breaks for the affected primitive. This audit catches drift.

See `docs/accessibility-design.md` for the cross-platform mapping
tables that authoritatively define what each `Role` should map to per
backend. Reference impls:

- Web: `crates/backend/web/src/a11y.rs`
- iOS-mobile: `crates/backend/ios/mobile/src/imp/a11y.rs`
- Android: `crates/backend/android/mobile/src/imp/a11y.rs`
- macOS: `crates/backend/macos/src/imp/a11y.rs`
- wgpu: `crates/gpu-backend/engine/src/backend_impl.rs` (see `init_node_a11y`, `build_a11y_node`)

Roku is intentionally a no-op — see the separate
[`backend-roku-a11y`](backend-roku-a11y.md) audit; the present audit
does NOT flag Roku's `_a11y` discipline.

## Checklist

### Core surface

- [ ] **Every `PrimitiveKind` has a `default_role` arm.** `default_role`
      lives in `runtime_shared::accessibility`. Open the function, list
      every `PrimitiveKind::*` variant, and confirm each one has an
      explicit `=>` arm (no wildcard `_ =>` swallowing new variants).
      A missing arm means the primitive ships with no inferred role on
      any backend.
- [ ] **`PrimitiveKind` covers every node primitive.** Every builtin
      primitive payload in `crates/runtime/vocabulary/src/prims/` that
      mounts a node (skip control flow: `when` / `switch` / keyed lists /
      fragments) has a matching `PrimitiveKind` variant.
- [ ] **Every node primitive's payload carries an `a11y:
      AccessibilityProps` field**, and its mount handler
      (`crates/runtime/vocabulary/src/handlers/*.rs`) passes it to the
      `create_*` capability method (`backend.create_view(&prim.a11y)` is
      the shape). A handler that builds the node without it drops the
      author's a11y props on every backend.

### Author surface

The author-facing setters live on the glue wrappers in
`crates/runtime/vocabulary/src/glue.rs` (the `glue_wrapper_common!`
expansion, plus the hand-written wrappers such as presence): the
whole-struct `accessibility(props)` and one granular setter per writable
field. The `ui!` / `jsx!` `a11y_*` attributes lower to those setters.

- [ ] **Every glue wrapper exposes the same a11y setter set**:
      `accessibility`, `a11y_label`, `a11y_hint`, `a11y_role`,
      `a11y_hidden`, `a11y_traits`, `live_region` (check against the
      writable `AccessibilityProps` fields). A wrapper missing one makes
      that attribute a compile error on that primitive only. A new
      author-writable field on `AccessibilityProps` needs a setter on
      every wrapper.
- [ ] **The macro attr list matches the setters.** `is_a11y_attr` in
      `crates/runtime/macros-parse/src/ast.rs` is the single source of
      truth both `ui!` and `jsx!` consult; each recognized name must equal
      a glue setter name (the macros emit `.<name>(<value>)`). A setter
      added without updating `is_a11y_attr` is unreachable from the
      macros; a name in `is_a11y_attr` with no matching setter is a
      compile error at the call site.
- [ ] **`announce` is installed at boot.** The boot path in
      `crates/runtime/vocabulary/src/backend.rs` must call
      `runtime_shared::host::install_announcer(...)` with a closure
      forwarding to the active backend's
      `A11yOps::announce_for_accessibility`, alongside the URL-opener /
      fullscreen installs. Without it, `runtime_core::announce(...)` is a
      silent no-op in real apps.

### Backend coverage

For each of `crates/backend/web`, `crates/backend/ios/mobile`,
`crates/backend/android/mobile`, `crates/backend/macos`, and
`crates/gpu-backend/engine` — separately:

- [ ] **No `_a11y` parameters in `create_*` methods** (Roku excluded).
      Grep `_a11y: &` (an `AccessibilityProps` parameter) inside the
      backend's `caps::*Ops` impls in `src/newcore.rs` and the `imp/`
      code it delegates to. A finding means the backend accepts the prop
      bag but drops it on the floor — the node renders with no semantic
      info.
- [ ] **Each `create_*` calls `a11y::apply(...)`** (or the equivalent
      per-node setter for wgpu's `init_node_a11y`). Grep each backend's
      `create_*` body and verify the call is present, with an
      appropriate inferred role (e.g. `Role::Button` for `create_pressable`,
      not `None`). Cross-reference against the mapping table in
      `docs/accessibility-design.md` §1.
- [ ] **`update_accessibility` is overridden** — calls the same
      `apply` function used at create-time so the update path produces
      the same DOM/UIView/View/NSView state as the create path.
- [ ] **`announce_for_accessibility` is overridden** — wires to the
      platform's announcement API (`UIAccessibility.post`,
      `View.announceForAccessibility`, `NSAccessibility.post`,
      hidden `aria-live` region, wgpu announcement queue).
- [ ] **`dump_accessibility_tree`** — wgpu must override (returns
      `Some(AccessibilityTree)`). Native widget backends (web/iOS/
      Android/macOS) must NOT override — the platform AX walker reads
      the widget tree directly. An override on a native widget backend
      is a bug.

### Role coverage per backend

- [ ] **Every `Role` variant maps to a backend constant.** For each
      backend's role-translation function (`role_to_aria`,
      `role_to_traits_bits`, the Android role helper, the macOS
      `role_to_ns_role` / `role_to_ns_subrole`), confirm every `Role::*`
      variant is in the `match`. The `#[non_exhaustive]` attribute
      means a missing arm needs an explicit wildcard `_ =>` (with a
      comment explaining why the fallback is acceptable). A wildcard
      without a documenting comment is a finding.

### Wire protocol parity

- [ ] **`WireAccessibilityProps` mirrors `AccessibilityProps` field
      for field.** New fields on `AccessibilityProps` MUST land on
      `WireAccessibilityProps` in the same commit, plus update both
      `From<&AccessibilityProps>` and `From<WireAccessibilityProps>`
      bridges. Grep the wire crate for `struct WireAccessibilityProps`
      and diff against `struct AccessibilityProps`.
- [ ] **`WireRole` has a variant for every `Role` variant** (plus the
      `#[serde(other)] Unknown` fallback). A new `Role` variant
      without a matching `WireRole` arm means dev-side a11y silently
      becomes `None` on the app side.
- [ ] **Every `Create*` Command variant has an
      `a11y: WireAccessibilityProps` field** (typically with
      `#[serde(default)]` for backward-compat with v2 logs). A
      `Create*` variant without the field will drop a11y at the wire
      boundary.
- [ ] **`Command::UpdateAccessibility` and
      `Command::AnnounceForAccessibility` exist** and are emitted by
      the recorder and handled by the replayer.
- [ ] **`SceneModel` carries per-node a11y state** so a late-joining
      runtime-server client receives the current props via snapshot, not the
      `Default::default()` it would get from re-applying creates in
      isolation. See memory `project_aas_state_snapshot`.
- [ ] **`AccessibilityAction` handlers cross the wire as `HandlerId`
      trampolines.** `WireAccessibilityAction` carries
      `{ name: String, handler: HandlerId }`; the recorder registers
      the in-memory `Rc<dyn Fn()>` into its `HandlerTable` and the
      replayer reconstructs a closure that posts
      `AppToDev::Event { handler, args: Unit }` over the reverse
      channel. A `WireAccessibilityAction` shape that loses the
      `handler` field (or a replayer that synthesizes no-op handlers
      instead of trampolines) means AT-triggered AX actions on the
      app side never reach the dev-side closure.
- [ ] **`PROTOCOL_VERSION` bumped** (`WIRE_VERSION` is an alias) when
      any of the above wire-shape changes land. Check
      `crates/dev/wire/src/lib.rs` for the current version. A
      `WireAccessibilityProps` schema change without a version bump is a
      finding.

### Tests

- [ ] **`AccessibilityProps`/`WireAccessibilityProps` round-trip
      tests still exist** (`crates/dev/wire/tests/roundtrip.rs`).
- [ ] **Each backend with a non-trivial `apply` has at least one test**
      that exercises the create + update + announce path. wgpu's
      `a11y_tests` module is the model.
- [ ] **A new `Role` variant lands with a backend mapping test** so
      regressions in the mapping are caught (the cross-platform
      contract is the value-add of `Role`; a mapping bug is a
      cross-cutting break).
- [ ] **The author surface has setter + macro tests.** Something must
      pin that each `a11y_*` attribute in `ui!` / `jsx!` lands in the
      payload's `AccessibilityProps` and reaches `create_*`
      (`crates/runtime/vocabulary/tests/caps_conformance.rs` and
      `glue_host_surface.rs` are the nearest existing coverage — the
      latter pins the announcer install). A setter or attr with no test
      reaching it is a finding.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium / high
- **Location**: `crate/src/file.rs:line`
- **Issue**: one-line description
- **Why**: brief reasoning (1–3 sentences) — name which checklist
  item triggered the finding.
- **Suggested fix**: actionable recommendation, or "needs design
  discussion".

End with a one-line summary: `Result: N high, M medium, K low findings.`
