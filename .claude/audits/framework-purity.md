---
name: framework-purity
description: Framework crates must be free of platform-specific implementations — only traits, abstractions, and platform-agnostic logic.
targets:
  - crates/runtime/core
  - crates/runtime/macros
  - crates/runtime/layout
  - crates/dev/wire
  - crates/dev/client
  - crates/runtime/reactive/arena
  - crates/runtime/reactive/refs
severity: high
---

# Framework purity

## Architectural rule

**The `framework/` layer must be completely void of platform-specific
implementations.** Framework crates define traits, types, protocols, and
platform-agnostic logic. Platform implementations live in `backend/`;
build/run tooling lives in `crates/tools/build/` and `crates/tools/run/`.

## Background

Today `framework-core` reaches into `objc2` / `objc2-foundation` /
`block2` to drive a CADisplayLink-substitute on iOS, and into
`wasm-bindgen` / `web-sys` for the rAF render loop on web. These are
platform implementations sitting in the framework layer. The right shape
is: framework defines a `RenderLoop` (or similar) trait; each `backend/*`
crate implements it for its platform.

## Checklist

For each framework crate (especially runtime crates — `core`,
`native-layout`, `wire`, `dev-*`):

- [ ] **Platform-specific dependencies** — scan `Cargo.toml` for
      `objc2*`, `block2`, `jni`, `ndk*`, `wasm-bindgen`, `web-sys`,
      `js-sys`, `windows*`, `core-foundation`, or any other
      platform-bound dep. Each is a finding unless it's a portable
      abstraction (e.g. `raw-window-handle` is fine — it's a handle
      enum, not a platform implementation).
- [ ] **`#[cfg(target_os = …)]` / `#[cfg(target_arch = …)]`** — any
      target-gated *implementation* (not just a portability shim
      around a stdlib gap) is a finding. The framework should be
      compiled the same way for every target.
- [ ] **Direct platform-API calls** — grep for `extern "C"`,
      `extern "system"`, Objective-C selectors (`sel!`, `msg_send!`),
      JNI function names, `JNIEnv`, `JavaVM`, raw `CFRetain`/`CFRelease`,
      etc. inside framework crates.
- [ ] **Backend imports** — framework must not depend on or `use`
      anything from `crates/backend/*`. The dependency direction is
      backend → framework.
- [ ] **Conditional compilation cliffs** — if removing a `cfg` gate
      would make the crate fail to compile on another platform, the
      gated code is platform-specific. Confirm it shouldn't be moved
      to a backend trait impl.
- [ ] **`target_type` / variant-dispatch smell** — framework code
      must not branch on *what kind of primitive/target* it has in
      order to do rendering or behavior work. That decision belongs
      on the `Backend` trait. Flag any of:
      - A field, method, or accessor named `target_type` (or
        `kind_name`, `node_type`, `primitive_type`) that returns a
        `&str` / `String` — stringly-typed dispatch tags are the
        worst form of this smell.
      - Helpers that take `target_type: &str` / `kind: &str` and
        `match`/`if` on the value.
      - `match primitive { Primitive::X => …, Primitive::Y => … }`
        or chained `if let Primitive::X { .. }` blocks in framework
        code *outside* the canonical walker (the dispatch site
        whose entire job is to call `backend.create_X(...)` per
        variant — `crates/runtime/core/src/walker/*.rs` today). If
        another file is matching on variant to do platform-shaped
        work (compute a frame, decide an animation curve, pick a
        layout strategy), that's a Backend method waiting to
        happen.
      - Per-variant branches that compensate for one backend's
        quirk (see rule 7 in `CLAUDE.md`): the fix lives in the
        backend, not behind a variant check in the framework.
      
      Allowed: typed `PrimitiveKind` enums used for *introspection*
      (debug logging, accessibility-role inference, a11y trait
      lookup) — these don't drive rendering. If you're not sure
      which side of the line you're on, ask: "would a new backend
      need this branch?" If yes, it's introspection. If only some
      backends would, it's dispatch, and it belongs behind a
      Backend method.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium / high
- **Location**: `crate/src/file.rs:line` or `crate/Cargo.toml:line`
- **Issue**: one-line description
- **Why**: explain which platform this couples the framework to.
- **Suggested fix**: name the trait/abstraction the framework should
  expose and the backend crate that should implement it, or "needs
  design discussion".

End with a one-line summary: `Result: N high, M medium, K low findings.`
