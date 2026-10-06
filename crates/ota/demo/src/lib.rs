//! The over-the-air demo.
//!
//! This file compiles twice: as the app (the window), and as the bundle the
//! app downloads (`idealyst ota publish` builds this crate's library for
//! wasm). The top half of the window is native: what the updater is doing,
//! read live from `ota`. The bottom half is [`Panel`], a remote component:
//! it comes from the newest release in the bucket, re-checked every few
//! seconds and swapped in as soon as it's downloaded.

#![cfg_attr(idealyst_stream_guest, allow(dead_code, unused_imports))]
#![cfg_attr(not(idealyst_stream_guest), allow(dead_code, unused_imports))]

use std::rc::Rc;

use idea_ui::{tone, typography_kind, Badge, Button, Card, Typography};
use runtime_core::{component, rx, signal, ui, Element};
use runtime_shared::{Color, FontWeight, Length, StyleRules, Tokenized};

// ---------------------------------------------------------------------------
// The over-the-air part. Edit it, then ./publish.sh.
// ---------------------------------------------------------------------------

/// The bottom half of the window: downloaded, never compiled into the app.
#[component(remote)]
pub fn Panel() -> Element {
    let taps = signal(0u32);
    ui! {
        Card() {
            Typography(content = "This is a hot update".to_string(), kind = typography_kind::H3)
            Typography(content = "Change this text in crates/ota/demo/src/lib.rs, run ./publish.sh, and watch it arrive.".to_string())
            Badge(label = "edit me".to_string(), tone = tone::Info)
            Button(
                label = rx!(format!("Tapped {} times", taps.get())),
                on_click = Rc::new(move || taps.update(|n| n + 1)) as Rc<dyn Fn()>,
                tone = tone::Primary
            )
        }
    }
}

// ---------------------------------------------------------------------------
// The app: native.
// ---------------------------------------------------------------------------

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

fn color(hex: &str) -> Option<Tokenized<Color>> {
    Some(Tokenized::Literal(Color(hex.into())))
}

fn column(gap: f32) -> StyleRules {
    StyleRules {
        flex_direction: Some(runtime_shared::FlexDirection::Column),
        gap: px(gap),
        ..StyleRules::default()
    }
}

#[cfg(not(idealyst_stream_guest))]
pub use app::{app, app_with};

/// What the bundle may call: idea-ui's global functions. Declared in
/// Cargo.toml (`host_fns = "host_fns"`), so `ota::start` and the manifest
/// `idealyst ota manifest` registers use this same list.
#[cfg(not(idealyst_stream_guest))]
pub fn host_fns() -> Vec<runtime_vocabulary::remote::HostFnDef> {
    idea_ui::host_fns()
}

#[cfg(not(idealyst_stream_guest))]
mod app {
    use std::cell::RefCell;
    use std::time::Duration;

    use ota::{Activity, BundleState, Ota, Source, Status};
    use runtime_world::Signal;

    use super::*;

    /// How often the app looks for a new release.
    const CHECK_EVERY: Duration = Duration::from_secs(5);

