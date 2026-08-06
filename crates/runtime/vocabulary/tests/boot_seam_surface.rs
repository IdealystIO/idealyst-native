//! Every backend boot entry must offer a `BuiltinSet` seam.
//!
//! The bundle-size lever only works if the app can choose its primitive set
//! at the boot seam, and it silently no-ops the moment ONE reachable entry
//! still names `AllBuiltins` — that straggler re-anchors the whole
//! vocabulary and nothing shrinks. Three separate instances of this were
//! measured while the seam was built:
//!
//!  1. `start_in` converted but `hydrate_in` not (0.4% instead of 35%),
//!  2. `hydrate_in_with`'s no-server-DOM fallback calling the non-generic
//!     `start_in` — which was also a correctness bug, since the fallback
//!     registered a different set than the caller asked for,
//!  3. the non-generic convenience entries being codegen'd into their rlib
//!     (hence `#[inline]` on every one of them).
//!
//! Those are linker-level properties a unit test cannot observe, so this
//! guards the reachable proxy: the source of every backend that boots a
//! registry must expose a `_with`-style entry and must not call the
//! non-generic `register_builtins` outside of tests. A new backend that
//! copies an old one's boot fn fails here rather than quietly costing every
//! app on that platform ~65 KB.
//!
//! The same "code that never runs" shape covers the boot seam's other
//! obligation: forwarding the backend's environment capabilities into the
//! ambient thread-locals (`install_env_services`). See
//! `every_backend_boot_installs_env_services` below.

use std::path::{Path, PathBuf};

/// Backend boot files that construct a `Registry` and register builtins.
const BOOT_FILES: &[&str] = &[
    "crates/backend/web/src/newcore.rs",
    "crates/backend/web/src/newcore_hydrate.rs",
    "crates/backend/macos/src/newcore.rs",
    "crates/backend/cpu/src/newcore.rs",
    "crates/backend/terminal/src/newcore.rs",
    "crates/backend/linux/src/newcore.rs",
    "crates/backend/windows/src/newcore.rs",
    "crates/backend/roku/src/newcore.rs",
    "crates/backend/ios/mobile/src/newcore.rs",
    "crates/backend/android/mobile/src/newcore.rs",
    "crates/backend/ssr/src/newcore.rs",
    "crates/gpu-backend/engine/src/newcore.rs",
];

fn repo_root() -> PathBuf {
    // <root>/crates/runtime/vocabulary
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repo root")
        .to_path_buf()
}

