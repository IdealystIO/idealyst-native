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

use runtime_core::primitives::scroll_view::{scroll_view, ScrollViewHandle};
use runtime_core::{
    effect, pressable, signal, text, ui, view, when, IntoPrimitive, Primitive, Ref, Signal,
    StyleApplication, ViewHandle,
};
use drawer_navigator::DrawerSlotProps;
use idea_ui::{
    current_breakpoint, dark_theme, light_theme, set_idea_theme, spacer, switch, typography,
    Breakpoint, TypographyKind, TypographyTone,
};

use crate::routes::{
    label_for_route, AGENTIC_ROUTE, BACKENDS_ROUTE, CONCEPTS_ROUTE, DEMO_ANIMATIONS_ROUTE,
    DEMO_COMPONENTS_ROUTE, DEMO_COUNTER_ROUTE, DEMO_NAVIGATION_ROUTE, FURTHER_READING_ROUTE,
    HOME_ROUTE, INSTALL_ROUTE, QUICKSTART_ROUTE, SECTIONS, SERVER_FUNCTIONS_ROUTE, WHY_RUST_ROUTE,
};
use crate::styles::{
    MobileHeader, MobileHeaderButton, MobileHeaderTitle, MobileHeaderTitleWrap, NavLink,
    PageColumn, PageRow, ScreenScroll, SidebarBody, SidebarFooter, SidebarHeader, SidebarSection,
    TocHeader, TocLink, TocPanel,
};

/// One entry in a page's table-of-contents. `handle` is a
/// `Ref<ViewHandle>` for the section's outer `View` (allocated by
/// the page via `Ref::<ViewHandle>::new()` and shared with the
/// matching `page_section(handle, ...)` call). The TOC reads each
/// handle's `absolute_frame()` to drive the active highlight and
/// compute the click-to-scroll target \u{2014} all portable across
/// every Backend impl, no `cfg(target_arch)` reaching into platform
/// APIs.
#[derive(Copy, Clone)]
pub struct TocEntry {
    pub handle: Ref<ViewHandle>,
    pub label: &'static str,
}

/// Wrap a page's content in a `ScrollView` sized to the drawer
/// body. The drawer-navigator's `.ui-nav-drawer-body` div has
/// `overflow: hidden`, so the screen needs its own scroll context
/// for long content. On native targets where the drawer SDK
/// supplies the scroll affordance (UIScrollView / Android
/// NestedScrollView), the `ScrollView` is the same primitive —
/// one author tree, every backend.
///
/// At narrow viewports the layout also mounts a [`mobile_header`]
/// pinned to the screen top — the `when()` is reactive on
/// `current_breakpoint()`, so a window resize across the
/// `Md`/`Sm` boundary mounts / unmounts the bar without rebuilding
/// the rest of the screen.
pub fn layout(content: Primitive) -> Primitive {
    let scroll_style = crate::responsive::responsive_style(ScreenScroll::sheet());
    let header = when(
        || matches!(current_breakpoint().get(), Breakpoint::Xs | Breakpoint::Sm),
        mobile_header,
        || view(Vec::<Primitive>::new()).into_primitive(),
    );
    ui! {
        View {
            header
            ScrollView(style = scroll_style) {
                content
            }
        }
    }
}

/// Y-line (in viewport coords, relative to the scroll view's top)
/// where a section is considered "active" once its own top crosses
/// past. Matches the MUI / docs-site convention of ~25 % of the
/// reading area. Constant rather than a fraction of the actual
/// scroll viewport because we don't want to query layout from inside
/// the reactive effect \u{2014} a fixed band is good enough for the
/// docs reading pattern (sections of roughly comparable length).
const ACTIVE_BAND_Y: f32 = 160.0;

