//! Navigation — stack / drawer / tab patterns + native back behavior.

use runtime_core::{ui, Primitive, Ref, ViewHandle};
use idea_ui::{stack, typography, StackGap, TypographyKind};

use crate::pages::common::{code_panel, page_header, page_section};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Primitive {
    let sdks_ref: Ref<ViewHandle> = Ref::new();
    let stack_ref: Ref<ViewHandle> = Ref::new();
    let drawer_ref: Ref<ViewHandle> = Ref::new();
    let tabs_ref: Ref<ViewHandle> = Ref::new();
    let back_ref: Ref<ViewHandle> = Ref::new();
    let extending_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: sdks_ref, label: "Three navigator SDKs" },
        TocEntry { handle: stack_ref, label: "Stack navigator" },
        TocEntry { handle: drawer_ref, label: "Drawer navigator" },
        TocEntry { handle: tabs_ref, label: "Tab navigator" },
        TocEntry { handle: back_ref, label: "Native back, for free" },
        TocEntry { handle: extending_ref, label: "Adding a new navigator" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Navigation",
                "Stacks, drawers, and tabs \u{2014} the platform-native navigation \
                 idioms surfaced through one cross-platform API. Native back gestures \
                 work for free; the URL bar on web is real."
            ) }
            { page_section(sdks_ref, vec![sdks()]) }
            { page_section(stack_ref, vec![stack_pattern()]) }
            { page_section(drawer_ref, vec![drawer_pattern()]) }
            { page_section(tabs_ref, vec![tab_pattern()]) }
            { page_section(back_ref, vec![native_back()]) }
            { page_section(extending_ref, vec![extending()]) }
        }
    };
    layout_with_toc(content, toc)
}

fn sdks() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Three navigator SDKs".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Navigation isn't built into the framework core. Each \
                navigator (stack, drawer, tab) is a separate SDK crate that plugs in \
                via the `Primitive::Navigator` external mechanism. The framework \
                provides the substrate (route table, URL sync, history); the SDKs \
                provide the per-platform chrome.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn stack_pattern() -> Primitive {
    let snippet = "use stack_navigator::{Navigator, StackBuilder, StackHandle};\n\
                   \n\
                   const HOME: Route<()> = Route::<()>::new(\"home\", \"/\");\n\
                   const DETAIL: Route<()> = Route::<()>::new(\"detail\", \"/detail\");\n\
                   \n\
                   let nav: Ref<StackHandle> = Ref::new();\n\
                   let builder = Navigator::new(&HOME)\n    \
                       .screen(HOME, move |_| home_page(nav))\n    \
                       .screen(DETAIL, move |_| detail_page(nav));\n\
                   \n\
                   ui! { builder.bind(nav) }";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Stack navigator".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Push/pop screens onto a stack. On iOS this is a \
                `UINavigationController` with native back gestures and headers; on \
                Android a `FragmentManager` with system back-button handling; on web \
                a history-API-backed unmount-on-push DOM swap.".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "Push from any screen via the navigator handle: \
                `nav.get().map(|h| h.push(&DETAIL, ()))`. Pop via `h.pop()`. The user's \
                back gesture / browser back button does the same thing.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn drawer_pattern() -> Primitive {
    let snippet = "use drawer_navigator::{DrawerBuilder, DrawerHandle, DrawerNavigator};\n\
                   \n\
                   let nav: Ref<DrawerHandle> = Ref::new();\n\
                   let builder = DrawerNavigator::new(&HOME)\n    \
                       .screen(HOME, move |_| home_page())\n    \
                       .screen(SETTINGS, move |_| settings_page())\n    \
                       .sidebar_with(|slot| sidebar(slot.active_route));\n\
                   \n\
                   ui! { builder.bind(nav) }";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Drawer navigator".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "A persistent sidebar slot + a swappable body. On mobile \
                the sidebar becomes a slide-out drawer with native chrome; on web (large \
                viewport) it's always visible alongside the body. This page is built \
                with the drawer navigator.".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "`sidebar_with(...)` receives reactive `DrawerSlotProps` \
                so the sidebar can highlight the active route, observe open/close state, \
                or fire programmatic selects \u{2014} all without rebuilding the sidebar \
                tree on navigation.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn tab_pattern() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Tab navigator".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Bottom-tab navigation \u{2014} the iOS / Android idiom \
                for top-level sections. The tab bar lives at the bottom (or on the side, \
                depending on platform); switching tabs swaps the body. Same shape as the \
                drawer: persistent chrome slot + screen swap.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn native_back() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Native back, for free".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "The platform's back affordance just works. iOS swipe-from-left, \
                Android system back button, browser back button \u{2014} all route through \
                the same navigator state. You don't write a back button; the user already \
                has one.".to_string())
        },
        ui! {
            Typography(content = "On web, the URL bar is real: deep-linked URLs load the \
                right screen, the back button walks visited screens in order, share links \
                point at the page the user was on. The navigator manages \
                `history.pushState`/`replaceState` per `Link.kind(NavKind::X)` semantics.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn extending() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Adding a new navigator".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(content = "Navigators are plug-in extensions. Each SDK crate is \
                ~200\u{2013}500 lines: a typed presentation struct, a per-backend handler, \
                an installation hook. The framework's core doesn't know what \"tab\" or \
                \"drawer\" means \u{2014} those are SDK definitions all the way down.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}
