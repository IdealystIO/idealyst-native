//! Idealyst documentation site.
//!
//! Single, platform-agnostic crate: [`app`] returns a `Primitive`
//! tree that runs unchanged on every backend the framework supports.
//! The per-platform glue (wasm-bindgen entry for web, etc.) is the
//! responsibility of the `idealyst` CLI, which materializes those
//! wrappers into `target/idealyst/<platform>/` at build time.
//!
//! The `web` module's `wasm-bindgen` `start()` is transitional — it
//! lives here until `idealyst dev` / `idealyst build web` generates
//! that wrapper crate.
//!
//! # Module ordering
//!
//! `mod shell;` is declared with `#[macro_use]` and BEFORE `mod
//! pages;`. The `#[component]` attribute on `PageHeader` / `Section`
//! / `CodeBlock` / `SectionWithCode` generates local `macro_rules!`;
//! `#[macro_use]` lifts those to crate-root scope so page modules
//! can invoke them via the `ui!` DSL.

use framework_core::{
    component, signal, ui, DrawerHandle, DrawerNavigator, HeaderStyle, Primitive, Ref, Screen,
    Signal,
};
#[allow(unused_imports)]
use idea_ui::{
    body, heading, idea_header, install_idea_theme, light_theme, stack, IdeaTheme,
};

pub mod meta;
pub mod registry;
mod routes;
mod styles;

// Order matters: `#[macro_use]` lifts the `#[component]`-generated
// invocation macros (from `shell` and `components`) to crate-root
// scope, where the `ui!` DSL inside every page module can find them.
#[macro_use]
mod shell;
#[macro_use]
mod components;
mod pages;

// Declared AFTER the `#[macro_use]` mods above so it can see the
// shell + idea-ui invocation macros that the `docs!` emission
// references.
#[cfg(test)]
mod macro_test;

#[cfg(target_arch = "wasm32")]
mod web;

use routes::{
    BACKENDS_ROUTE, CLI_ROUTE, COMPONENTS_ROUTE, DEV_TOOLS_ROUTE, ICONS_ROUTE, LISTS_ROUTE,
    MACROS_ROUTE, NAVIGATION_ROUTE, OVERVIEW_ROUTE, PLATFORMS_ROUTE, PRIMITIVES_ROUTE,
    QUICKSTART_ROUTE, REACTIVITY_ROUTE, REFS_ROUTE, ROBOT_ROUTE, STYLES_ROUTE, UI_DSL_ROUTE,
    WRITING_A_BACKEND_ROUTE,
};
use shell::content_builder;
#[cfg(target_arch = "wasm32")]
use shell::web_layout;

#[component]
pub fn app() -> Primitive {
    install_idea_theme(light_theme());

    // Theme flag the sidebar's dark-mode toggle drives. Owned at
    // the root so it survives across screen pushes.
    let is_dark: Signal<bool> = signal!(false);
    let drawer: Ref<DrawerHandle> = Ref::new();

    // TEMP MINIMAL DEBUG — strip everything that might be the OOB
    // trigger: no idea_header callback (no navigator-color Effects),
    // no sidebar content closure, one screen with a tiny body, no
    // layout. If this still crashes wasm with the same
    // `attach_style_reactive` OOB, the bug is in something deeper
    // than the docs site.
    let builder = DrawerNavigator::new(&OVERVIEW_ROUTE)
        .header(idea_header(|t| HeaderStyle {
            background: Some(t.colors().surface.value().clone()),
            title: Some(t.colors().text.value().clone()),
            tint: Some(t.colors().text.value().clone()),
            body_background: Some(t.colors().background.value().clone()),
        }))
        .screen(OVERVIEW_ROUTE, |_| {
            Screen::new(ui! {
                Text { "minimal".to_string() }
            })
            .title("Overview")
        })
        // STEP 7: add one sidebar_section, which calls nav_link.
        // nav_link creates a reactive-style closure (the active
        // route highlight). If THIS triggers the OOB, the bug is
        // in `attach_style_reactive` for a `Text(style = move ||
        // ...)` invocation.
        .content({
            let _ = is_dark;
            move |props: framework_core::DrawerContentProps| {
                use framework_core::IntoPrimitive;
                let sidebar_style = crate::styles::Sidebar();
                let header_style = crate::styles::SidebarHeader();
                let header_children: Vec<framework_core::Primitive> = vec![
                    ui! { Heading(content = "Idealyst".to_string(), kind = idea_ui::HeadingKind::H2) },
                    ui! { Body(content = "Cross-platform Rust framework".to_string(), tone = idea_ui::BodyTone::Muted) },
                ];
                let active_route = props.active_route;
                // STEP 7f: add the inner View(SidebarSection) {
                // Text(SidebarSectionLabel) ... Link ... } wrapping
                // — matches the failing structure from earlier.
                use framework_core::StyleApplication;
                use crate::styles::{NavLink, SidebarSection, SidebarSectionLabel};
                use crate::routes::OVERVIEW_ROUTE;
                let route_for_match: &str = "overview";
                let style = move || {
                    let variant = if active_route.get() == route_for_match {
                        "on"
                    } else {
                        "off"
                    };
                    StyleApplication::new(NavLink::sheet()).with("active", variant.to_string())
                };
                let section_style = SidebarSection();
                let label_style = SidebarSectionLabel();
                let _ = label_style;
                let _ = header_style;
                let _ = header_children;
                let _ = sidebar_style;
                let _ = section_style;
                let _ = OVERVIEW_ROUTE;
                // STEP 7k: same shape minus Link. If this crashes,
                // ScrollView > View > Text(reactive) is enough.
                // If it works, Link is essential.
                ui! {
                    ScrollView {
                        View {
                            Text(style = style) { "Overview".to_string() }
                        }
                    }
                }
            }
        })
        .bind(drawer);

    #[cfg(target_arch = "wasm32")]
    let builder = builder.layout(web_layout());

    ui! { builder }
}
