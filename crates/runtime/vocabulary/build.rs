//! Declares the `--cfg` names this crate reads, so cargo does not warn
//! `unexpected_cfg_condition_name`.
//!
//! These come from `RUSTFLAGS` (set by `idealyst build --web`, see
//! build-web's `cargo_build_wasm`) rather than cargo features, because a
//! feature would have to be forwarded through every crate between the app
//! and here — `runtime-core`, `backend-web`, `idea-ui`, … — and unified
//! across all of them. That forwarding fragility is what sank the old
//! `prim-*` / `style-dynamic` gating model. A `RUSTFLAGS` cfg applies
//! uniformly to the whole build with nothing to forward.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    // `--premint`: the `stylesheet!` builder's all-constant fast path
    // returns a build-time class instead of building `StyleRules`.
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint)");
    // `--premint-only`: additionally compiles the runtime style engine OUT.
    // See `style_attach::PREMINT_ONLY_VIOLATION`.
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint_only)");
    // Set only for the ephemeral native dump binary that collects every
    // `stylesheet!` into `PREMINT_SHEETS`.
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint_dump)");
    // `--premint-report`: keeps the engine (so the app renders normally)
    // but logs every style that FELL THROUGH to it. The diagnostic for
    // "why can't this app use --premint-only?".
    println!("cargo::rustc-check-cfg=cfg(idealyst_premint_report)");
}
