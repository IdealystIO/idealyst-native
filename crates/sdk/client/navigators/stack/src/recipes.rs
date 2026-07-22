//! Compile-checked usage **recipes** for the stack navigator — the canonical
//! "how do I wire a two-screen stack" example the MCP catalog serves.
//!
//! Why this exists: the arena's nav-notes feedback report (run-0, 2026-07-21)
//! showed an agent reconstructing exactly this skeleton — typed `Route`
//! consts, the `StackBuilder`/`StackScreenExt` import pair, a `.layout(...)`
//! shell around the outlet — by grepping framework source, because
//! `list_recipes` had no navigation recipe and the guide's example didn't
//! compile. This recipe compiles against the live builder API (a signature
//! change that isn't reflected here is a compile error whenever the catalog
//! is built), so the served example can't rot.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every
//! production build) it expands to nothing. Recipes are self-contained —
//! imports live inside the fn — so the captured source reads as a complete,
//! copy-pasteable example.

use runtime_core::recipe;

recipe!(
    StackNavigator,
    /// A list → detail stack: two screens, a typed `Route` param that
    /// round-trips through the URL (`/note/:slug`), and an author
    /// `.layout(...)` shell around the outlet. The `use stack_navigator::{…}`
    /// line matters: `StackBuilder` (`.screen`/`.layout`/`.bind`) and
    /// `StackScreenExt` (`.title`) are extension traits — both must be in
    /// scope or the builder calls fail to resolve. In the layout, splat
    /// `{ nav.outlet }` BARE (it ships its own `flex: 1 1 0; min-height: 0`
    /// fill rules; a styled wrapper replaces them and collapses grow-based
    /// screens) and keep chrome in a non-growing sibling slot.
    pub fn stack_two_screens() -> ::runtime_core::Element {
        use std::collections::HashMap;
        use std::rc::Rc;

        use ::runtime_core::{
            cached_stylesheet, rx, ui, Element, FlexDirection, Length, Ref, Route, RouteParams,
            Screen, StyleApplication, StyleRules, StyleSheet,
        };
        // Both extension traits MUST be imported alongside the builder.
        use stack_navigator::{
            header_state, StackBuilder, StackContext, StackHandle, StackNavigator, StackScreenExt,
        };

        // Typed params for the detail route: `to_path` fills `/note/:slug` on
        // push; `from_segments` parses it back out on a web cold load of
        // /note/<slug> (and on pop re-mounts under Rebuild retention).
        #[derive(Clone)]
        struct NoteId {
            slug: String,
        }
        impl RouteParams for NoteId {
            fn to_path(&self, pattern: &str) -> String {
                pattern.replace(":slug", &self.slug)
            }
            fn from_segments(segs: &HashMap<String, String>) -> Option<Self> {
                segs.get("slug").map(|slug| NoteId { slug: slug.clone() })
            }
        }

        const LIST: Route<()> = Route::<()>::new("list", "/");
        const DETAIL: Route<NoteId> = Route::<NoteId>::new("detail", "/note/:slug");

        // Demo-local styles. One shared empty base sheet via
        // `cached_stylesheet` (a per-file `thread_local!` would burn an
        // Android/bionic pthread TLS key per sheet).
        fn base() -> Rc<StyleSheet> {
            static KEY: u8 = 0;
            cached_stylesheet(&KEY as *const u8 as usize, || {
                Rc::new(StyleSheet::r#static(StyleRules::default()))
            })
        }
        // The shell fills the navigator container and stacks header + outlet.
        fn shell_style() -> StyleApplication {
            StyleApplication::new(base()).with_computed("stack_recipe_shell", || StyleRules {
                flex_direction: Some(FlexDirection::Column),
                width: Some(Length::Percent(100.0).into()),
                height: Some(Length::Percent(100.0).into()),
                ..Default::default()
            })
        }
        // Chrome must sit in a NON-growing slot — only the outlet grows. (A
        // chrome component that roots in a growing surface, like idea-ui-nav's
        // StackHeader, would otherwise split the viewport with the outlet.)
        fn header_slot_style() -> StyleApplication {
            StyleApplication::new(base()).with_computed("stack_recipe_header", || StyleRules {
                flex_direction: Some(FlexDirection::Row),
                flex_grow: Some(0.0.into()),
                flex_shrink: Some(0.0.into()),
                gap: Some(Length::Px(12.0).into()),
                padding_top: Some(Length::Px(12.0).into()),
                padding_bottom: Some(Length::Px(12.0).into()),
                padding_left: Some(Length::Px(12.0).into()),
                padding_right: Some(Length::Px(12.0).into()),
                ..Default::default()
            })
        }

        // Filled by the framework at mount; screen closures capture it and
        // read it when a row is actually pressed.
        let nav: Ref<StackHandle> = Ref::new();

        let builder = StackNavigator::new(&LIST)
            .screen(LIST, move |_| {
                let open = move |slug: &'static str| {
                    let params = NoteId { slug: slug.to_string() };
                    nav.get().map(|h| h.push(&DETAIL, params)).unwrap_or_default();
                };
                let body: Element = ui! {
                    view {
                        text { "Notes" }
                        button(label = "Groceries", on_click = move || open("groceries"))
                        button(label = "Ideas", on_click = move || open("ideas"))
                    }
                };
                Screen::new(body).title("Notes")
            })
            .screen(DETAIL, |params: NoteId| {
                // Runs with the typed param — from a push OR a cold URL load.
                let title = format!("Note: {}", params.slug);
                let heading = title.clone();
                let body: Element = ui! {
                    view {
                        text { "{heading}" }
                        text { "Built from the typed :slug segment of the URL." }
                    }
                };
                Screen::new(body).title(title)
            })
            .layout(|nav: StackContext| {
                // The title derives from the active screen's `.title(...)`
                // slot — the same data a native bar renders on mobile.
                let screen_chrome = nav.screen_chrome;
                let title =
                    rx!(header_state(&screen_chrome).map(|s| s.title).unwrap_or_default());
                let on_back = nav.pop.clone();
                ui! {
                    view(style = shell_style) {
                        // Chrome lives in its own non-growing slot. Wrapping
                        // the CHROME (never the outlet) also keeps the
                        // `{ nav.outlet }` splat below a sibling — a braced
                        // splat directly after a PascalCase component call
                        // would parse as that component's children block.
                        view(style = header_slot_style) {
                            // `pop` is a no-op at the root; gate the button on
                            // `nav.can_go_back` in a real shell.
                            button(label = "Back", on_click = move || on_back())
                            text { "{title}" }
                        }
                        // Splat the outlet BARE — its built-in fill rules
                        // (`flex: 1 1 0; min-height: 0`) make the active
                        // screen absorb the remaining height.
                        { nav.outlet }
                    }
                }
            });

        ui! { builder.bind(nav) }
    }
);
