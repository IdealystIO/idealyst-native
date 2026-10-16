---
name: framework-purity
description: Framework crates must be free of platform-specific implementations — only traits, abstractions, and platform-agnostic logic.
targets:
  - crates/runtime/world
  - crates/runtime/scene
  - crates/runtime/vocabulary
  - crates/runtime/shared
  - crates/runtime/layout
  - crates/runtime/core
  - crates/runtime/macros
  - crates/runtime/macros-expand
  - crates/runtime/macros-parse
  - crates/runtime/template
  - crates/dev/wire
  - crates/dev/client
severity: high
---

# Framework purity

## Architectural rule

**The runtime layer (`crates/runtime/`) must be completely void of
platform-specific implementations.** Runtime crates define traits, types,
protocols, and platform-agnostic logic. Platform implementations live in
`crates/backend/*` and `crates/gpu-backend/*`; build/run tooling lives in
`crates/tools/build/` and `crates/tools/run/`.

## Background

The seam is `runtime_scene::Host` plus the `runtime_vocabulary::caps::*Ops`
traits; anything platform-shaped the runtime needs (a scheduler, a time
source, a render loop, an async executor) is an install point the backend
fills at boot (`runtime_shared::backend`, which documents the order). The
one sanctioned exception is `zone-offset`, its own crate so the OS
zone-database calls (libc / Win32) stay out of `runtime-shared` itself.

## Checklist

For each targeted crate:

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
      on a capability (`caps::*Ops`) method the backend implements. Flag any of:
      - A field, method, or accessor named `target_type` (or
        `kind_name`, `node_type`, `primitive_type`) that returns a
        `&str` / `String` — stringly-typed dispatch tags are the
        worst form of this smell.
      - Helpers that take `target_type: &str` / `kind: &str` and
        `match`/`if` on the value.
      - Matching on a primitive's kind (`PrimitiveKind`, a payload
        type, a scene `Element` variant) to do platform-shaped work
        *outside* the canonical dispatch: scene realize
        (`crates/runtime/scene/src/realize.rs`), whose job is to hand
        each payload to its registered handler, and the mount handlers
        (`crates/runtime/vocabulary/src/handlers/*.rs`), whose job is to
        call the matching `caps` method. If another file is matching on
        kind to compute a frame, decide an animation curve, or pick a
        layout strategy, that's a capability method waiting to happen.
      - Per-variant branches that compensate for one backend's
        quirk (see rule 7 in `CLAUDE.md`): the fix lives in the
        backend, not behind a variant check in the framework.
      
      Allowed: typed `PrimitiveKind` enums used for *introspection*
      (debug logging, accessibility-role inference, a11y trait
      lookup) — these don't drive rendering. If you're not sure
      which side of the line you're on, ask: "would a new backend
      need this branch?" If yes, it's introspection. If only some
      backends would, it's dispatch, and it belongs behind a
      capability method.

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