    thread_local! {
        static OTA: RefCell<Option<Ota>> = const { RefCell::new(None) };
        /// The time, ticking each second, for "next check in …".
        static CLOCK: RefCell<Option<Signal<u64>>> = const { RefCell::new(None) };
    }

    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }

    fn tick(clock: Signal<u64>) {
        clock.set(now());
        runtime_shared::scheduling::after_ms_detached(1000, move || tick(clock));
    }

    /// The window's root: starts the updater (once), then the app.
    pub fn app() -> Element {
        app_with(None)
    }

    /// [`app`], keeping downloads in `cache_dir` (the app's private files by
    /// default). Tests.
    #[doc(hidden)]
    pub fn app_with(cache_dir: Option<std::path::PathBuf>) -> Element {
        if OTA.with(|o| o.borrow().is_none()) {
            idea_ui::install_idea_theme_schemes(idea_ui::light_theme(), idea_ui::dark_theme());
            let ota = ota::start(
                ota::config!(),
                ota::Options {
                    // The host functions come from Cargo.toml (`host_fns`).
                    // Swap a new release in at once (the default waits for
                    // the next launch, so nothing changes under the user).
                    apply: ota::Apply::Now,
                    check_every: Some(CHECK_EVERY),
                    cache_dir,
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("ota: {e}"));
            OTA.with(|o| *o.borrow_mut() = Some(ota));
            let clock = runtime_world::unscoped(|| runtime_world::signal(now()));
            CLOCK.with(|c| *c.borrow_mut() = Some(clock));
            tick(clock);
        }
        ui! { App() }
    }

    fn ota() -> Ota {
        OTA.with(|o| o.borrow().clone()).expect("started in app()")
    }

    #[component]
    fn App() -> Element {
        ui! {
            scroll_view(style = StyleRules {
                background: Some(Tokenized::token("color-background", Color("#f6f7f9".into()))),
                flex_grow: Some(Tokenized::Literal(1.0)),
                ..StyleRules::default()
            }) {
                view(style = StyleRules { padding_top: px(20.0), padding_bottom: px(20.0), padding_left: px(20.0), padding_right: px(20.0), ..column(16.0) }) {
                    Updater()
                    view(style = StyleRules { height: px(1.0), background: color("#d1d5db"), ..StyleRules::default() }) {}
                    Remote()
                }
            }
        }
    }

    /// The top half: everything the updater knows, live.
    #[component]
    fn Updater() -> Element {
        let ota = ota();
        let (status, bundles, checks) = (ota.status(), ota.bundles(), ota.checks());
        let clock = CLOCK.with(|c| c.borrow().expect("started in app()"));
        let check = ota.clone();
        let manifest = short(ota.manifest_id()).to_string();
        ui! {
            view(style = StyleRules {
                background: color("#111827"),
                border_top_left_radius: px(12.0),
                border_top_right_radius: px(12.0),
                border_bottom_left_radius: px(12.0),
                border_bottom_right_radius: px(12.0),
                padding_top: px(16.0), padding_bottom: px(16.0), padding_left: px(16.0), padding_right: px(16.0),
                ..column(8.0)
            }) {
                text(style = StyleRules { color: color("#9ca3af"), font_size: px(12.0), ..StyleRules::default() }) { "NATIVE · the updater" }
                text(style = StyleRules { color: color("#f9fafb"), font_size: px(18.0), font_weight: Some(FontWeight::Bold), ..StyleRules::default() }) {
                    move || status_line(&status.get())
                }
                text(style = StyleRules { color: color("#e5e7eb"), font_size: px(14.0), ..StyleRules::default() }) {
                    move || {
                        let all = bundles.get();
                        if all.is_empty() {
                            "no bundles yet — waiting for the index".to_string()
                        } else {
                            all.iter().map(bundle_lines).collect::<Vec<_>>().join("\n\n")
                        }
                    }
                }
                text(style = StyleRules { color: color("#9ca3af"), font_size: px(13.0), ..StyleRules::default() }) {
                    move || {
                        let (c, now) = (checks.get(), clock.get());
                        let last = c.last.map_or("never".to_string(), |t| format!("{}s ago", now.saturating_sub(t)));
                        let next = c.next.map_or(String::new(), |t| format!(" · next in {}s", t.saturating_sub(now)));
                        let by = match c.answered_by {
                            None => "",
                            Some(ota::AnsweredBy::Service) => " · answered by the service",
                            Some(ota::AnsweredBy::Precomputed) => " · answered by this build's precomputed file",
                            Some(ota::AnsweredBy::Index) => " · decided here from the index",
                        };
                        format!("last check {last}{next}{by}\nmanifest {manifest} · {}", ota::config!().url)
                    }
                }
                button(label = "Check now", on_click = move || check.check())
            }
        }
    }

    /// The bottom half: the remote component, or what's happening to it.
    #[component]
    fn Remote() -> Element {
        let bundles = ota().bundles();
        let running = move || bundles.get().iter().any(|b| b.name == "panel" && b.running.is_some());
        ui! {
            view(style = column(8.0)) {
                text(style = StyleRules { color: color("#6b7280"), font_size: px(12.0), ..StyleRules::default() }) { "REMOTE · from the bundle" }
                if !running() {
                    text(style = StyleRules { color: color("#6b7280"), ..StyleRules::default() }) { "Fetching the panel's bundle…" }
                }
                Panel()
            }
        }
    }

    fn status_line(s: &Status) -> String {
        match s {
            Status::Checking => "● Checking for updates…".into(),
            Status::UpToDate => "● Up to date".into(),
            Status::UpdateReady => "● Update downloaded — runs at the next launch".into(),
            Status::AppUpdateRequired => "● A newer release needs an app update".into(),
            Status::Failed(e) => format!("● Check failed: {e}"),
        }
    }

    fn short(sha: &str) -> &str {
        &sha[..sha.len().min(8)]
    }

    fn bundle_lines(b: &BundleState) -> String {
        let running = match &b.running {
            None => "not downloaded yet".to_string(),
            Some(r) => {
                let from = match r.from {
                    Source::BuiltIn => "built in",
                    Source::Cache => "from the cache",
                    Source::Download => "downloaded",
                };
                format!("running {} ({}, {from})", r.version.as_deref().unwrap_or("?"), short(&r.sha256))
            }
        };
        let latest = match &b.latest {
            None => "none this app can run".to_string(),
            Some(l) => format!("{} ({})", l.version, short(&l.sha256)),
        };
        let activity = match &b.activity {
            Activity::Idle => String::new(),
            Activity::Downloading(v) => format!("\n  ↓ downloading {v}…"),
            Activity::Ready(v) => format!("\n  ✓ {v} ready for the next launch"),
            Activity::Failed(e) => format!("\n  ✗ {e}"),
        };
        let needs_app = if b.newer_needs_app_update { "\n  ⚠ a newer release needs an app update" } else { "" };
        format!("bundle `{}`\n  {running}\n  latest: {latest}{activity}{needs_app}", b.name)
    }
}
