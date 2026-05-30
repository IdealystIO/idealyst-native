//! Every target — user-facing list of platforms idealyst runs on.

use runtime_core::{component, ui, Element, Ref, ViewHandle};
use idea_ui::{typography_kind, Stack, StackGap, Typography};

use crate::pages::common::{PageHeader, PageSection};
use crate::routes::BACKENDS_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let phones_ref: Ref<ViewHandle> = Ref::new();
    let desktops_ref: Ref<ViewHandle> = Ref::new();
    let browser_ref: Ref<ViewHandle> = Ref::new();
    let native_gpu_ref: Ref<ViewHandle> = Ref::new();
    let embedded_ref: Ref<ViewHandle> = Ref::new();
    let tty_ref: Ref<ViewHandle> = Ref::new();
    let tv_ref: Ref<ViewHandle> = Ref::new();
    let extending_ref: Ref<ViewHandle> = Ref::new();
    let status_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: phones_ref, label: "Phones" },
        TocEntry { handle: desktops_ref, label: "Desktops" },
        TocEntry { handle: browser_ref, label: "Browsers" },
        TocEntry { handle: native_gpu_ref, label: "Native GPU rendering" },
        TocEntry { handle: embedded_ref, label: "Embedded & custom" },
        TocEntry { handle: tty_ref, label: "Terminal" },
        TocEntry { handle: tv_ref, label: "Television" },
        TocEntry { handle: extending_ref, label: "Adding your own target" },
        TocEntry { handle: status_ref, label: "Implementation status" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Every target",
                blurb = "The full list of platforms idealyst runs on, plus the path to teach \
                 it about a new one. If you can drive it from code, you can ship to it.",
            )
            PageSection(handle = phones_ref) { phones() }
            PageSection(handle = desktops_ref) { desktops() }
            PageSection(handle = browser_ref) { browser() }
            PageSection(handle = native_gpu_ref) { native_gpu() }
            PageSection(handle = embedded_ref) { embedded() }
            PageSection(handle = tty_ref) { tty() }
            PageSection(handle = tv_ref) { tv() }
            PageSection(handle = extending_ref) { extending() }
            PageSection(handle = status_ref) { status_link() }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// TargetRow — one entry on a target section's list. Promoted from the
// snake_case `target_row` helper because it has props and is called
// many times across the page (CLAUDE.md §9.5).
// =============================================================================

#[derive(Default)]
pub struct TargetRowProps {
    pub title: String,
    pub blurb: String,
}

#[component]
pub fn TargetRow(props: TargetRowProps) -> Element {
    let title = props.title;
    let blurb = props.blurb;
    ui! {
        Stack(gap = StackGap::Xs) {
            Typography(content = title, kind = typography_kind::H3)
            Typography(content = blurb, muted = true)
        }
    }
}

// =============================================================================
// Per-target sections — no-param file-local helpers (allowed per §9.5).
// Each invokes `TargetRow` for its individual entries inside a `Stack`.
// =============================================================================

fn phones() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Phones".to_string(), kind = typography_kind::H2)
            TargetRow(
                title = "iOS".to_string(),
                blurb = "UIKit driven via objc2. Native UIView hierarchy, native back gestures, \
                 native scroll physics. iOS 13+. The standard pattern for shipping an idealyst \
                 app to the App Store.".to_string(),
            )
            TargetRow(
                title = "Android".to_string(),
                blurb = "Android Views over JNI. Native View hierarchy, native FragmentManager, \
                 system back-button handling. API 24+. Same shape as iOS, different \
                 toolchain. Distributable via Play Store / sideload / closed beta.".to_string(),
            )
        }
    }
}

fn desktops() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Desktops".to_string(), kind = typography_kind::H2)
            TargetRow(
                title = "macOS".to_string(),
                blurb = "AppKit via objc2. Native NSWindow + NSView hierarchy. Today: window shell, \
                 buttons, text, scroll. Animation + media still being filled in.".to_string(),
            )
            TargetRow(
                title = "Windows (in progress)".to_string(),
                blurb = "Win32 host. Evaluation in progress \u{2014} the goal is the same \"native \
                 widgets driven by the framework\" shape that iOS and Android already have.".to_string(),
            )
            TargetRow(
                title = "Linux (in progress)".to_string(),
                blurb = "GTK host. Same goals as the Windows target. Both desktop targets share \
                 the wgpu render path as a fallback for surfaces the native toolkit can't \
                 reach.".to_string(),
            )
        }
    }
}

