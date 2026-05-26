//! Persistent sidebar — mounted once via `DrawerNavigator::sidebar_with`.
//!
//! On web, drawer-navigator renders as a permanent flex-row layout
//! (sidebar + body). The sidebar mounts ONCE at navigator init and
//! survives every screen change — the navigator only swaps the
//! body. On iOS/Android the same drawer SDK becomes a slide-in
//! side panel.
//!
//! Active-link highlight: `DrawerSlotProps::active_route` is a
//! reactive `Signal<&'static str>` that the navigator updates on
//! every screen change. Each nav link reads it inside its style
//! closure, so the highlight updates without rebuilding the
//! sidebar tree.

use std::rc::Rc;

use runtime_core::{signal, ui, Primitive, Signal, StyleApplication};
use drawer_navigator::DrawerSlotProps;
use idea_ui::{
    dark_theme, light_theme, set_idea_theme, spacer, switch, typography, TypographyKind,
    TypographyTone,
};

use crate::routes::{
    AGENTIC_ROUTE, BACKENDS_ROUTE, CONCEPTS_ROUTE, DEMO_ANIMATIONS_ROUTE, DEMO_COMPONENTS_ROUTE,
    DEMO_COUNTER_ROUTE, DEMO_NAVIGATION_ROUTE, FURTHER_READING_ROUTE, HOME_ROUTE, INSTALL_ROUTE,
    QUICKSTART_ROUTE, SECTIONS, WHY_RUST_ROUTE,
};
use crate::styles::{
    NavLink, PageColumn, PageRow, ScreenScroll, SidebarBody, SidebarFooter, SidebarHeader,
    SidebarSection, TocHeader, TocLink, TocPanel,
};

/// One entry in a page's table-of-contents. `id` matches the
/// `AccessibilityProps::identifier` set on the section's outer
/// view (which web emits as the DOM `id` attribute), so the
/// TOC entry can scroll to or highlight the matching section.
#[derive(Clone)]
pub struct TocEntry {
    pub id: &'static str,
    pub label: &'static str,
}

/// Wrap a page's content in a `ScrollView` sized to the drawer
/// body. The drawer-navigator's `.ui-nav-drawer-body` div has
/// `overflow: hidden`, so the screen needs its own scroll context
/// for long content. On native targets where the drawer SDK
/// supplies the scroll affordance (UIScrollView / Android
/// NestedScrollView), the `ScrollView` is the same primitive —
/// one author tree, every backend.
pub fn layout(content: Primitive) -> Primitive {
    let scroll_style = ScreenScroll();
    ui! {
        ScrollView(style = scroll_style) {
            content
        }
    }
}

/// Variant of [`layout`] that adds a Material-UI-style table-of-
/// contents column to the right of the page. Each `TocEntry::id`
/// must match an `AccessibilityProps::identifier` set on a section
/// view inside `content` \u{2014} typically via `page_section(id, ...)`
/// from `pages::common`.
///
/// The TOC's active link is driven by a viewport scroll-spy: an
/// `IntersectionObserver` watches every matching DOM id; as
/// sections scroll across the viewport, the matching link
/// highlights. Web-only; on native targets the TOC renders as a
/// regular list with no observer (native nav idioms don't share
/// this pattern).
pub fn layout_with_toc(content: Primitive, entries: Vec<TocEntry>) -> Primitive {
    let scroll_style = ScreenScroll();
    let row_style = PageRow();
    let column_style = PageColumn();
    let toc = render_toc(entries);
    ui! {
        ScrollView(style = scroll_style) {
            View(style = row_style) {
                View(style = column_style) { content }
                toc
            }
        }
    }
}