/// Strip `#[cfg(test)]` modules so test-only `register_builtins` calls (of
/// which there are many, and which are fine) don't trip the scan.
fn without_test_modules(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("#[cfg(test)]") {
            // Skip the attribute and, if the next line opens a module,
            // everything through its matching brace.
            if let Some(next) = lines.peek() {
                if next.contains("mod ") {
                    let mut depth = 0usize;
                    let mut started = false;
                    for inner in lines.by_ref() {
                        depth += inner.matches('{').count();
                        started |= depth > 0;
                        depth = depth.saturating_sub(inner.matches('}').count());
                        if started && depth == 0 {
                            break;
                        }
                    }
                    continue;
                }
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[test]
fn every_backend_boot_offers_a_builtin_set_seam() {
    let root = repo_root();
    let mut missing: Vec<String> = Vec::new();

    for rel in BOOT_FILES {
        let path = root.join(rel);
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            // A backend that isn't checked out (or was renamed) should not
            // fail the suite; the list is the contract, not the filesystem.
            Err(_) => continue,
        };
        let src = without_test_modules(&src);

        if !src.contains("BuiltinSet") {
            missing.push(format!("{rel}: no `BuiltinSet` seam"));
        }
        if !src.contains("register_builtins_with") {
            missing.push(format!("{rel}: does not forward a set to the registry"));
        }
        // The non-generic `register_builtins` hardcodes `AllBuiltins`.
        for (i, line) in src.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") || t.starts_with("///") {
                continue;
            }
            if t.contains("register_builtins(") {
                missing.push(format!(
                    "{rel}:{}: calls the non-generic `register_builtins`, which \
                     pins `AllBuiltins` and re-anchors every handler — forward \
                     the caller's `S` instead",
                    i + 1
                ));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "backend boot seams missing the BuiltinSet lever:\n  {}",
        missing.join("\n  "),
    );
}

/// The convenience entries (`start`, `hydrate`, …) delegate with
/// `AllBuiltins`. As plain non-generic `pub` fns they get codegen'd into
/// their rlib and can survive to the final link, instantiating the full
/// vocabulary even for an app that selected a smaller set. `#[inline]` is
/// what stops that, so it is load-bearing rather than a perf hint.
#[test]
fn all_builtins_delegates_are_inline() {
    let root = repo_root();
    let mut bad: Vec<String> = Vec::new();

    for rel in BOOT_FILES {
        let path = root.join(rel);
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.contains("::<runtime_vocabulary::AllBuiltins") {
                continue;
            }
            // Walk back past the enclosing fn's own signature (which may
            // span several lines) into its attribute block, and require
            // `#[inline]` there. Stopping at the first `pub fn` would stop
            // at the signature we are inside.
            let mut inline = false;
            let mut passed_signature = false;
            for back in lines[..i].iter().rev().take(60) {
                let t = back.trim_start();
                if t.starts_with("pub fn ") || t.starts_with("fn ") {
                    if passed_signature {
                        break; // reached the PREVIOUS item
                    }
                    passed_signature = true;
                    continue;
                }
                if passed_signature {
                    if t.starts_with("#[inline]") {
                        inline = true;
                        break;
                    }
                    // Only attributes and comments may sit between a fn and
                    // its `#[inline]`; anything else means we left the block.
                    if !(t.starts_with("//") || t.starts_with("#[") || t.is_empty()) {
                        break;
                    }
                }
            }
            if !inline {
                bad.push(format!("{rel}:{}", i + 1));
            }
        }
    }

    assert!(
        bad.is_empty(),
        "these AllBuiltins delegates are missing `#[inline]`, so they emit a \
         standalone symbol that re-anchors the whole vocabulary:\n  {}",
        bad.join("\n  "),
    );
}

// ---------------------------------------------------------------------------
// Navigator services must ride the set, not the scheduler
// ---------------------------------------------------------------------------

/// Web boot files that must gate the URL provider behind `nav_services`.
const WEB_BOOT_FILES: &[&str] = &[
    "crates/backend/web/src/newcore.rs",
    "crates/backend/web/src/newcore_hydrate.rs",
];

/// Return the body of the first `nav_services(` call in `src`, brace-matched
/// from the `(` that opens it. `None` when the file makes no such call.
fn nav_services_body(src: &str) -> Option<&str> {
    let at = src.find("nav_services(")?;
    let open = at + "nav_services".len();
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&src[open + 1..i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Regression: a nav-less bundle still paid for the navigator.
///
/// `install_scheduler()` used to install the browser URL provider as a
/// convenience ("every web host calls it anyway"). That made it
/// unconditional, and its popstate listener calls `nav::handle_popstate` —
/// so `NavigatorControl::dispatch` plus the `Rc` drop glue for it stayed
/// reachable from boot in bundles that had dropped the navigator primitives
/// entirely. Measured on a `--primitives view,text` hello-world: 10,827
/// bytes of navigator code, 351,522 → 333,472 wasm / 124,659 → 118,850
/// brotli once the call moved behind the set.
///
/// The failure mode is invisible at the type level — everything compiles
/// either way and the app behaves identically — so this pins the shape:
/// the install may only appear inside the `nav_services` closure, whose body
/// a set without `nav` never runs and therefore never codegens.
#[test]
fn url_provider_install_is_gated_on_nav_services() {
    let root = repo_root();
    let mut bad: Vec<String> = Vec::new();

    // 1. The scheduler must not carry it. This is the exact regression.
    let sched_rel = "crates/backend/web/src/scheduler.rs";
    if let Ok(src) = std::fs::read_to_string(root.join(sched_rel)) {
        for (i, line) in src.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if t.contains("install_url_provider(") {
                bad.push(format!(
                    "{sched_rel}:{}: installs the URL provider from the \
                     scheduler, which is unconditional and re-anchors \
                     `NavigatorControl` in nav-less bundles — move it into \
                     the boot seam's `nav_services` closure",
                    i + 1
                ));
            }
        }
    }

    // 2. Both web boot paths must install it, and only inside the closure.
    for rel in WEB_BOOT_FILES {
        let Ok(src) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        let src = without_test_modules(&src);
        let body = nav_services_body(&src);
        match body {
            None => bad.push(format!("{rel}: makes no `nav_services` call")),
            Some(body) => {
                if !body.contains("install_url_provider()") {
                    bad.push(format!(
                        "{rel}: `nav_services` closure does not install the URL \
                         provider — a hydrated navigator loses its deep-link \
                         seed and pushState/popstate mirroring"
                    ));
                }
            }
        }
        // Any occurrence outside the closure defeats the gate.
        let inside = body.map(|b| b.matches("install_url_provider(").count()).unwrap_or(0);
        let total = src.matches("install_url_provider(").count();
        if total > inside {
            bad.push(format!(
                "{rel}: {} ungated `install_url_provider(` call(s) outside the \
                 `nav_services` closure",
                total - inside
            ));
        }
    }

    assert!(
        bad.is_empty(),
        "navigator URL services escaped the BuiltinSet gate:\n  {}",
        bad.join("\n  "),
    );
}

// ---------------------------------------------------------------------------
// Environment services must ride every boot entry
// ---------------------------------------------------------------------------

/// Regression: five author-facing reads were dead on every backend.
///
/// `platform()`, `color_scheme()`, `open_url()`, `set_fullscreen()` and
/// `announce()` read thread-local slots so author code can call them
/// without a backend reference. The pre-v2 `mount()` filled those slots
/// from the backend; `mount()` was deleted with the walker and nothing
/// replaced it. Every backend still *implemented* `AppEnvOps` and
/// `A11yOps::announce_for_accessibility`, so the caps-conformance suite
/// stayed green — but no boot entry forwarded them, so on every shipped
/// backend `platform()` returned `Custom("")` and the three routed
/// services were silent no-ops.
///
/// That is a hole in *absent* code, which no unit test can see: the
/// behavior tests in `backend_env_seam.rs` pass against a backend that
/// calls `install_env_services`, and say nothing about one that forgets.
/// `backend_macos::newcore` even documented the gap in a comment instead
/// of a test, which is exactly why it survived the migration.
///
/// So this scans the source. A new backend that copies an old one's boot
/// fn now fails here rather than shipping five quietly broken APIs.
#[test]
fn every_backend_boot_installs_env_services() {
    let root = repo_root();
    let mut missing: Vec<String> = Vec::new();

    for rel in BOOT_FILES {
        let path = root.join(rel);
        // Same policy as the scan above: the list is the contract, not
        // the filesystem — a backend that isn't checked out is skipped.
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let src = without_test_modules(&src);

        let installs = src
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with("//") && t.contains("install_env_services(")
            })
            .count();

        if installs == 0 {
            missing.push(format!(
                "{rel}: never calls `install_env_services` — `platform()`, \
                 `color_scheme()`, `open_url()`, `set_fullscreen()` and \
                 `announce()` are all dead on this backend"
            ));
            continue;
        }

        // Every registry-booting entry needs it, not just the first one.
        // `render_wgpu::newcore` has two (`start_with` + `start_in_world_with`)
        // and `backend_web` splits start/hydrate across two files — a boot
        // entry that builds a registry without the install is a live app
        // path with dead env services.
        let boots = src.matches("Registry::new()").count();
        if installs < boots {
            missing.push(format!(
                "{rel}: {boots} registry-booting entr{} but only {installs} \
                 `install_env_services` call(s) — every boot entry needs one",
                if boots == 1 { "y" } else { "ies" },
            ));
        }
    }

    assert!(
        missing.is_empty(),
        "backend boot entries missing the environment seam:\n  {}",
        missing.join("\n  "),
    );
}

