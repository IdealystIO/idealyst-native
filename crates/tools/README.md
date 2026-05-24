# `tools/` — user-facing orchestration

Crates here are *not* part of the runtime. They're the things you
invoke from a shell: scaffold a project, drive a build, run an app,
port code in from another framework. An Idealyst app can run
without ever touching `tools/` — the CLI is workflow sugar, not a
dependency of the framework itself.

| Group | Path | Role |
| --- | --- | --- |
| [`cli/`](./cli) | The `idealyst` command. Single entry point that dispatches into the `build/`, `run/`, and `port/` orchestrators. Subcommands: `new`, `dev`, `build`, `run`. |
| [`build/`](./build) | Per-platform build orchestration. Each sub-crate (`android`, `ios`, `macos`, `roku`, `sim`, `terminal`, `web`, `runtime-server`) generates the ephemeral host crate, picks the right toolchain, and drives `cargo` to produce a deployable artifact. |
| [`run/`](./run) | Post-build launchers. `run-android` builds the APK + DEXes + signs + installs + launches without Gradle. `run-ios` builds + codesigns + installs via Simulator/Devices. Likewise for `macos`, `roku`, `terminal`. |
| [`port/`](./port) | Source-language porters. Translates React, Solid, Vue, Svelte into Idealyst Rust. `port-core` is the shared IR; per-framework crates are lifters; `port-project` walks a directory or git URL; `port-preview` scaffolds a check-only crate to validate the output compiles. |

## Why one tree

`cli/`, `build/`, `run/`, `port/` are all things a user invokes via
the same `idealyst` binary. Keeping them under `tools/` makes the
"what you type" surface its own group, distinct from the framework
crates an app *runs against*.

## What the CLI does that's load-bearing

When you `idealyst new`, you get one crate — the app. When you
`idealyst dev` or `idealyst build`, the CLI generates an ephemeral
*host crate* for each platform in a build directory and compiles it
against your app. You never write the wasm-bindgen entry, the JNI
bridge, the `UIApplicationDelegate`, or the Roku manifest — `tools/`
does. The generated source is readable if you want to look, but
not part of the project's source tree.