/// Render the TOC panel + install the scroll-spy. `active_id` is
/// the signal the TOC links subscribe to; the observer updates it
/// as sections cross the viewport.
fn render_toc(entries: Vec<TocEntry>) -> Primitive {
    let panel_style = TocPanel();
    let header_style = TocHeader();
    // App-level reactive cell for the currently-visible section.
    // Lives in the layout's scope, so it drops with the screen on
    // navigation \u{2014} the observer's `on_cleanup` disconnect
    // (see `install_scroll_spy`) drops alongside it.
    let active_id: Signal<String> = signal!(String::new());

    let mut children: Vec<Primitive> = Vec::with_capacity(entries.len() + 1);
    children.push(ui! {
        Text(style = header_style) { "On this page" }
    });
    for entry in &entries {
        children.push(toc_link(entry.id, entry.label, active_id));
    }

    install_scroll_spy(entries, active_id);

    ui! { View(style = panel_style) { children } }
}

/// One TOC link. The style closure reads `active_id` reactively to
/// flip the `active` variant on/off as the user scrolls. Click
/// dispatches a smooth scroll to the matching section.
fn toc_link(id: &'static str, label: &'static str, active_id: Signal<String>) -> Primitive {
    let label_text = label.to_string();
    let style = move || {
        let variant = if active_id.get() == id { "on" } else { "off" };
        StyleApplication::new(TocLink::sheet()).with("active", variant.to_string())
    };
    let children: Vec<Primitive> = vec![ui! { Text(style = style) { label_text } }];
    // `runtime_core::pressable(children, on_click)` is the framework
    // primitive \u{2014} idea-ui's `Btn` wraps it; we call it directly
    // here because the TOC entry's styling doesn't match Btn's
    // intent/kind axes (it's a sidebar-style nav, not an action).
    let bound = runtime_core::pressable(children, move || scroll_to_section(id));
    runtime_core::IntoPrimitive::into_primitive(bound)
}

/// Smooth-scroll the document to the element with the given id.
/// Web-only; native targets no-op (TOC links on iOS / Android
/// don't have an analogous behavior here).
fn scroll_to_section(id: &'static str) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsValue;
        let Some(window) = web_sys::window() else { return };
        let Some(doc) = window.document() else { return };
        let Some(elem) = doc.get_element_by_id(id) else { return };
        let opts = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&opts, &"behavior".into(), &"smooth".into());
        let _ = js_sys::Reflect::set(&opts, &"block".into(), &"start".into());
        let _ = js_sys::Function::from(
            js_sys::Reflect::get(&elem, &JsValue::from_str("scrollIntoView"))
                .expect("scrollIntoView missing"),
        )
        .call1(&elem, &opts);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = id;
    }
}

