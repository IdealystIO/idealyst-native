/// A three-screen swap navigator with an author tab bar. `swap` has no
/// push/pop depth — selecting a screen SWAPS the one visible screen — and
/// the "tab bar" is just ordinary author layout wrapped around the
/// navigator's single `{ nav.outlet }` (the analog of react-router's
/// `<Outlet/>`). The `use swap_navigator::{…}` line matters: `SwapBuilder`
/// (`.screen`/`.layout`/`.bind`) is an extension trait on the navigator
/// builder — it must be in scope or the builder calls fail to resolve.
///
/// In the layout: splat `{ nav.outlet }` BARE (it ships its own
/// `flex: 1 1 0; min-height: 0` fill rules; a styled wrapper replaces them
/// and collapses grow-based screens) and keep the tab bar in a non-growing
/// sibling slot so only the outlet grows. Each tab button calls
/// `nav.on_select("<route name>")` to switch; read `nav.active_route` to
/// highlight the live tab.
pub fn swap_three_screens_tab_bar() -> ::runtime_core::Element {
    use std::rc::Rc;

    use ::runtime_core::{
        cached_stylesheet, ui, Element, FlexDirection, Length, Ref, Route, Screen,
        StyleApplication, StyleRules, StyleSheet,
    };
    // The builder methods live on the `SwapBuilder` extension trait — it
    // MUST be imported alongside the navigator type or `.screen`/`.layout`/
    // `.bind` won't resolve.
    use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};

    // Three co-equal routes. All unit-params (`Route<()>`), so a bare-name
    // `on_select` can build each one's URL with no path segments.
    const HOME: Route<()> = Route::<()>::new("home", "/");
    const SEARCH: Route<()> = Route::<()>::new("search", "/search");
    const PROFILE: Route<()> = Route::<()>::new("profile", "/profile");

    // Demo-local styles. One shared empty base sheet via `cached_stylesheet`
    // (a per-file `thread_local!` would burn an Android/bionic pthread TLS
    // key per sheet).
    fn base() -> Rc<StyleSheet> {
        static KEY: u8 = 0;
        cached_stylesheet(&KEY as *const u8 as usize, || {
            Rc::new(StyleSheet::r#static(StyleRules::default()))
        })
    }
    // The shell fills the navigator container and stacks outlet + tab bar.
    fn shell_style() -> StyleApplication {
        StyleApplication::new(base()).with_computed("swap_recipe_shell", || StyleRules {
            flex_direction: Some(FlexDirection::Column),
            width: Some(Length::Percent(100.0).into()),
            height: Some(Length::Percent(100.0).into()),
            ..Default::default()
        })
    }
    // The tab bar must sit in a NON-growing slot — only the outlet grows.
    fn tab_bar_style() -> StyleApplication {
        StyleApplication::new(base()).with_computed("swap_recipe_tab_bar", || StyleRules {
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

    // Filled by the framework at mount; use it to switch screens with
    // typed params imperatively (chrome uses `nav.on_select` instead).
    let nav: Ref<SwapHandle> = Ref::new();

    let builder = SwapNavigator::new(&HOME)
        .screen(HOME, |_| {
            let body: Element = ui! { view { text { "Home" } } };
            Screen::new(body)
        })
        .screen(SEARCH, |_| {
            let body: Element = ui! { view { text { "Search" } } };
            Screen::new(body)
        })
        .screen(PROFILE, |_| {
            let body: Element = ui! { view { text { "Profile" } } };
            Screen::new(body)
        })
        .layout(|nav| {
            // `on_select` is `Rc<dyn Fn(&'static str)>`; clone one per tab.
            let (home, search, profile) =
                (nav.on_select.clone(), nav.on_select.clone(), nav.on_select.clone());
            ui! {
                view(style = shell_style) {
                    // Outlet grows to fill the space above the bar. Splat
                    // it BARE — its built-in fill rules make the active
                    // screen absorb the remaining height. (Braces are only
                    // needed when the splat follows a PascalCase component
                    // call, which would otherwise swallow it as children.)
                    nav.outlet
                    // Author "tab bar": one Select button per screen, in a
                    // non-growing sibling slot.
                    view(style = tab_bar_style) {
                        button(label = "Home", on_click = move || home("home"))
                        button(label = "Search", on_click = move || search("search"))
                        button(label = "Profile", on_click = move || profile("profile"))
                    }
                }
            }
        });

    ui! { builder.bind(nav) }
}
