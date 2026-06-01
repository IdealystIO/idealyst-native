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

use runtime_core::primitives::scroll_view::ScrollViewHandle;
use runtime_core::{
    component, derived, effect, icon, pressable, signal, text, ui, view, when, Easing,
    IntoElement, Element, Ref, Route, Signal, StrokeAnimation, StyleApplication, ViewHandle,
};
use drawer_navigator::SlotProps;
use idea_ui::{
    current_breakpoint, dark_theme, light_theme, set_idea_theme, Spacer, Switch, Typography,
    Breakpoint,
};

use crate::branding::LIGHT_LOGO;
use crate::routes::{
    label_for_route, BACKENDS_ROUTE, CONCEPTS_ROUTE, HOME_ROUTE, QUICKSTART_ROUTE, SECTIONS,
    WHY_RUST_ROUTE,
};
use crate::styles::{
    Footer, FooterBottom, FooterBrand, FooterColumn, FooterCopy, FooterGrid, FooterLink,
    FooterTagline, FooterTitle, FooterWordmark, MobileHeader, MobileHeaderButton,
    MobileHeaderTitle, MobileHeaderTitleWrap, NavLink, NavLinkActive, PageColumn, PageRow,
    PageScrollColumn, ScreenScroll,
    SidebarBody, SidebarBrandRow, SidebarBrandText, SidebarFooter, SidebarHeader, SidebarLogo,
    SidebarSection, TocHeader, TocLink, TocPanel,
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

/// Wrap a screen's content in the website's OWN `scroll_view`.
///
/// The drawer navigator no longer owns scroll — its body is a
/// non-scrolling flex child and `ambient_scroll_context()` always
/// returns `None`. So each page provides its own scroll surface: a
/// `scroll_view` that fills the navigator body and scrolls the page
/// content together with the site footer as a single column. The
/// footer is the LAST child INSIDE the `scroll_view`, so it scrolls
/// into view at the end of the page rather than being a pinned slot.
///
/// The mobile header still lives in the navigator's `top` slot — see
/// `lib.rs`'s `.top_with(...)`. There is no `bottom` slot anymore.
pub fn layout(content: Element) -> Element {
    let style = ScreenScroll();
    ui! {
        scroll_view(style = style) {
            content
            footer()
        }
    }
}

/// Fraction of the body viewport where the "active band" sits — a
/// section becomes active once its top scrolls above this line.
/// 30 % from the top is the MUI / Tailwind-docs / VitePress
/// convention: high enough that the user clearly sees the section
/// they just scrolled to (the heading is *above* their reading
/// point), low enough that short sections still pass through the
/// band as the user scrolls.
const ACTIVE_BAND_FRACTION: f32 = 0.30;

/// Pixel threshold for "scrolled to the bottom" — within this many
/// pixels of the body's `scrollHeight - clientHeight`, the spy
/// force-selects the last TOC entry. Without this, a final
/// section shorter than `clientHeight * (1 - ACTIVE_BAND_FRACTION)`
/// would never become active, because the user can't scroll
/// further once they hit the end of the body.
const END_OF_SCROLL_EPSILON: f32 = 8.0;

/// Pixel tolerance for the band-compare in the scroll-spy. When a
/// click programmatically scrolls a section to exactly `band_y`,
/// rounding (browser `scrollTop` is integer; signals are `f32`)
/// can put the section 1–4 px BELOW the band line — failing a
/// strict `<= band_y` check and handing active state to the
/// previous section. Padding the band by this much keeps the
/// clicked section highlighted across the click-and-spy
/// round-trip.
const BAND_COMPARE_TOLERANCE: f32 = 8.0;

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
pub fn layout_with_toc(content: Element, entries: Vec<TocEntry>) -> Element {
    let row_style = crate::responsive::responsive_style(PageRow::sheet());
    let column_style = PageColumn();

    // The website owns its own scroll now (the drawer navigator no
    // longer publishes a scroll context). Drive `scroll_y` from the
    // page `scroll_view`'s `on_scroll` callback instead of reading
    // `ambient_scroll_context()`.
    let scroll_y: Signal<f32> = signal!(0.0_f32);
    let active_idx: Signal<Option<usize>> = signal!(None);

    // Geometry handles, all in WINDOW coordinates via
    // `ViewHandle::absolute_frame()`:
    //   - `viewport_ref`  → the scroll viewport wrapper. Its frame's
    //     `.y` is the body's top edge (`body_viewport_top`) and its
    //     `.height` is the visible (client) height.
    //   - `content_ref`   → the inner content column wrapping the
    //     whole scrolled tree (page row + footer). Its frame's
    //     `.height` is the total scrollable content height.
    //   - `scroll_ref`    → the `scroll_view` handle, used by TOC
    //     clicks to `scroll_to(0, target)`.
    let viewport_ref: Ref<ViewHandle> = Ref::<ViewHandle>::new();
    let content_ref: Ref<ViewHandle> = Ref::<ViewHandle>::new();
    let scroll_ref: Ref<ScrollViewHandle> = Ref::<ScrollViewHandle>::new();

    install_scroll_spy(entries.clone(), scroll_y, active_idx, viewport_ref, content_ref);

    // The TOC ("On this page") column is only rendered at Lg or
    // wider — below that it crowds the prose. `when(...)` swaps
    // between the TOC subtree and an empty placeholder reactively,
    // so a window resize across the threshold mounts/unmounts the
    // TOC + its scroll-spy effect rather than just hiding via CSS.
    let toc_entries = entries;
    let toc = when(
        move || {
            matches!(current_breakpoint().get(), Breakpoint::Lg | Breakpoint::Xl)
        },
        move || render_toc(
            toc_entries.clone(),
            active_idx,
            scroll_y,
            scroll_ref,
            viewport_ref,
            content_ref,
        ),
        || view(Vec::<Element>::new()).into_element(),
    );

    let body_style = ScreenScroll();
    // The scroll viewport wrapper tightly wraps the `scroll_view`
    // (which fills it via `ScreenScroll`'s flex_grow), so the
    // wrapper's `absolute_frame()` reports the visible scroll box.
    let viewport_wrap_style = ScreenScroll();
    // `content_col_style` is a plain content-sized flex column that
    // holds the entire scrolled tree (the page row + the footer) so a
    // single `absolute_frame().height` reads the total content height.
    let content_col_style = PageScrollColumn();
    ui! {
        view(style = viewport_wrap_style) {
            scroll_view(style = body_style) {
                view(style = content_col_style) {
                    view(style = row_style) {
                        view(style = column_style) { content }
                        toc
                    }
                    footer()
                }.bind(content_ref)
            }
                .bind(scroll_ref)
                .on_scroll(move |_x, y| scroll_y.set(y))
        }.bind(viewport_ref)
    }
}

// =============================================================================
// Footer
// =============================================================================

/// Project GitHub URL — referenced from the install snippet on
/// `/install` too. Keep both in sync if the repo ever moves.
const GITHUB_URL: &str = "https://github.com/IdealystIO/idealyst-native";
const GITHUB_ISSUES_URL: &str = "https://github.com/IdealystIO/idealyst-native/issues";
const GITHUB_DISCUSSIONS_URL: &str =
    "https://github.com/IdealystIO/idealyst-native/discussions";

/// Footer link to an off-app URL (GitHub, etc.). Uses the `link`
/// primitive's `external` form: on web a real `<a target="_blank">`
/// (browser-native, never popup-blocked), on native a platform
/// `open_url`. Same styling as `FooterLinkInternal` so the footer
/// reads uniformly.
///
/// Renamed from the snake_case `external_link` helper because it has
/// props and is called from multiple call sites — CLAUDE.md §9.5
/// requires the component form. The `FooterLink` *stylesheet* name is
/// taken, so the component is prefixed with the variant axis.
#[derive(Default)]
pub struct FooterLinkExternalProps {
    pub label: &'static str,
    pub url: &'static str,
}

#[component]
pub fn FooterLinkExternal(props: FooterLinkExternalProps) -> Element {
    let label_text = props.label.to_string();
    let url = props.url;
    let style = move || StyleApplication::new(FooterLink::sheet());
    ui! {
        link(external = url) {
            text(style = style) { label_text }
        }
    }
}

/// Footer link to a framework route — same styling as
/// `FooterLinkExternal` so the footer reads uniformly. Uses `link`
/// (not `pressable + nav.push`) so the SDK's link-activator dispatches
/// the right command for the active navigator (drawer = Select).
#[derive(Default)]
pub struct FooterLinkInternalProps {
    pub label: &'static str,
    pub route: Option<&'static Route<()>>,
}

#[component]
pub fn FooterLinkInternal(props: FooterLinkInternalProps) -> Element {
    let label_text = props.label.to_string();
    let route = props
        .route
        .expect("FooterLinkInternal requires a `route` prop");
    let style = move || StyleApplication::new(FooterLink::sheet());
    ui! {
        link(route = route, params = ()) {
            text(style = style) { label_text }
        }
    }
}

/// Build the site-wide footer. Rendered as the LAST child inside the
/// page's own `scroll_view` by [`layout`] / [`layout_with_toc`], so
/// it scrolls in at the end of the page content (it is no longer a
/// pinned navigator `bottom` slot — the drawer no longer owns
/// scroll). Mounted unconditionally; CSS variant switching handles
/// narrow-viewport stacking via the `size` variant on the footer
/// stylesheets.
pub fn footer() -> Element {
    let footer_style = crate::responsive::responsive_style(Footer::sheet());
    let grid_style = crate::responsive::responsive_style(FooterGrid::sheet());
    let bottom_style = FooterBottom();
    let wordmark_style = move || StyleApplication::new(FooterWordmark::sheet());
    let tagline_style = move || StyleApplication::new(FooterTagline::sheet());
    let title_style = move || StyleApplication::new(FooterTitle::sheet());
    let copy_style = move || StyleApplication::new(FooterCopy::sheet());

    // `FooterColumn()` / `FooterBrand()` etc return move-only style
    // sources, so each View call site needs its own instance.

    let brand = ui! {
        view(style = FooterBrand()) {
            text(style = wordmark_style) { "Idealyst" }
            text(style = tagline_style) { "One codebase, native everywhere." }
        }
    };

    let project_column = ui! {
        view(style = FooterColumn()) {
            text(style = title_style) { "Project" }
            FooterLinkExternal(label = "GitHub", url = GITHUB_URL)
            FooterLinkExternal(label = "Issues", url = GITHUB_ISSUES_URL)
            FooterLinkExternal(label = "Discussions", url = GITHUB_DISCUSSIONS_URL)
        }
    };

    let resources_column = ui! {
        view(style = FooterColumn()) {
            text(style = title_style) { "Resources" }
            FooterLinkInternal(label = "Quickstart", route = &QUICKSTART_ROUTE)
            FooterLinkInternal(label = "Core concepts", route = &CONCEPTS_ROUTE)
            FooterLinkInternal(label = "Why Rust", route = &WHY_RUST_ROUTE)
            FooterLinkInternal(label = "Backends", route = &BACKENDS_ROUTE)
        }
    };

    let grid = ui! {
        view(style = grid_style) {
            brand
            project_column
            resources_column
        }
    };

    let bottom = ui! {
        view(style = bottom_style) {
            text(style = copy_style) { "© Idealyst 2026" }
        }
    };

    ui! {
        view(style = footer_style) {
            grid
            bottom
        }
    }
}

/// Mobile-style top bar — menu button on the left, current
/// screen's title leading-aligned. Lives in the navigator's
/// `top` slot, so it mounts ONCE at navigator init and survives
/// every screen swap.
///
/// Visibility: the bar is rendered unconditionally, but its
/// `MobileHeader` stylesheet has a `size` variant — at wide
/// viewports it's `display: none` via the `min-height: 0` trick
/// (height 0, padding 0). Below the sidebar-collapse breakpoint
/// it expands to the 56-px bar shown in the screenshots.
///
/// Reactive title: reads `slot.active_route` directly — no
/// thread-local mirror needed since `SlotProps` already carries
/// the SDK's authoritative signal. Reading inside the
/// `text(closure)` source subscribes the bar's reactive scope to
/// every navigation.
///
/// Menu dispatch: reads `slot.open_drawer` (pre-bound by the SDK
/// to dispatch `DrawerCmd::Open`). No more thread-local
/// `OPEN_FN` round-trip.
pub fn mobile_header(slot: SlotProps) -> Element {
    // Keyed on the sidebar-collapse breakpoint (not the content-tighten
    // breakpoint `responsive_style` uses): the hamburger is the only way
    // to open the drawer once the sidebar overlays itself, so it must
    // appear at exactly the width where the sidebar collapses.
    let header_style = crate::responsive::collapse_responsive_style(MobileHeader::sheet());
    let title_wrap_style = MobileHeaderTitleWrap();
    let title_style = move || StyleApplication::new(MobileHeaderTitle::sheet());
    let button_style = move || StyleApplication::new(MobileHeaderButton::sheet());

    // --- menu button (leading) ---
    let menu_icon: Element = ui! { text(style = button_style) { "\u{2630}" } };
    let open_drawer = slot.open_drawer.clone();
    let menu_button = pressable(vec![menu_icon], move || open_drawer())
        .into_element();

    // --- title (center) — reactive on the navigator's active_route ---
    let active_route = slot.active_route;
    let title_source = move || label_for_route(active_route.get()).to_string();
    let title_view: Element = text(title_source).with_style(title_style).into_element();
    let title_node = ui! {
        view(style = title_wrap_style) { title_view }
    };

    ui! {
        view(style = header_style) {
            menu_button
            title_node
        }
    }
}

/// Render the TOC panel. The active highlight is driven by
/// `active_idx`; clicks call the page `scroll_view`'s
/// [`ScrollViewHandle::scroll_to`] (via `scroll_ref`) to scroll the
/// body to the matching section. `viewport_ref` / `content_ref`
/// supply the body geometry the click target math needs.
fn render_toc(
    entries: Vec<TocEntry>,
    active_idx: Signal<Option<usize>>,
    scroll_y: Signal<f32>,
    scroll_ref: Ref<ScrollViewHandle>,
    viewport_ref: Ref<ViewHandle>,
    content_ref: Ref<ViewHandle>,
) -> Element {
    let panel_style = TocPanel();
    let header_style = TocHeader();

    let mut children: Vec<Element> = Vec::with_capacity(entries.len() + 1);
    children.push(ui! {
        text(style = header_style) { "On this page" }
    });
    for (i, entry) in entries.iter().enumerate() {
        children.push(toc_link(i, *entry, active_idx, scroll_y, scroll_ref, viewport_ref, content_ref));
    }

    ui! { view(style = panel_style) { children } }
}

/// One TOC link. The style closure reads `active_idx` reactively
/// to flip the `active` variant. Click computes the target Y in
/// the page `scroll_view`'s scroll coords and dispatches via the
/// bound [`ScrollViewHandle`] (`scroll_ref`).
fn toc_link(
    index: usize,
    entry: TocEntry,
    active_idx: Signal<Option<usize>>,
    scroll_y: Signal<f32>,
    scroll_ref: Ref<ScrollViewHandle>,
    viewport_ref: Ref<ViewHandle>,
    content_ref: Ref<ViewHandle>,
) -> Element {
    let label_text = entry.label.to_string();
    let style = move || {
        let variant = if active_idx.get() == Some(index) { "on" } else { "off" };
        StyleApplication::new(TocLink::sheet()).with("active", variant.to_string())
    };
    let children: Vec<Element> = vec![ui! { text(style = style) { label_text } }];

    let bound = runtime_core::pressable(children, move || {
        // Pin the clicked entry as active right away — the spy
        // re-fires on the impending scroll and *should* land on
        // the same entry, but if rounding nudges it 1 px off
        // we'd briefly highlight a neighbour and then snap back.
        // Explicit set + the spy's `BAND_COMPARE_TOLERANCE` keep
        // the click-and-stay UX rock-solid.
        active_idx.set(Some(index));

        // `absolute_frame()` returns the section's position in
        // *window* coordinates. To compute the target scrollTop
        // for the body, translate into body-relative coordinates
        // first by subtracting `body_viewport_top` — otherwise
        // the offset added by any chrome above the body (mobile
        // header at narrow widths) makes us over-scroll and clip
        // the section's top.
        let section_window_y = entry
            .handle
            .with(|h| h.absolute_frame())
            .flatten()
            .map(|r| r.y)
            .unwrap_or(0.0);
        let current_scroll = scroll_y.get();
        let dims = read_body_scroll_dims(current_scroll, viewport_ref, content_ref);
        let section_body_y = section_window_y - dims.body_viewport_top;
        let target = (current_scroll + section_body_y - dims.band_y).max(0.0);
        scroll_ref.with(|h| h.scroll_to(0.0, target));
    });
    runtime_core::IntoElement::into_element(bound)
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
/// Snapshot of the scroll-surface metrics needed for scroll-spy
/// math, reconstructed app-side from `ViewHandle::absolute_frame()`
/// (the website owns its scroll now — there's no navigator scroll
/// context to read). `ScrollViewHandle` is write-only, so the
/// viewport height, content height, and viewport top all come from
/// two bound `ViewHandle`s:
///   - `viewport_ref` → the scroll viewport wrapper: `.y` is the
///     body's top edge, `.height` is the visible (client) height.
///   - `content_ref`  → the content column: `.height` is the total
///     scrollable content height.
/// No `web_sys`, no `cfg(target_arch)` — `absolute_frame()` is a
/// Backend-trait primitive, uniform across every backend.
///
/// Returns sane defaults when either frame isn't measured yet
/// (`absolute_frame()` is `None` on the very first build pass before
/// layout has run): a 160 px fixed band, "never near bottom", and a
/// zero viewport top. These exist so the spy effect's first read is
/// well-defined; a real page re-runs the effect once layout reports
/// frames.
struct ScrollDims {
    band_y: f32,
    near_bottom: bool,
    /// Body's top edge in window coordinates. Subtract from a
    /// section's `absolute_frame().y` (also window-relative) to
    /// get the section's body-relative Y.
    body_viewport_top: f32,
}

fn read_body_scroll_dims(
    current_scroll: f32,
    viewport_ref: Ref<ViewHandle>,
    content_ref: Ref<ViewHandle>,
) -> ScrollDims {
    let viewport = viewport_ref.with(|h| h.absolute_frame()).flatten();
    let Some(viewport) = viewport else {
        return ScrollDims { band_y: 160.0, near_bottom: false, body_viewport_top: 0.0 };
    };
    let height = viewport.height;
    let viewport_top = viewport.y;
    // Content height = the scrolled column's frame height. Falls back
    // to the viewport height (→ "never near bottom") until measured.
    let scroll_h = content_ref
        .with(|h| h.absolute_frame())
        .flatten()
        .map(|r| r.height)
        .unwrap_or(height);
    let band_y = if height > 0.0 {
        (height * ACTIVE_BAND_FRACTION).max(80.0)
    } else {
        160.0
    };
    let near_bottom = scroll_h > 0.0
        && current_scroll + height >= scroll_h - END_OF_SCROLL_EPSILON;
    ScrollDims { band_y, near_bottom, body_viewport_top: viewport_top }
}

fn install_scroll_spy(
    entries: Vec<TocEntry>,
    scroll_y: Signal<f32>,
    active_idx: Signal<Option<usize>>,
    viewport_ref: Ref<ViewHandle>,
    content_ref: Ref<ViewHandle>,
) {
    effect!({
        // Subscribe to scroll position. The `get()` registers this
        // effect as a dependent; subsequent `set()` calls from the
        // `on_scroll` callback retrigger.
        let current_scroll = scroll_y.get();

        // Read the body's live dimensions for two viewport-relative
        // calculations: (a) place the active band at
        // `ACTIVE_BAND_FRACTION` of the body height (so short
        // sections still catch a moment of activity instead of
        // requiring `clientHeight * (1 - fraction)` of scroll to
        // cross a fixed band), and (b) detect "at the bottom of
        // scroll" so the last entry force-selects even if it's
        // shorter than the band-to-bottom gap.
        let dims = read_body_scroll_dims(current_scroll, viewport_ref, content_ref);

        if dims.near_bottom && !entries.is_empty() {
            let last = Some(entries.len() - 1);
            if active_idx.get() != last {
                active_idx.set(last);
            }
            return;
        }

        // Both `band_y` and the section rect are in the same
        // coordinate space (body-relative) once we subtract the
        // body's viewport top from the window-relative
        // `absolute_frame()` result. The `+ BAND_COMPARE_TOLERANCE`
        // covers rounding error from the click-scroll round-trip
        // — a section the click placed at exactly the band can
        // measure slightly below it on the spy's next read.
        let mut best: Option<usize> = None;
        let mut best_top: f32 = f32::NEG_INFINITY;
        for (i, entry) in entries.iter().enumerate() {
            let Some(rect) = entry.handle.with(|h| h.absolute_frame()).flatten() else {
                continue;
            };
            let section_body_y = rect.y - dims.body_viewport_top;
            if section_body_y <= dims.band_y + BAND_COMPARE_TOLERANCE
                && section_body_y > best_top
            {
                best_top = section_body_y;
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
/// during `init`; the returned Element's reactive scope survives
/// for the navigator's entire lifetime.
///
/// `is_dark` is an app-level signal lifted out of `app()` so the
/// theme-toggle's state survives navigation (signals scoped to a
/// screen would reset on every push). Toggling it both flips the
/// signal AND swaps the installed idea-ui theme via
/// `set_idea_theme(...)`.
pub fn sidebar(slot: SlotProps, is_dark: Signal<bool>) -> Element {
    let body_style = SidebarBody();
    let header_style = SidebarHeader();

    let brand_text_children: Vec<Element> = vec![
        ui! { Typography(content = "Idealyst".to_string(), kind = idea_ui::typography_kind::H3) },
        ui! {
            Typography(
                content = "One codebase, native everywhere.".to_string(),
                muted = true,
            )
        },
    ];
    let brand_row = ui! {
        link(route = &HOME_ROUTE, params = ()) {
            view(style = SidebarBrandRow()) {
                icon(LIGHT_LOGO)
                    .with_style(SidebarLogo())
                    .animate(StrokeAnimation::new(1400, Easing::EaseInOut))
                view(style = SidebarBrandText()) { brand_text_children }
            }
        }
    };
    let header_children: Vec<Element> = vec![brand_row];

    let active_route = slot.active_route;

    // The whole sidebar is one `ui!` tree. Nested `for` loops over the
    // static route table emit flat siblings (no per-iteration wrapper
    // View); the section title is a `.then(...)` so title-less sections
    // (e.g. Home) add nothing; and `Spacer` / `ThemeToggle` sit inline
    // rather than being pushed onto a vector afterwards.
    ui! {
        view(style = body_style) {
            view(style = header_style) { header_children }
            for section in SECTIONS {
                (!section.title.is_empty()).then(|| ui! {
                    text(style = SidebarSection()) { section.title.to_string() }
                })
                for entry in section.entries {
                    SidebarLink(
                        route = entry.route,
                        label = entry.label,
                        active_route = active_route,
                    )
                }
            }
            // `Spacer` grows to fill leftover vertical space, pinning the
            // footer to the bottom when nav content is short; when it
            // overflows, the outer `.ui-nav-drawer-sidebar` div scrolls.
            Spacer()
            ThemeToggle(is_dark = is_dark)
        }
    }
}

/// Dark/light theme switch pinned to the bottom of the sidebar.
/// Flips `is_dark` AND swaps the installed `IdeaTheme` so every
/// component re-renders against the new token set.
///
/// Promoted from the snake_case `theme_toggle` helper because it has
/// props (CLAUDE.md §9.5); the wrapper `SidebarFooter` style is now
/// computed inside the component instead of being passed in.
#[derive(Default)]
pub struct ThemeToggleProps {
    pub is_dark: Signal<bool>,
}

#[component]
pub fn ThemeToggle(props: ThemeToggleProps) -> Element {
    let is_dark = props.is_dark;
    let footer_style = SidebarFooter();
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |dark| {
        is_dark.set(dark);
        if dark {
            set_idea_theme(dark_theme());
        } else {
            set_idea_theme(light_theme());
        }
    });
    ui! {
        view(style = footer_style) {
            Switch(
                label = Some("Dark mode".to_string()),
                value = is_dark,
                on_change = on_change,
            )
        }
    }
}

/// One sidebar nav link. Routes are matched by name; each emits a
/// `link` to the corresponding `Route<()>` constant, which the
/// drawer SDK rewrites to a `Select` command. The style closure
/// reads `active_route` so the active variant flips reactively
/// without rebuilding the link.
///
/// Promoted from the snake_case `nav_link` helper because it has
/// props and is called from a `for` loop (CLAUDE.md §9.5). The name
/// is `SidebarLink`, not `NavLink`, because `NavLink` is a stylesheet
/// in `styles.rs` — promoting the helper to `NavLink` would collide
/// with the `pub type NavLink = NavLinkProps` alias `#[component]`
/// emits.
#[derive(Default)]
pub struct SidebarLinkProps {
    pub route: Option<&'static Route<()>>,
    pub label: &'static str,
    pub active_route: Signal<&'static str>,
}

#[component]
pub fn SidebarLink(props: SidebarLinkProps) -> Element {
    let route = props
        .route
        .expect("SidebarLink requires a `route` prop");
    let label_text = props.label.to_string();
    let active_route = props.active_route;
    let route_for_match: &'static str = route.name();
    // The `active` axis is derived reactively from `active_route`: the
    // `derived(...)` closure reads the signal, so the style effect
    // re-resolves (flipping On/Off) whenever the route changes — no
    // manual `StyleApplication::with("active", …)` string plumbing.
    let style = NavLink().active(derived(move || {
        if active_route.get() == route_for_match {
            NavLinkActive::On
        } else {
            NavLinkActive::Off
        }
    }));
    ui! {
        link(route = route, params = ()) {
            text(style = style) { label_text }
        }
    }
}