/// Variant of [`layout`] that adds a Material-UI-style table-of-
/// contents column to the right of the page. Each `TocEntry::handle`
/// must match the `Ref<ViewHandle>` passed to the corresponding
/// `page_section(handle, ...)` call inside `content`.
///
/// The TOC's active link is driven by `ScrollView::on_scroll` \u{2014}
/// a framework primitive that fires uniformly across every backend
/// (web `scroll` event, iOS `UIScrollViewDelegate`, Android
/// `OnScrollChangeListener`, macOS `NSViewBoundsDidChange`, wgpu host
/// tick). The author-side code is target-agnostic: write to a
/// `Signal<f32>`, read each section's `ViewHandle::absolute_frame()`
/// inside an `effect!`, set the active index.
pub fn layout_with_toc(content: Primitive, entries: Vec<TocEntry>) -> Primitive {
    let row_style = crate::responsive::responsive_style(PageRow::sheet());
    let column_style = PageColumn();

    // `Signal<f32>` written by the ScrollView's `on_scroll`
    // callback. Reads inside the active-link effect subscribe; the
    // effect re-runs every scroll tick and recomputes which section
    // sits in the active band.
    let scroll_y: Signal<f32> = signal!(0.0_f32);
    // The currently-active TocEntry index. `None` while we're above
    // the first section; otherwise `Some(i)`.
    let active_idx: Signal<Option<usize>> = signal!(None);
    // Handle to the page's own ScrollView \u{2014} used by `toc_link`
    // to dispatch programmatic scrolls when the user clicks an entry.
    let scroll_ref: Ref<ScrollViewHandle> = Ref::new();

    install_scroll_spy(entries.clone(), scroll_y, active_idx);

    // The TOC ("On this page") column is only rendered at Lg or
    // wider — below that it crowds the prose. `when(...)` swaps
    // between the TOC subtree and an empty placeholder reactively,
    // so a window resize across the threshold mounts/unmounts the
    // TOC + its scroll-spy effect rather than just hiding via CSS.
    // Capture the closure inputs by value (entries.clone(), Copy
    // signals/refs) so the cond/then closures are 'static.
    let toc_entries = entries;
    let toc = when(
        move || {
            // Read inside the derived so the layout's reactive scope
            // subscribes to the breakpoint signal — when it crosses
            // the threshold, the `when` re-evaluates and mounts /
            // unmounts the TOC subtree.
            matches!(current_breakpoint().get(), Breakpoint::Lg | Breakpoint::Xl)
        },
        move || render_toc(toc_entries.clone(), active_idx, scroll_ref, scroll_y),
        || view(Vec::<Primitive>::new()).into_primitive(),
    );

    let body = ui! {
        View(style = row_style) {
            View(style = column_style) { content }
            toc
        }
    };

    let scroll = scroll_view(vec![body])
        .bind(scroll_ref)
        .on_scroll(move |_x, y| scroll_y.set(y))
        .with_style(crate::responsive::responsive_style(ScreenScroll::sheet()))
        .into_primitive();

    // Mobile header is reactive on the breakpoint just like the
    // non-TOC `layout()` above. Mounts only at narrow widths.
    let header = when(
        || matches!(current_breakpoint().get(), Breakpoint::Xs | Breakpoint::Sm),
        mobile_header,
        || view(Vec::<Primitive>::new()).into_primitive(),
    );
    ui! { View { header scroll } }
}

/// Mobile-style top bar — menu button on the left, current screen's
/// title centered (well, leading-aligned in the available space) on
/// the right. Pinned to the screen top via `position: absolute` from
/// the `MobileHeader` stylesheet; visible only at narrow widths via
/// the reactive `when(...)` gate in [`layout`] / [`layout_with_toc`].
///
/// The title text is reactive on the SDK's `active_route` signal —
/// captured into a thread-local in
/// [`crate::responsive::set_active_route`] (called from the sidebar
/// builder where the SDK passes it through). Pre-bind (before the
/// navigator's first build), the title is empty rather than panic.
///
/// The trailing slot is intentionally empty for now; the function
/// can take `Option<Primitive>` arguments when the site grows a
/// per-screen action (share, link to source, etc.).
pub fn mobile_header() -> Primitive {
    let header_style = MobileHeader();
    let title_wrap_style = MobileHeaderTitleWrap();
    let title_style = move || StyleApplication::new(MobileHeaderTitle::sheet());
    let button_style = move || StyleApplication::new(MobileHeaderButton::sheet());

    // --- menu button (leading) ---
    let menu_icon: Primitive = ui! { Text(style = button_style) { "\u{2630}" } };
    let menu_button = pressable(vec![menu_icon], || crate::responsive::open_drawer())
        .into_primitive();

    // --- title (center) ---
    // Reading `active_route_signal()` returns the stable mirror;
    // `.get()` subscribes the surrounding text-reactive scope so
    // the title re-renders on every navigation.
    let title_source = || {
        let sig = crate::responsive::active_route_signal();
        label_for_route(sig.get()).to_string()
    };
    let title_view: Primitive = text(title_source).with_style(title_style).into_primitive();
    let title_node = ui! {
        View(style = title_wrap_style) { title_view }
    };

    ui! {
        View(style = header_style) {
            menu_button
            title_node
        }
    }
}

