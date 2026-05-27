---
name: ffi-safety
description: Unsafe blocks, raw pointer handling, panic boundaries, and ownership across FFI for native backends.
targets:
  - crates/backend/ios/core
  - crates/backend/ios/mobile
  - crates/backend/ios/tv
  - crates/backend/ios-stack
  - crates/backend/android/core
  - crates/backend/android/mobile
  - crates/backend/android/tv
  - crates/backend/roku
  - crates/backend/web
  - benchmark/idealyst-native/wasm
severity: high
---

# FFI safety

## Background

Each backend bridges Rust to a foreign runtime: Objective-C/Swift on iOS,
JNI on Android, BrightScript on Roku, JS via wasm-bindgen on web. Panics
that unwind across an FFI boundary are UB; raw pointers and JNI globals
must be tracked carefully. The codebase has prior FFI footguns (e.g.
`dispatch_get_main_queue` being a macro on iOS — see auto-memory
`feedback_robot_feature`).

## Checklist

- [ ] **`unsafe` blocks** — every `unsafe` block should have a `// SAFETY:`
      comment naming the invariant. Flag any block without one.
- [ ] **Panic boundaries** — any `extern "C"` or JNI-exposed function must
      catch panics (`catch_unwind`) before returning to the foreign caller.
      Flag exported functions that can panic without a guard.
- [ ] **Raw pointer lifetimes** — `*mut T` / `*const T` returned to or
      received from the foreign side must have documented ownership.
      Flag `from_raw` / `into_raw` pairs that span functions without
      a clear handoff comment.
- [ ] **JNI globals** (Android) — `NewGlobalRef` must be matched by
      `DeleteGlobalRef`. `JNIEnv` references must not be cached across
      threads.
- [ ] **Objective-C retain/release** (iOS) — bridging retained vs
      autoreleased objects. Flag `msg_send![release]` / `retain` patterns
      that look unbalanced.
- [ ] **wasm-bindgen drops** — closures passed to JS via
      `Closure::wrap`/`forget` must have an owner; flag `.forget()` without
      a corresponding cleanup hook.
- [ ] **Roku transpile boundary** — any place where Rust types are encoded
      for BrightScript should validate that the encoded form is total
      (no `unreachable!` on values the foreign side could send).
- [ ] **Thread affinity** — UI calls that must run on the main thread
      should be gated or documented. Flag UI-touching FFI called from
      background contexts.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium / high
- **Location**: `crate/src/file.rs:line`
- **Issue**: one-line description
- **Why**: brief reasoning, including what the worst-case symptom is
  (crash, UB, leak, deadlock).
- **Suggested fix**: actionable recommendation, or "needs design discussion"

End with a one-line summary: `Result: N high, M medium, K low findings.`
