//! Every target — user-facing list of platforms idealyst runs on.

use runtime_core::{ui, Primitive};
use idea_ui::{stack, typography, StackGap, TypographyKind, TypographyTone};

use crate::pages::common::{page_header, page_section};
use crate::routes::BACKENDS_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Primitive {
    const PHONES: &str = "phones";
    const DESKTOPS: &str = "desktops";
    const BROWSER: &str = "browsers";
    const NATIVE_GPU: &str = "native-gpu";
    const EMBEDDED: &str = "embedded";
    const TTY: &str = "terminal";
    const TV: &str = "television";
    const EXTENDING: &str = "extending";
    const STATUS: &str = "implementation-status";

    let toc = vec![
        TocEntry { id: PHONES, label: "Phones" },
        TocEntry { id: DESKTOPS, label: "Desktops" },
        TocEntry { id: BROWSER, label: "Browsers" },
        TocEntry { id: NATIVE_GPU, label: "Native GPU rendering" },
        TocEntry { id: EMBEDDED, label: "Embedded & custom" },
        TocEntry { id: TTY, label: "Terminal" },
        TocEntry { id: TV, label: "Television" },
        TocEntry { id: EXTENDING, label: "Adding your own target" },
        TocEntry { id: STATUS, label: "Implementation status" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Every target",
                "The full list of platforms idealyst runs on, plus the path to teach \
                 it about a new one. If you can drive it from code, you can ship to it."
            ) }
            { page_section(PHONES, vec![phones()]) }
            { page_section(DESKTOPS, vec![desktops()]) }
            { page_section(BROWSER, vec![browser()]) }
            { page_section(NATIVE_GPU, vec![native_gpu()]) }
            { page_section(EMBEDDED, vec![embedded()]) }
            { page_section(TTY, vec![tty()]) }
            { page_section(TV, vec![tv()]) }
            { page_section(EXTENDING, vec![extending()]) }
            { page_section(STATUS, vec![status_link()]) }
        }
    };
    layout_with_toc(content, toc)
}

fn target_row(title: &str, blurb: &str) -> Primitive {
    let title_text = title.to_string();
    let blurb_text = blurb.to_string();
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = title_text, kind = TypographyKind::H3) },
        ui! { Typography(content = blurb_text, tone = TypographyTone::Muted) },
    ];
    ui! { Stack(gap = StackGap::Xs) { children } }
}

