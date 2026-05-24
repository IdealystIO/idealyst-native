# runtime-core test suites

Each `tests/<name>.rs` is one **suite**. Suites are independent test
binaries — they share `tests/common/` infrastructure (MockBackend,
counted-fire helpers, TestRuntime) but otherwise compile and run on
their own.

## Layout

```
tests/
├── common/                ← shared infrastructure, not a suite itself
│   ├── mod.rs
│   ├── mock_backend.rs    ← records every Backend call into an event log
│   ├── counted.rs         ← fire-counted Effect / Memo wrappers
│   └── runtime.rs         ← TestRuntime: MockBackend + sync scheduler
├── reactive.rs            ← suite entry, declares its sub-modules
├── reactive/
│   ├── smoke.rs           ← scaffolding sanity checks (first to break)
│   ├── topology.rs        ← (planned) diamond / chain / fan-out
│   ├── memo.rs            ← (planned) memo + memo_with
│   ├── batch.rs           ← (planned) grouped writes
│   ├── on_cleanup.rs      ← (planned) cleanup ordering + lifetime
│   ├── context.rs         ← (planned) provide / inject
│   └── resource.rs        ← (planned) async + cancellation
├── walker.rs              ← (planned) primitive dispatch + lifecycle
├── walker/
│   ├── primitives.rs
│   ├── lifecycle.rs       ← when / switch / presence mount-unmount
│   └── refs.rs            ← .bind / fill ordering
├── style.rs               ← (planned)
├── style/
│   ├── resolution.rs
│   ├── tokens.rs
│   └── overrides.rs
├── identity.rs            ← (planned)
├── external.rs            ← (planned)
└── README.md
```

Each suite's `tests/<name>.rs` is **the test binary's root**. Submodules
are declared with explicit `#[path]` because integration-test binaries
don't follow Cargo's `src/` directory convention for `mod foo;`:

```rust
// tests/reactive.rs
#[path = "common/mod.rs"]
mod common;

#[path = "reactive/smoke.rs"]
mod smoke;

#[path = "reactive/topology.rs"]
mod topology;
```

## Scoping how you run

### One whole suite

```bash
cargo test -p runtime-core --test reactive
```

### One module inside a suite

```bash
cargo test -p runtime-core --test reactive topology::
cargo test -p runtime-core --test walker lifecycle::
```

### One specific test

```bash
cargo test -p runtime-core --test reactive topology::diamond
```

### Everything

```bash
cargo test -p runtime-core
```

### Feature combinations

Some test modules cover feature-gated APIs (`resource()` under
`async-driver`, `Robot` under `robot`, debug events under
`debug-stats`). Run the matrix when you change those areas:

```bash
# Default features
cargo test -p runtime-core

# Add each feature in turn (catches feature-gate slips)
cargo test -p runtime-core --features async-driver
cargo test -p runtime-core --features robot
cargo test -p runtime-core --features debug-stats
cargo test -p runtime-core --features hot-reload

# Everything together
cargo test -p runtime-core --all-features
```

The `xtask test-matrix` script (planned) automates the matrix.

## Coverage measurement

```bash
# Branch coverage report (HTML)
cargo llvm-cov --html --branch -p runtime-core --all-features

# Just the number, for CI
cargo llvm-cov --branch --summary-only -p runtime-core --all-features

# Mutation testing (slow — run nightly, not per-commit)
cargo mutants -p runtime-core
```

Coverage targets:
- **≥90% branch coverage**, measured under `--all-features`
- **≥85% mutation kill rate**

Both gated in CI once we have an established baseline.

## Writing a new suite

1. Pick a concern name (`reactive`, `walker`, `style`, `identity`,
   `external`, `external-registry`, etc.).
2. Create `tests/<name>.rs` with the standard preamble:
   ```rust
   #[path = "common/mod.rs"]
   mod common;

   #[path = "<name>/<topic>.rs"]
   mod <topic>;
   ```
3. Create `tests/<name>/<topic>.rs` with the test module.
4. Run `cargo test -p runtime-core --test <name>` to verify.

Avoid clever proc-macro-based test generators. The explicit `#[path]`
declarations are slightly verbose but mean the source tree matches
what the test binary actually contains.

## Assertion conventions

- **Fire counts before values.** When testing reactive behavior, use
  `counted_effect` / `counted_memo` and assert on the counter first,
  the value second. Most reactive bugs are "ran too many times" or
  "ran too few times" — final-value-only assertions miss them.
- **Use `MockBackend::assert_events` for walker tests.** Don't try to
  inspect the framework's internal state; let the backend record what
  the walker called and assert on that sequence.
- **Document framework behavior in test prose.** Tests that prove a
  specific framework design choice (Signal::set always notifies, memo
  fires twice on creation, etc.) should have a doc comment naming
  the behavior. The test then doubles as executable documentation.
- **No `sleep` for async tests.** Drive futures manually so tests are
  deterministic. The `TestRuntime` handles synchronous scheduling.

## What's NOT in this directory

- **Inline `#[cfg(test)]` modules.** Already exist in source files
  (`reactive.rs`, `style.rs`, `identity.rs`, etc.). They cover
  implementation details a public-API integration test can't reach.
  Keep them for what they're for; move public-API tests here.
- **trybuild compile-fail tests.** Live in `tests/compile_fail/` (when
  added). They validate type-system invariants — `Ref<ButtonHandle>`
  binding to a view, `Signal<NotSend>` not accidentally becoming
  `Send`, etc.
- **Property tests.** Use `proptest!` macros inside the relevant suite
  module. No separate top-level directory needed.
- **Benchmarks.** Live in `benches/`, not `tests/`. Not part of the
  correctness-coverage push.
