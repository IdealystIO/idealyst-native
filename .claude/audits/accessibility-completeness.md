---
name: accessibility-completeness
description: The a11y surface (Role, PrimitiveKind, AccessibilityProps, WireAccessibilityProps, default_role) stays in lockstep across framework-core, every backend, and the wire protocol.
targets:
  - crates/runtime/core/src/accessibility.rs
  - crates/runtime/core/src/backend.rs
  - crates/runtime/core/src/element.rs
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

The a11y surface lives in `framework_core::accessibility` and threads
through every primitive, every backend, and the dev/app wire protocol.
A new `Role` variant, a new `Element` variant, a new `create_*`
method, or a new backend impl all have to update several files at
once or assistive technology silently breaks for the affected
primitive. This audit catches drift.

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
      lives in `framework_core::accessibility`. Open the function, list
      every `PrimitiveKind::*` variant, and confirm each one has an
      explicit `=>` arm (no wildcard `_ =>` swallowing new variants).
      A missing arm means the primitive ships with no inferred role on
      any backend.
- [ ] **`PrimitiveKind` covers every `Element` variant** that
      represents a node (skip `When` / `Switch` / `Repeat` — they're
      control-flow only). Grep `enum Element` and `enum PrimitiveKind`;
      anything in the former whose `accessibility:` field exists must
      have a matching `PrimitiveKind` variant.
- [ ] **Every `Element` node variant carries `accessibility: AccessibilityProps`.**
      Grep `enum Element { ... }` for variants without an
      `accessibility:` field. The constructors / `ui!` macros must also
      default-initialize the field.

### Backend coverage

For each of `crates/backend/web`, `crates/backend/ios/mobile`,
`crates/backend/android/mobile`, `crates/backend/macos`, and
`crates/gpu-backend/engine` — separately:

- [ ] **No `_a11y` parameters in `create_*` methods** (Roku excluded).
      Grep `_a11y: &framework_core::accessibility::AccessibilityProps`
      inside `impl Backend for`. A finding means the backend accepts
      the prop bag but drops it on the floor — the node renders with
      no semantic info.
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
- [ ] **`WIRE_VERSION` bumped** when any of the above wire-shape
      changes land. Check `crates/dev/wire/src/lib.rs` for the
      current version; the design doc records v4 as the latest
      a11y-complete version (v3 added the props envelope; v4 added
      `HandlerId`-trampolined `AccessibilityAction` handlers). A
      WireAccessibilityProps schema change without a version bump is
      a finding.

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