fn phones() -> Primitive {
    let rows: Vec<Primitive> = vec![
        target_row(
            "iOS",
            "UIKit driven via objc2. Native UIView hierarchy, native back gestures, \
             native scroll physics. iOS 13+. The standard pattern for shipping an idealyst \
             app to the App Store.",
        ),
        target_row(
            "Android",
            "Android Views over JNI. Native View hierarchy, native FragmentManager, \
             system back-button handling. API 24+. Same shape as iOS, different \
             toolchain. Distributable via Play Store / sideload / closed beta.",
        ),
    ];
    let mut children: Vec<Primitive> = vec![
        ui! { Typography(content = "Phones".to_string(), kind = TypographyKind::H2) },
    ];
    children.extend(rows);
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn desktops() -> Primitive {
    let rows: Vec<Primitive> = vec![
        target_row(
            "macOS",
            "AppKit via objc2. Native NSWindow + NSView hierarchy. Today: window shell, \
             buttons, text, scroll. Animation + media still being filled in.",
        ),
        target_row(
            "Windows (in progress)",
            "Win32 host. Evaluation in progress \u{2014} the goal is the same \"native \
             widgets driven by the framework\" shape that iOS and Android already have.",
        ),
        target_row(
            "Linux (in progress)",
            "GTK host. Same goals as the Windows target. Both desktop targets share \
             the wgpu render path as a fallback for surfaces the native toolkit can't \
             reach.",
        ),
    ];
    let mut children: Vec<Primitive> = vec![
        ui! { Typography(content = "Desktops".to_string(), kind = TypographyKind::H2) },
    ];
    children.extend(rows);
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn browser() -> Primitive {
    let rows: Vec<Primitive> = vec![
        target_row(
            "Web (WASM + DOM)",
            "Reference backend, most complete primitive coverage. Compiles to a WASM \
             bundle (typically a few hundred KB gzipped) and mounts into a div. No \
             JavaScript framework dependency \u{2014} the app is the wasm.",
        ),
    ];
    let mut children: Vec<Primitive> = vec![
        ui! { Typography(content = "Browsers".to_string(), kind = TypographyKind::H2) },
    ];
    children.extend(rows);
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn native_gpu() -> Primitive {
    let rows: Vec<Primitive> = vec![
        target_row(
            "wgpu renderer",
            "A second-implementation backend that drives the framework over wgpu. \
             Same Backend trait, but rendering goes through a GPU pipeline instead of \
             a native toolkit. Useful when you want pixel-perfect control of the \
             render output (custom widgets, novel visual styles, embedded devices \
             with a GPU).",
        ),
        target_row(
            "Phone / tablet / TV skins",
            "Pre-wired wgpu hosts that ship with iOS-sim and Android-sim skins so the \
             wgpu output visually matches the native toolkit it would normally run \
             against. Useful for development, screenshots, simulator-style demos.",
        ),
    ];
    let mut children: Vec<Primitive> = vec![
        ui! { Typography(content = "Native GPU rendering".to_string(), kind = TypographyKind::H2) },
    ];
    children.extend(rows);
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn embedded() -> Primitive {
    let rows: Vec<Primitive> = vec![
        target_row(
            "Microcontrollers (planned)",
            "A CPU-based graphics backend targeting `embedded-graphics`-compatible \
             devices: ESP32, Arduino with an LCD shield, Raspberry Pi Pico, etc. The \
             same `app()` function compiles into a `no_std`-friendly binary that drives \
             a tiny display.",
        ),
        target_row(
            "Custom rendering",
            "If your target is none of the above \u{2014} a pixel buffer you draw \
             yourself, a proprietary embedded surface, a server-side renderer \u{2014} \
             implement the Backend trait. There's no architectural assumption that the \
             target has a windowing system or a GPU or anything else.",
        ),
    ];
    let mut children: Vec<Primitive> = vec![
        ui! { Typography(content = "Embedded & custom".to_string(), kind = TypographyKind::H2) },
    ];
    children.extend(rows);
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn tty() -> Primitive {
    let rows: Vec<Primitive> = vec![
        target_row(
            "Terminal (TTY)",
            "crossterm-backed text-cell renderer. The framework treats the terminal grid \
             as a backend like any other \u{2014} you write the same `ui!` tree and it \
             paints into the cell buffer. Useful for CLI tools that want richer UI than \
             a sequence of prompts.",
        ),
    ];
    let mut children: Vec<Primitive> = vec![
        ui! { Typography(content = "Terminal".to_string(), kind = TypographyKind::H2) },
    ];
    children.extend(rows);
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn tv() -> Primitive {
    let rows: Vec<Primitive> = vec![
        target_row(
            "Roku",
            "BrightScript / SceneGraph transpile. The framework's `ui!` tree is rewritten \
             into Roku's native scene format. Less polished than the mobile backends \u{2014} \
             theme switching is currently disabled pending a token-system refactor.",
        ),
        target_row(
            "iOS TV / Android TV",
            "Both iOS and Android have TV variant crates scaffolded. They share the \
             primitive layer with their phone counterparts; the diff is layout defaults \
             and the input model (focus + d-pad instead of touch). Phone targets are \
             the priority right now, TV is a known follow-up.",
        ),
    ];
    let mut children: Vec<Primitive> = vec![
        ui! { Typography(content = "Television".to_string(), kind = TypographyKind::H2) },
    ];
    children.extend(rows);
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn extending() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Adding your own target".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Adding a new target is implementing the Backend trait. \
                A trait. One file's worth of contract.".to_string())
        },
        ui! {
            Typography(content = "The trait surface is moderately large \u{2014} one \
                method per primitive (create / update / insert / remove), plus a handful \
                of cross-cutting hooks (style apply, animated values, layout, refs). But \
                it's a fixed surface; the framework doesn't ask the backend to know about \
                routing, theming, components, or any higher-level concept. Get the \
                primitive surface right and everything else just works.".to_string(),
                tone = TypographyTone::Muted)
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn status_link() -> Primitive {
    let title = ui! {
        Typography(content = "Implementation status".to_string(), kind = TypographyKind::H2)
    };
    let para = ui! {
        Typography(content = "Per-backend implementation status \u{2014} which primitives \
            work where, what's in progress, what's planned \u{2014} lives on the Backends \
            page.".to_string())
    };
    let cta = ui! {
        Link(route = &BACKENDS_ROUTE, params = ()) {
            Typography(content = "See the Backends matrix \u{2192}".to_string())
        }
    };
    let children: Vec<Primitive> = vec![title, para, cta];
    ui! { Stack(gap = StackGap::Md) { children } }
}
