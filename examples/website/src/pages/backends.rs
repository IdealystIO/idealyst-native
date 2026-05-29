//! Backends — implementation status per platform.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{PageHeader, PageSection};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let matrix_ref: Ref<ViewHandle> = Ref::new();
    let coverage_ref: Ref<ViewHandle> = Ref::new();
    let roadmap_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: matrix_ref, label: "Backend status" },
        TocEntry { handle: coverage_ref, label: "Per-primitive coverage" },
        TocEntry { handle: roadmap_ref, label: "Framework-level subsystems" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Backends",
                blurb = "What's implemented per target, what's in progress, what's planned. \
                 The Backend trait is the framework's only seam to the platform; \
                 this page is the per-platform status of that seam.",
            )
            PageSection(handle = matrix_ref) { matrix() }
            PageSection(handle = coverage_ref) { coverage() }
            PageSection(handle = roadmap_ref) { roadmap() }
        }
    };
    layout_with_toc(content, toc)
}

fn matrix() -> Element {
    let rows: Vec<(&str, &str, &str)> = vec![
        ("Web (WASM + DOM)", "Working", "Reference backend. Most complete primitive coverage."),
        ("Android (JNI + Views)", "Working", "Phone form factor. TV variant is a stub."),
        ("iOS (UIKit via objc2)", "Working", "Phone form factor. TV variant is a stub. Virtualizer supports vertical + horizontal single-section lists (measured-cells + sections pending framework-core API)."),
        ("macOS (AppKit via objc2)", "Working", "All Backend primitives covered — no `unimplemented!()` panics. Real AppKit widgets for View, Text, Button, Image, TextInput, TextArea, Toggle, Slider, ActivityIndicator. Icon renders real vector paths via CAShapeLayer + shared `apple/core::icon_path` parser. ScrollView wraps a real NSScrollView. Virtualizer wraps NSScrollView + NSCollectionView with real cell reuse. Graphics attaches a wgpu Surface to a CAMetalLayer-backed NSView. Portal mounts to host contentView. External + Navigator registries mirror iOS. All three navigator SDKs (`drawer_navigator`, `stack_navigator`, `tab_navigator`) ship macOS modules — single-window layouts per `project_macos_navigator_design`: drawer = persistent sidebar + outlet; stack = outlet-swap on push/pop (no animated chrome); tab = top/bottom tabbar + outlet."),
        ("Roku (BrightScript transpile)", "Working", "Theme switching works via re-applied literal values; no runtime token layer on SceneGraph."),
        ("Native GPU (wgpu)", "In progress", "Implements Backend over a GPU pipeline. `host-winit` is production-ready with optional AccessKit bridge (Windows UIA / Linux AT-SPI / macOS NSAccessibility via the `a11y` feature flag); `host-web` is the browser-canvas variant. All 21 primitives covered with no `unimplemented!()` panics. TextArea routes through TextInput chrome (multi-line wrap pending); Presence enter/exit transitions tween natively via the host tick loop. External + Navigator registries mirror iOS + macOS — SDK leaves register handlers via `register_external::<T, _>()` / `register_navigator::<P, _>()` on the wgpu engine; unregistered kinds get explicit 'not registered' placeholders. The overlay-per-host story for SDKs that need real native views (WebKit/Maps) remains a separate follow-up."),
        ("CPU graphics (Arduino, ESP32)", "Working (MVP)", "Software rasterizer with custom `Surface` trait. Full support for View, Text, Button, Pressable, ScrollView (gradients, rounded corners, hit-testing). Inputs, lists, GPU, modals, navigators render a visible placeholder text per `feedback_cpu_unsupported_placeholders` — missing support is SEEN on device, not silently no-op'd."),
        ("Windows", "Scaffold", "Native Win32 backend via the `windows` crate. View/Text/Button/Pressable wire to real HWNDs (STATIC/BUTTON classes). `finish()` drives Taffy layout into `SetWindowPos`. `WM_COMMAND` dispatch is wired: control-ids allocated per Button/Pressable, host's WndProc forwards `LOWORD(wParam)` through `WindowsBackend::dispatch_command`. Other primitives render placeholder labels. Compiles to empty rlib on non-Windows hosts."),
        ("Linux", "Scaffold", "Native GTK4 backend via `gtk4-rs`. Containers (View/Pressable/ScrollView) use `gtk::Fixed` for absolute positioning. View/Text/Button/Pressable/Toggle/Slider/ActivityIndicator/TextInput/TextArea/ScrollView all wire to real GTK widgets. `finish()` drives Taffy into `fixed.move_()` + `set_size_request()`. Other primitives render placeholder labels. Compiles to empty rlib on non-Linux hosts."),
        ("Terminal (TTY)", "Working", "crossterm-backed; no animations or auto-rendered header chrome (per terminal-minimalism convention)."),
    ];

    let mut row_children: Vec<Element> = Vec::with_capacity(rows.len() * 3);
    for (name, status, notes) in rows {
        let title = format!("{} \u{00b7} {}", name, status);
        row_children.push(ui! {
            Typography(content = title, kind = idea_ui::typography_kind::H3)
        });
        row_children.push(ui! {
            Typography(content = notes.to_string(), muted = true)
        });
    }
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Backend status".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "\"Working\" means the backend implements the Backend \
                trait and runs the existing example apps. It doesn't mean every primitive \
                works (see the coverage matrix below).".to_string())
        },
    ];
    let mut all: Vec<Element> = Vec::with_capacity(children.len() + row_children.len());
    all.extend(children);
    all.extend(row_children);
    ui! { Stack(gap = StackGap::Md) { all } }
}