/// Render the TOC panel. The active highlight is driven by
/// `active_idx`; clicks dispatch `scroll_ref.scroll_to(...)`
/// computed from each section's `absolute_frame()`.
fn render_toc(
    entries: Vec<TocEntry>,
    active_idx: Signal<Option<usize>>,
    scroll_ref: Ref<ScrollViewHandle>,
    scroll_y: Signal<f32>,
) -> Primitive {
    let panel_style = TocPanel();
    let header_style = TocHeader();

    let mut children: Vec<Primitive> = Vec::with_capacity(entries.len() + 1);
    children.push(ui! {
        Text(style = header_style) { "On this page" }
    });
    for (i, entry) in entries.iter().enumerate() {
        children.push(toc_link(i, *entry, active_idx, scroll_ref, scroll_y));
    }

    ui! { View(style = panel_style) { children } }
}

/// One TOC link. The style closure reads `active_idx` reactively
/// to flip the `active` variant. Click computes the target offset
/// from the section's current viewport position and the current
/// scroll y, then calls `scroll_ref.scroll_to(0, target)` \u{2014}
/// all via framework primitives.
fn toc_link(
    index: usize,
    entry: TocEntry,
    active_idx: Signal<Option<usize>>,
    scroll_ref: Ref<ScrollViewHandle>,
    scroll_y: Signal<f32>,
) -> Primitive {
    let label_text = entry.label.to_string();
    let style = move || {
        let variant = if active_idx.get() == Some(index) { "on" } else { "off" };
        StyleApplication::new(TocLink::sheet()).with("active", variant.to_string())
    };
    let children: Vec<Primitive> = vec![ui! { Text(style = style) { label_text } }];

    let bound = runtime_core::pressable(children, move || {
        // Section's current Y in window-coords (moves as user scrolls).
        // Subtract `ACTIVE_BAND_Y` so the section ends up at the same
        // line the spy's active band uses \u{2014} click + spy stay in
        // sync.
        let section_y = entry
            .handle
            .with(|h| h.absolute_frame())
            .flatten()
            .map(|r| r.y)
            .unwrap_or(0.0);
        let current_scroll = scroll_y.get();
        let target = (current_scroll + section_y - ACTIVE_BAND_Y).max(0.0);
        let _ = scroll_ref.with(|h| h.scroll_to(0.0, target));
    });
    runtime_core::IntoPrimitive::into_primitive(bound)
}

/// Reactively pick the active TocEntry index whenever the scroll
/// position changes. Reads `scroll_y` (subscribing) so the effect
/// re-runs every tick; then reads each entry's
/// `absolute_frame()` to compute its current viewport Y. The
/// section with the largest top still <= `ACTIVE_BAND_Y` wins.
///
/// No `cfg(target_arch)`, no `web_sys`, no `IntersectionObserver`.
/// One author tree, every backend \u{2014} the per-platform plumbing
/// is the `ScrollView::on_scroll` callback and
/// `ViewHandle::absolute_frame()`, both Backend-trait primitives.
fn install_scroll_spy(
    entries: Vec<TocEntry>,
    scroll_y: Signal<f32>,
    active_idx: Signal<Option<usize>>,
) {
    effect!({
        // Subscribe to scroll position. The `get()` registers this
        // effect as a dependent; subsequent `set()` calls from the
        // `on_scroll` callback retrigger.
        let _ = scroll_y.get();

        let mut best: Option<usize> = None;
        let mut best_top: f32 = f32::NEG_INFINITY;
        for (i, entry) in entries.iter().enumerate() {
            let Some(rect) = entry.handle.with(|h| h.absolute_frame()).flatten() else {
                continue;
            };
            if rect.y <= ACTIVE_BAND_Y && rect.y > best_top {
                best_top = rect.y;
                best = Some(i);
            }
        }
        // Default to the first entry when we're above all sections
        // (e.g. at the page header) so the TOC always shows something
        // highlighted instead of going blank.
        if best.is_none() && !entries.is_empty() {
            best = Some(0);
        }
        if active_idx.get() != best {
            active_idx.set(best);
        }
    });
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
        "server-functions" => ui! {
            Link(route = &SERVER_FUNCTIONS_ROUTE, params = ()) {
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