fn browser() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Browsers".to_string(), kind = typography_kind::H2)
            TargetRow(
                title = "Web (WASM + DOM)".to_string(),
                blurb = "Reference backend, most complete primitive coverage. Compiles to a WASM \
                 bundle (typically a few hundred KB gzipped) and mounts into a div. No \
                 JavaScript framework dependency \u{2014} the app is the wasm.".to_string(),
            )
        }
    }
}

fn native_gpu() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Native GPU rendering".to_string(), kind = typography_kind::H2)
            TargetRow(
                title = "wgpu renderer".to_string(),
                blurb = "A second-implementation backend that drives the framework over wgpu. \
                 Same Backend trait, but rendering goes through a GPU pipeline instead of \
                 a native toolkit. Useful when you want pixel-perfect control of the \
                 render output (custom widgets, novel visual styles, embedded devices \
                 with a GPU).".to_string(),
            )
            TargetRow(
                title = "Phone / tablet / TV skins".to_string(),
                blurb = "Pre-wired wgpu hosts that ship with iOS-sim and Android-sim skins so the \
                 wgpu output visually matches the native toolkit it would normally run \
                 against. Useful for development, screenshots, simulator-style demos.".to_string(),
            )
        }
    }
}

fn embedded() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Embedded & custom".to_string(), kind = typography_kind::H2)
            TargetRow(
                title = "Microcontrollers (planned)".to_string(),
                blurb = "A CPU-based graphics backend targeting `embedded-graphics`-compatible \
                 devices: ESP32, Arduino with an LCD shield, Raspberry Pi Pico, etc. The \
                 same `app()` function compiles into a `no_std`-friendly binary that drives \
                 a tiny display.".to_string(),
            )
            TargetRow(
                title = "Custom rendering".to_string(),
                blurb = "If your target is none of the above \u{2014} a pixel buffer you draw \
                 yourself, a proprietary embedded surface, a server-side renderer \u{2014} \
                 implement the Backend trait. There's no architectural assumption that the \
                 target has a windowing system or a GPU or anything else.".to_string(),
            )
        }
    }
}

fn tty() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Terminal".to_string(), kind = typography_kind::H2)
            TargetRow(
                title = "Terminal (TTY)".to_string(),
                blurb = "crossterm-backed text-cell renderer. The framework treats the terminal grid \
                 as a backend like any other \u{2014} you write the same `ui!` tree and it \
                 paints into the cell buffer. Useful for CLI tools that want richer UI than \
                 a sequence of prompts.".to_string(),
            )
        }
    }
}

fn tv() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Television".to_string(), kind = typography_kind::H2)
            TargetRow(
                title = "Roku".to_string(),
                blurb = "BrightScript / SceneGraph transpile. The framework's `ui!` tree is rewritten \
                 into Roku's native scene format. Less polished than the mobile backends \u{2014} \
                 theme switching is currently disabled pending a token-system refactor.".to_string(),
            )
            TargetRow(
                title = "iOS TV / Android TV".to_string(),
                blurb = "Both iOS and Android have TV variant crates scaffolded. They share the \
                 primitive layer with their phone counterparts; the diff is layout defaults \
                 and the input model (focus + d-pad instead of touch). Phone targets are \
                 the priority right now, TV is a known follow-up.".to_string(),
            )
        }
    }
}

fn extending() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Adding your own target".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Adding a new target is implementing the Backend trait. \
                    A trait. One file's worth of contract.".to_string(),
            )
            Typography(
                content = "The trait surface is moderately large \u{2014} one \
                    method per primitive (create / update / insert / remove), plus a handful \
                    of cross-cutting hooks (style apply, animated values, layout, refs). But \
                    it's a fixed surface; the framework doesn't ask the backend to know about \
                    routing, theming, components, or any higher-level concept. Get the \
                    primitive surface right and everything else just works.".to_string(),
                muted = true,
            )
        }
    }
}

fn status_link() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Implementation status".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Per-backend implementation status \u{2014} which primitives \
                    work where, what's in progress, what's planned \u{2014} lives on the Backends \
                    page.".to_string(),
            )
            link(route = &BACKENDS_ROUTE, params = ()) {
                Typography(content = "See the Backends matrix \u{2192}".to_string())
            }
        }
    }
}
