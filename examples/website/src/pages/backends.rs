//! Backends — implementation status per platform.

use runtime_core::{ui, Primitive};
use idea_ui::{stack, typography, StackGap, TypographyKind, TypographyTone};

use crate::pages::common::page_header;
use crate::shell::layout;
use crate::styles::PagePad;

pub fn page() -> Primitive {
    let pad = PagePad();
    let content = ui! {
        View(style = pad) {
            Stack(gap = StackGap::Xl) {
                { page_header(
                    "Backends",
                    "What's implemented per target, what's in progress, what's planned. \
                     The Backend trait is the framework's only seam to the platform; \
                     this page is the per-platform status of that seam."
                ) }
                { matrix() }
                { coverage() }
                { roadmap() }
            }
        }
    };
    layout(content)
}

fn matrix() -> Primitive {
    let rows: Vec<(&str, &str, &str)> = vec![
        ("Web (WASM + DOM)", "Working", "Reference backend. Most complete primitive coverage."),
        ("Android (JNI + Views)", "Working", "Phone form factor. TV variant is a stub."),
        ("iOS (UIKit via objc2)", "Working", "Phone form factor. TV variant is a stub. `Video` and `Virtualizer` still missing."),
        ("macOS (AppKit via objc2)", "Early", "Window shell + basics. Many primitives unimplemented."),
        ("Roku (BrightScript transpile)", "Working", "Theme switching temporarily disabled (token refactor); panics on theme update."),
        ("Native GPU (wgpu)", "In progress", "Implements Backend over a GPU pipeline. `host-winit`/`host-web`/`host-appkit` wire it to OS windows."),
        ("CPU graphics (Arduino, ESP32)", "Planned", "Software rasterizer driving an `embedded-graphics` surface."),
        ("Windows", "In progress", "Win32 surface, evaluation in progress."),
        ("Linux", "In progress", "GTK surface, evaluation in progress."),
        ("Terminal (TTY)", "Working", "crossterm-backed; no animations or auto-rendered header chrome (per terminal-minimalism convention)."),
    ];

    let mut row_children: Vec<Primitive> = Vec::with_capacity(rows.len() * 3);
    for (name, status, notes) in rows {
        let title = format!("{} \u{00b7} {}", name, status);
        row_children.push(ui! {
            Typography(content = title, kind = TypographyKind::H3)
        });
        row_children.push(ui! {
            Typography(content = notes.to_string(), tone = TypographyTone::Muted)
        });
    }
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Backend status".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "\"Working\" means the backend implements the Backend \
                trait and runs the existing example apps. It doesn't mean every primitive \
                works (see the coverage matrix below).".to_string())
        },
    ];
    let mut all: Vec<Primitive> = Vec::with_capacity(children.len() + row_children.len());
    all.extend(children);
    all.extend(row_children);
    ui! { Stack(gap = StackGap::Md) { all } }
}

fn coverage() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Per-primitive coverage".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "A blank in the per-primitive coverage matrix means the \
                trait default panics with `unimplemented!()`. Author code that reaches \
                for that primitive on that backend will crash at mount, not silently \
                no-op.".to_string())
        },
        ui! { Typography(content = "Core primitives".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "View, Text, Button: every backend.".to_string())
        },
        ui! { Typography(content = "Inputs".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "Image, TextInput, ScrollView, Slider, Toggle, Icon, \
                ActivityIndicator, Graphics: every backend except macOS.".to_string())
        },
        ui! { Typography(content = "Lists + media".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "Virtualizer, Video: web + Android. iOS is catching up.".to_string())
        },
        ui! { Typography(content = "External SDKs".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "`Primitive::External` (Maps, WebView, idea-codeblock): \
                web + iOS + Android. macOS and Roku not yet wired.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn roadmap() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Framework-level subsystems".to_string(), kind = TypographyKind::H2) },
        ui! { Typography(content = "Working".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "Primitives + reactivity + render walker; `ui!` / `jsx!` / \
                `#[component]` / `stylesheet!`; reactive `if` / `when` / `for` in the DSLs; \
                refs via `Ref<H>`; idea-ui component library; icon registry; Robot + MCP \
                introspection; hot-reload dev server with runtime-server shell; \
                server-driven UI over the wire protocol.".to_string(),
                tone = TypographyTone::Muted)
        },
        ui! { Typography(content = "In progress".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "Custom rendering via wgpu (skins for phone, tablet, \
                tv); native backend interactions / media / OS integration.".to_string(),
                tone = TypographyTone::Muted)
        },
        ui! { Typography(content = "Planned".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "Async data + `Resource<T>`; first-class accessibility \
                across every primitive; SSR + hydration.".to_string(),
                tone = TypographyTone::Muted)
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}