/// Install a scroll-spy that highlights the TOC entry of whichever
/// section is currently nearest the top of the scrolling viewport.
///
/// Uses a `scroll` event listener (not `IntersectionObserver`)
/// because the page content scrolls inside a nested `ScrollView`
/// (the drawer body's `overflow:auto` div), not the document
/// viewport. An IntersectionObserver with the default root would
/// never fire \u{2014} sections only cross *their own* scroll
/// container, not the viewport. We could set the observer's
/// `root` to that container, but a scroll listener with
/// `getBoundingClientRect` is simpler and avoids the API/version
/// matrix on `IntersectionObserverInit::set_root`.
///
/// Cleanup removes the listener on scope drop.
fn install_scroll_spy(entries: Vec<TocEntry>, active_id: Signal<String>) {
    #[cfg(target_arch = "wasm32")]
    {
        use std::cell::RefCell;
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;

        // Defer to a microtask so the section elements are mounted
        // before we try to attach the listener.
        let entries_for_setup = entries.clone();
        runtime_core::schedule_microtask(move || {
            let Some(window) = web_sys::window() else { return };
            let Some(doc) = window.document() else { return };

            // Find the scrolling ancestor by walking up from the
            // first section element. The drawer body itself has
            // `overflow: hidden`, and inside it the page's own
            // `ScrollView` is the actual scrolling container.
            let scroll_root: Option<web_sys::Element> = entries_for_setup
                .iter()
                .find_map(|e| doc.get_element_by_id(e.id))
                .and_then(|el| find_scroll_root(&window, &el));

            // The "active band" sits ~25% from the top of the scroll
            // root \u{2014} the section whose top is closest to (but
            // not below) this line wins. Matches MUI / docs-site
            // convention. We recompute it each scroll tick so window
            // resizes don't desync it.
            let entries_for_cb = entries_for_setup.clone();
            let scroll_root_for_cb = scroll_root.clone();
            let window_for_cb = window.clone();
            let active_for_cb = active_id;
            let doc_for_cb = doc.clone();
            let update_active = move || {
                let (root_top, root_height) = match &scroll_root_for_cb {
                    Some(root) => {
                        let rect = root.get_bounding_client_rect();
                        (rect.top(), rect.height())
                    }
                    None => (
                        0.0,
                        window_for_cb
                            .inner_height()
                            .ok()
                            .and_then(|v| v.as_f64())
                            .unwrap_or(800.0),
                    ),
                };
                let target_y = root_top + root_height * 0.25;

                // Walk every tracked section; pick the one whose
                // top is the largest value still \u{2264} target_y.
                // If no section has crossed yet (we're above the
                // first one), fall back to the first entry so the
                // TOC always shows *something* highlighted.
                let mut best_id: Option<String> = None;
                let mut best_top: f64 = f64::NEG_INFINITY;
                for e in &entries_for_cb {
                    if let Some(elem) = doc_for_cb.get_element_by_id(e.id) {
                        let top = elem.get_bounding_client_rect().top();
                        if top <= target_y && top > best_top {
                            best_top = top;
                            best_id = Some(e.id.to_string());
                        }
                    }
                }
                let next = best_id.unwrap_or_else(|| {
                    entries_for_cb
                        .first()
                        .map(|e| e.id.to_string())
                        .unwrap_or_default()
                });
                if active_for_cb.get() != next {
                    active_for_cb.set(next);
                }
            };

            // Initial calc so the top section highlights before the
            // user scrolls.
            update_active();

            let cb = Closure::wrap(Box::new(move |_evt: web_sys::Event| {
                update_active();
            }) as Box<dyn FnMut(web_sys::Event)>);

            // Attach the listener to the scrolling element if found,
            // else the window (defensive fallback). `scroll` events
            // don't bubble, so we have to listen on the right node.
            let listener_target: web_sys::EventTarget = match &scroll_root {
                Some(root) => root.clone().unchecked_into(),
                None => window.clone().unchecked_into(),
            };
            let _ = listener_target
                .add_event_listener_with_callback("scroll", cb.as_ref().unchecked_ref());

            // Anchor the listener + closure to the layout's scope
            // so they live as long as the page is mounted and drop
            // when the navigator unmounts the screen.
            let target_for_cleanup = listener_target.clone();
            let cb_box = Rc::new(RefCell::new(Some(cb)));
            let cb_for_cleanup = cb_box.clone();
            runtime_core::on_cleanup(move || {
                if let Some(cb) = cb_for_cleanup.borrow_mut().take() {
                    let _ = target_for_cleanup.remove_event_listener_with_callback(
                        "scroll",
                        cb.as_ref().unchecked_ref(),
                    );
                }
            });
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (entries, active_id);
    }
}

/// Walk up the DOM from `start` until we find an element whose
/// computed `overflow-y` (or `overflow`) allows scrolling. Returns
/// `None` if no such ancestor exists \u{2014} the caller should fall
/// back to the window/viewport.
#[cfg(target_arch = "wasm32")]
fn find_scroll_root(
    window: &web_sys::Window,
    start: &web_sys::Element,
) -> Option<web_sys::Element> {
    let mut cur = start.parent_element();
    while let Some(p) = cur {
        if let Ok(Some(style)) = window.get_computed_style(&p) {
            let oy = style.get_property_value("overflow-y").unwrap_or_default();
            let o = style.get_property_value("overflow").unwrap_or_default();
            let scrollable = |v: &str| v == "auto" || v == "scroll" || v == "overlay";
            if scrollable(&oy) || scrollable(&o) {
                return Some(p);
            }
        }
        cur = p.parent_element();
    }
    None
}

/// Build the persistent sidebar. Called once by drawer-navigator
/// during `init`; the returned Primitive's reactive scope survives
/// for the navigator's entire lifetime.
///
/// `is_dark` is an app-level signal lifted out of `app()` so the
/// theme-toggle's state survives navigation (signals scoped to a
/// screen would reset on every push). Toggling it both flips the
/// signal AND swaps the installed idea-ui theme via
/// `set_idea_theme(...)`.
pub fn sidebar(slot: DrawerSlotProps, is_dark: Signal<bool>) -> Primitive {
    let body_style = SidebarBody();
    let header_style = SidebarHeader();
    let footer_style = SidebarFooter();

    let header_children: Vec<Primitive> = vec![
        ui! { Typography(content = "Idealyst".to_string(), kind = TypographyKind::H3) },
        ui! {
            Typography(
                content = "One codebase, native everywhere.".to_string(),
                tone = TypographyTone::Muted,
            )
        },
    ];

    let active_route = slot.active_route;

    let mut children: Vec<Primitive> = Vec::new();
    children.push(ui! { View(style = header_style) { header_children } });

    for section in SECTIONS {
        if !section.title.is_empty() {
            let title = section.title.to_string();
            let section_style = SidebarSection();
            children.push(ui! { Text(style = section_style) { title } });
        }
        for entry in section.entries {
            children.push(nav_link(entry.name, entry.label, active_route));
        }
    }

    // `Spacer` grows to fill the leftover vertical space, pinning
    // the footer to the bottom of the sidebar column when nav
    // content is short. When nav content overflows, the spacer has
    // no room to grow and the footer just sits after the last nav
    // link (the outer `.ui-nav-drawer-sidebar` div is overflow:auto
    // so the whole sidebar scrolls in that case).
    children.push(ui! { Spacer() });
    children.push(theme_toggle(footer_style, is_dark));

    ui! { View(style = body_style) { children } }
}

/// Dark/light theme switch pinned to the bottom of the sidebar.
/// Flips `is_dark` AND swaps the installed `IdeaTheme` so every
/// component re-renders against the new token set.
fn theme_toggle(footer_style: SidebarFooter, is_dark: Signal<bool>) -> Primitive {
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |dark| {
        is_dark.set(dark);
        if dark {
            set_idea_theme(dark_theme());
        } else {
            set_idea_theme(light_theme());
        }
    });

    let row_children: Vec<Primitive> = vec![
        ui! {
            Switch(
                label = Some("Dark mode".to_string()),
                value = is_dark,
                on_change = on_change,
            )
        },
    ];

    ui! { View(style = footer_style) { row_children } }
}

/// One sidebar nav link. Routes are matched by name; each emits a
/// `Link` to the corresponding `Route<()>` constant, which the
/// drawer SDK rewrites to a `Select` command. The style closure
/// reads `active_route` so the active variant flips reactively
/// without rebuilding the link.
fn nav_link(
    name: &'static str,
    label: &'static str,
    active_route: runtime_core::Signal<&'static str>,
) -> Primitive {
    let route_for_match: &'static str = name;
    let style = move || {
        let variant = if active_route.get() == route_for_match {
            "on"
        } else {
            "off"
        };
        StyleApplication::new(NavLink::sheet()).with("active", variant.to_string())
    };
    let label_text = label.to_string();

    match name {
        "home" => ui! {
            Link(route = &HOME_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "install" => ui! {
            Link(route = &INSTALL_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "quickstart" => ui! {
            Link(route = &QUICKSTART_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "concepts" => ui! {
            Link(route = &CONCEPTS_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "why-rust" => ui! {
            Link(route = &WHY_RUST_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "demo-counter" => ui! {
            Link(route = &DEMO_COUNTER_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "demo-components" => ui! {
            Link(route = &DEMO_COMPONENTS_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "demo-animations" => ui! {
            Link(route = &DEMO_ANIMATIONS_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "demo-navigation" => ui! {
            Link(route = &DEMO_NAVIGATION_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "backends" => ui! {
            Link(route = &BACKENDS_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "agentic" => ui! {
            Link(route = &AGENTIC_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        "further-reading" => ui! {
            Link(route = &FURTHER_READING_ROUTE, params = ()) {
                Text(style = style) { label_text }
            }
        },
        _ => ui! { Text { label_text } },
    }
}
