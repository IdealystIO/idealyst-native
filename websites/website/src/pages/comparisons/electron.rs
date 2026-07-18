//! "Why Idealyst over Electron?" — the native-vs-Chromium-wrapped pitch.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Stack, StackGap, Typography};

use crate::pages::common::PageHeader;
use crate::routes::{COMPARISONS_ROUTE, TARGETS_ROUTE};
use crate::shell::layout;

pub fn page() -> Element {
    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Why Idealyst over Electron?",
                blurb = "Electron made desktop cross-platform tractable for a generation \
                 of web developers, and that's a real contribution. The trade-off it asks \
                 for is the one Idealyst is built to avoid: every Electron app ships a full \
                 Chromium browser around your code.",
            )
            real_native_software()
            no_bundled_runtime()
            platform_feel()
            footer_links()
        }
    };
    layout(content)
}

fn real_native_software() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Real native software".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "An Idealyst app on macOS is an AppKit app. On Windows it's a \
                 Win32 host (in progress). On Linux it's a GTK host (in progress). There's \
                 no Chromium between your UI and the platform's window server — buttons are \
                 the platform's buttons, scroll views scroll the way the OS scrolls, the \
                 menu bar is a real menu bar.".to_string(),
            )
            Typography(
                content = "Where a target has no native toolkit to drive — embedded \
                 surfaces, custom GPU pipelines — the framework's wgpu backend renders the \
                 same primitives itself. Same author tree, different bottom layer.".to_string(),
                muted = true,
            )
        }
    }
}

fn no_bundled_runtime() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "No bundled browser runtime".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Electron's resting baseline is roughly the weight of Chromium \
                 plus Node — every app ships its own copy. That's the cost of giving each \
                 app a sealed browser environment, and it adds up across install size, \
                 memory footprint, and update bandwidth.".to_string(),
            )
            Typography(
                content = "Idealyst's desktop builds are a native binary linking the \
                 framework directly. No embedded engine, no per-app runtime to ship or keep \
                 patched against browser CVEs.".to_string(),
            )
        }
    }
}

fn platform_feel() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Reads as belonging to the device".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Native accessibility focus, native text selection, native \
                 keyboard handling, native drag-and-drop — these come from the platform's \
                 own widgets, not approximations layered over a webview. Users don't have \
                 to consciously notice it, but they feel it when something behaves the way \
                 the rest of their OS does.".to_string(),
            )
        }
    }
}

fn footer_links() -> Element {
    ui! {
        Stack(gap = StackGap::Sm) {
            link(route = &TARGETS_ROUTE, params = ()) {
                Typography(content = "See every target Idealyst runs on \u{2192}".to_string())
            }
            link(route = &COMPARISONS_ROUTE, params = ()) {
                Typography(content = "Back to all comparisons \u{2192}".to_string())
            }
        }
    }
}
