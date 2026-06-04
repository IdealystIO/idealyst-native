//! `stack-demo` — bare-minimum stack-navigator example.
//!
//! The whole app is one `Navigator` with four screens. The root
//! ("Home") has buttons that push the other three; each non-root
//! screen has an in-content `[ Back ]` button that calls
//! `nav.pop()`.
//!
//! Native stacks (iOS UINavigationController, Android FragmentManager,
//! browser history) ship their own back affordance. The terminal
//! backend renders no navigator chrome (per the framework's
//! terminal-minimalism convention), so the in-content back button
//! is what makes the demo portable across every target.

use runtime_core::{
    component, signal, text, ui, IntoElement, Element, Ref, Route, Screen, Signal,
};
use idea_ui::{Typography, Card, install_idea_theme, light_theme, Stack, StackGap, StackPadding};
use stack_navigator::{Navigator, StackBuilder, StackHandle, StackScreenExt};

// ---------------------------------------------------------------------------
// Per-target SDK-handler registration hook. The CLI-generated wrapper
// crates call `register_extensions(&mut backend)` once before mount so
// the navigator SDK can install its handler factory on the backend.
// ---------------------------------------------------------------------------

// Navigators + externals self-register at backend construction via
// `inventory::submit!` inside their SDK crates — the app just uses them, no
// per-platform registration. The hook remains for app-local externals; the CLI
// bootstrap still calls it. See [[project_inventory_self_registration]].
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

// Recorder-side registration for the runtime-server sidecar. Distinct fn
// name (not an overload of `register_extensions`) so it never collides
// with the host target's per-backend overload when both compile in the
// sidecar build. Gated by `sidecar` (set only by the generated sidecar
// wrapper) so device/web builds never pull `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(backend: &mut dev_server::WireRecordingBackend) {
    stack_navigator::recording::register(backend);
}

// ---------------------------------------------------------------------------
// Routes. One per screen. The `Route<()>` constants get reused by
// the screen builder AND by every `nav.push(&ROUTE, ())` call site —
// no string keys in author code.
// ---------------------------------------------------------------------------

const HOME: Route<()> = Route::<()>::new("home", "/");
const ABOUT: Route<()> = Route::<()>::new("about", "/about");
const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");
const COUNTER: Route<()> = Route::<()>::new("counter", "/counter");

// ---------------------------------------------------------------------------
// App entry.
// ---------------------------------------------------------------------------

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let nav: Ref<StackHandle> = Ref::new();
    // App-level state — survives navigation because it lives in the
    // root scope, not a per-screen one. Push/pop only releases per-
    // screen scopes; the Counter screen will see whichever value the
    // counter held when it last unmounted.
    let count: Signal<i32> = signal!(0);

    let builder = Navigator::new(&HOME)
        .screen(HOME, move |_| Screen::new(home_page(nav)).title("Home"))
        .screen(ABOUT, move |_| Screen::new(about_page(nav)).title("About"))
        .screen(SETTINGS, move |_| Screen::new(settings_page(nav)).title("Settings"))
        .screen(COUNTER, move |_| Screen::new(counter_page(nav, count)).title("Counter"));

    ui! { builder.bind(nav) }
}

// ---------------------------------------------------------------------------
// Pages. Each is a plain function returning a `Element`. Children
// are built into a `Vec<Element>` first to keep the `ui!` body
// unambiguous — `Typography(...)` followed by a `{ expr }` brace-block in
// the same scope would otherwise be parsed as `Typography(...) { children }`
// and the macro errors because `TypographyProps` has no `children` field.
// ---------------------------------------------------------------------------

fn home_page(nav: Ref<StackHandle>) -> Element {
    let go_about = move || nav.get().map(|h| h.push(&ABOUT, ())).unwrap_or_default();
    let go_settings = move || nav.get().map(|h| h.push(&SETTINGS, ())).unwrap_or_default();
    let go_counter = move || nav.get().map(|h| h.push(&COUNTER, ())).unwrap_or_default();

    let children: Vec<Element> = vec![
        ui! { Typography(content = "Stack demo".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "Tap a button to push a detail screen onto the stack. Each detail screen has a Back button that pops.".to_string(),
                muted = true,
            )
        },
        ui! { button(label = "Open About".to_string(), on_click = go_about) },
        ui! { button(label = "Open Settings".to_string(), on_click = go_settings) },
        ui! { button(label = "Open Counter".to_string(), on_click = go_counter) },
    ];

    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Lg) { children }
    }
}

fn about_page(nav: Ref<StackHandle>) -> Element {
    let children: Vec<Element> = vec![
        ui! { Typography(content = "About".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "This screen was pushed onto the stack. Press Back to pop it.".to_string(),
                muted = true,
            )
        },
        back_button(nav),
    ];
    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Lg) { children }
    }
}

fn settings_page(nav: Ref<StackHandle>) -> Element {
    let card_children: Vec<Element> = vec![ui! {
        Typography(
            content = "Imagine real settings here. The card is just to show that pages can carry their own layout.".to_string(),
            muted = true,
        )
    }];
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Settings".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! { Card { card_children } },
        back_button(nav),
    ];
    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Lg) { children }
    }
}

fn counter_page(nav: Ref<StackHandle>, count: Signal<i32>) -> Element {
    let increment = move || count.update(|n| *n += 1);
    let decrement = move || count.update(|n| *n -= 1);
    // Reactive label — `text(closure)` returns a `Bound<TextHandle>`
    // primitive whose content re-resolves when `count` changes,
    // unlike `Typography(content = String)` which captures a string at
    // build time. Keeps the demo wired correctly for the canonical
    // "stateful counter survives push/pop" pattern.
    let label = text(move || format!("Count: {}", count.get())).into_element();

    let children: Vec<Element> = vec![
        ui! { Typography(content = "Counter".to_string(), kind = idea_ui::typography_kind::H1) },
        label,
        ui! { button(label = "+".to_string(), on_click = increment) },
        ui! { button(label = "-".to_string(), on_click = decrement) },
        back_button(nav),
    ];
    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Lg) { children }
    }
}

// Shared `Back` button — same shape on every detail page so the
// affordance reads consistently. Pops the topmost screen; native
// back chrome (iOS chevron, Android system back, browser back) does
// the same thing, the button is the portable in-content equivalent
// for terminal.
fn back_button(nav: Ref<StackHandle>) -> Element {
    let on_back = move || nav.get().map(|h| h.pop()).unwrap_or_default();
    ui! { button(label = "Back".to_string(), on_click = on_back) }
}