fn coverage() -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Per-primitive coverage".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(content = "Every backend now implements every Backend method — no \
                `unimplemented!()` panics anywhere. Primitives that aren't yet fully \
                rendered on a given backend render a visible placeholder (per \
                `feedback_cpu_unsupported_placeholders`); the dev / user sees the gap \
                instead of hitting a silent crash or empty rect.".to_string())
        },
        ui! { Typography(content = "Core primitives".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "View, Text, Button, Pressable: every backend.".to_string())
        },
        ui! { Typography(content = "Inputs + media".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "Image, TextInput, TextArea, Slider, Toggle, ActivityIndicator, \
                Icon: web / iOS / Android / macOS / wgpu all render real widgets. Icon shares \
                its SVG parser across iOS + macOS via `apple/core::icon_path`. CPU + Win32 + \
                Linux scaffolds use placeholder text for the inputs they don't yet wire to \
                native widgets.".to_string())
        },
        ui! { Typography(content = "Lists".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "Virtualizer: web + Android + iOS + macOS (single-section, \
                vertical or horizontal; macOS via NSCollectionView with real cell reuse). \
                wgpu mounts every item. CPU / Win32 / Linux render placeholder text.".to_string())
        },
        ui! { Typography(content = "External SDKs".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "`Element::External` registry is wired on web + iOS + \
                Android + macOS. Per-SDK leaves: Video (web/iOS/Android), \
                WebView (web full / iOS full / Android URL-only — Kotlin shim pending), \
                Maps (web/iOS via MKMapView — Android leaf pending), \
                idea-codeblock (web only). wgpu / CPU / Win32 / Linux render placeholders \
                for any unregistered or unsupported external kind.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn roadmap() -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Framework-level subsystems".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! { Typography(content = "Working".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "Primitives + reactivity + render walker; `ui!` / `jsx!` / \
                `#[component]` / `stylesheet!`; reactive `if` / `when` / `for` in the DSLs; \
                refs via `Ref<H>`; idea-ui component library; icon registry; Robot + MCP \
                introspection; hot-reload dev server with runtime-server shell; \
                server-driven UI over the wire protocol; server-side rendering to \
                HTML + CSS at a URL (`backend-ssr`).".to_string(),
                muted = true)
        },
        ui! { Typography(content = "In progress".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "Custom rendering via wgpu (skins for phone, tablet, \
                tv); native backend interactions / media / OS integration; in-place \
                hydration (DOM adoption + viewport-determinism prototype).".to_string(),
                muted = true)
        },
        ui! { Typography(content = "Planned".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(content = "Async data + `Resource<T>`; first-class accessibility \
                across every primitive.".to_string(),
                muted = true)
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}
