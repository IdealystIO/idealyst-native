//! Backends — implementation status per platform.

use runtime_core::{ui, Primitive};
use idea_ui::{stack, typography, StackGap, TypographyKind, TypographyTone};

use crate::pages::common::{page_header, page_section};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Primitive {
    const MATRIX: &str = "matrix";
    const COVERAGE: &str = "coverage";
    const ROADMAP: &str = "roadmap";

    let toc = vec![
        TocEntry { id: MATRIX, label: "Backend status" },
        TocEntry { id: COVERAGE, label: "Per-primitive coverage" },
        TocEntry { id: ROADMAP, label: "Framework-level subsystems" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Backends",
                "What's implemented per target, what's in progress, what's planned. \
                 The Backend trait is the framework's only seam to the platform; \
                 this page is the per-platform status of that seam."
            ) }
            { page_section(MATRIX, vec![matrix()]) }
            { page_section(COVERAGE, vec![coverage()]) }
            { page_section(ROADMAP, vec![roadmap()]) }
        }
    };
    layout_with_toc(content, toc)
}

fn matrix() -> Primitive {
    let rows: Vec<(&str, &str, &str)> = vec![
        ("Web (WASM + DOM)", "Working", "Reference backend. Most complete primitive coverage."),
        ("Android (JNI + Views)", "Working", "Phone form factor. TV variant is a stub."),
        ("iOS (UIKit via objc2)", "Working", "Phone form factor. TV variant is a stub. Virtualizer supports vertical + horizontal single-section lists (measured-cells + sections pending framework-core API)."),
        ("macOS (AppKit via objc2)", "Working", "All Backend primitives covered — no `unimplemented!()` panics. Real AppKit widgets for View, Text, Button, Image, TextInput, TextArea, Toggle, Slider, ActivityIndicator. Icon renders real vector paths via CAShapeLayer + shared `apple/core::icon_path` parser (same output as iOS). Portal mounts to host contentView. External registry mirrors iOS. Placeholder treatments (deferred to focused PRs): Virtualizer = eager-mount; Graphics + Navigator = visible red placeholder."),
        ("Roku (BrightScript transpile)", "Working", "Theme switching works via re-applied literal values; no runtime token layer on SceneGraph."),
        ("Native GPU (wgpu)", "In progress", "Implements Backend over a GPU pipeline. `host-winit` is production-ready; `host-web`/`host-appkit`/`host-terminal` are WIP. Engine covers 19/21 primitives — TextArea routes through TextInput chrome (multi-line wrap pending); Presence enter/exit transitions tween natively via the host tick loop; Navigator + External still panic."),
        ("CPU graphics (Arduino, ESP32)", "Working (MVP)", "Software rasterizer with custom `Surface` trait. Full support for View, Text, Button, Pressable, ScrollView (gradients, rounded corners, hit-testing). Inputs, lists, GPU, modals, navigators render a visible placeholder text per `feedback_cpu_unsupported_placeholders` — missing support is SEEN on device, not silently no-op'd."),
        ("Windows", "Scaffold", "Native Win32 backend via the `windows` crate. View/Text/Button/Pressable wire to real HWNDs (STATIC/BUTTON classes). Other primitives render visible placeholder labels. Compiles to empty rlib on non-Windows hosts; needs a Windows test env to exercise the actual widget rendering."),
        ("Linux", "Scaffold", "Native GTK4 backend via `gtk4-rs`. View/Text/Button/Pressable/Toggle/Slider/ActivityIndicator/TextInput/TextArea/ScrollView all wire to real GTK widgets. Other primitives render placeholder labels. Compiles to empty rlib on non-Linux hosts; needs a Linux test env with `libgtk-4-dev` to exercise."),
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
            Typography(content = "Image, TextInput, TextArea, Slider, Toggle, ActivityIndicator: \
                every backend including macOS. ScrollView on macOS uses a flat document view \
                (real NSScrollView wrap pending).".to_string())
        },
        ui! { Typography(content = "Lists".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "Virtualizer: web + Android + iOS (single-section, vertical \
                or horizontal). macOS not yet wired.".to_string())
        },
        ui! { Typography(content = "External SDKs".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(content = "`Primitive::External` registry is wired on web + iOS + Android \
                (macOS and Roku pending). Per-SDK leaves: Video (web/iOS/Android), \
                WebView (web full / iOS full / Android URL-only — Kotlin shim pending), \
                Maps (web/iOS via MKMapView — Android leaf pending), \
                idea-codeblock (web only).".to_string())
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