/// The install must precede the root build.
///
/// A component body may read `platform()` while constructing — theme
/// selection off `color_scheme()` at the app root is the common case — so
/// seeding the slots after `realize` is too late and produces a
/// first-paint that branched on `Custom("")`. `Registry::new()` is the
/// stable landmark for "boot preamble is over"; requiring the install
/// above it keeps ordering enforceable without parsing the fn body.
#[test]
fn env_services_install_before_the_registry_and_build() {
    let root = repo_root();
    let mut bad: Vec<String> = Vec::new();

    for rel in BOOT_FILES {
        let Ok(src) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        let src = without_test_modules(&src);

        // Pair each install with the registry construction that follows
        // it: walking forward, an install must be seen before the next
        // `Registry::new()`.
        let mut pending_install = false;
        for (i, line) in src.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if t.contains("install_env_services(") {
                pending_install = true;
            }
            if t.contains("Registry::new()") {
                if !pending_install {
                    bad.push(format!(
                        "{rel}:{}: builds a `Registry` with no preceding \
                         `install_env_services` — a component body that reads \
                         `platform()` / `color_scheme()` during the root build \
                         would see the uninstalled default",
                        i + 1
                    ));
                }
                pending_install = false;
            }
        }
    }

    assert!(
        bad.is_empty(),
        "environment seam installed too late:\n  {}",
        bad.join("\n  "),
    );
}
