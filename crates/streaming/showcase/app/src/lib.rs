//! A shopping-style app whose screens come from a remote bundle.
//!
//! This file compiles twice: as the app (`remote-showcase`), and as the
//! bundle the app downloads (`remote-showcase-bundle`, wasm32). What runs
//! where:
//!
//! | Piece | Where | Shows |
//! |---|---|---|
//! | Tab shell (`App`, a swap navigator) | app | native code hosting remote screens |
//! | `FeedScreen` | bundle | remote screen, bundle state, app context, a sync `#[host_fn]` |
//! | `ShopNavigator` | bundle | a navigator DEFINED in the bundle: typed routes, route links, screen options in its header, an async `#[host_fn]`, writing the app's cart |
//! | `Settings` | app | native controls for the `Theme` the remote screens read live |
//! | `Card`, `Pill` | app | app components the bundle uses — imported, not bundled |
//!
//! The rules on display:
//! - `#[component(remote)]` ships in the bundle; every other `#[component]`
//!   is the app's (in the bundle, a use of `Card` is an import of the app's).
//!   Bundle-only helpers are plain functions (`product_list`, …).
//! - App state crosses as props (`cart: Signal<u32>` — the bundle writes the
//!   app's signal) and as `#[remote_context]` context (`Theme`).
//! - Native code the bundle calls is a `#[host_fn]`.
//!
//! Live reload: `cargo run --release -p stream-spike --bin stream-serve`,
//! edit a remote component below, press **Reload** in the window.

#![cfg_attr(idealyst_stream_guest, allow(dead_code, unused_imports))]

use std::rc::Rc;

use runtime_core::{component, remote_context, signal, ui, Element, ReadSignal, Signal};
use runtime_shared::primitives::navigator::{Route, RouteParams};
use runtime_shared::{Color, Length, StyleRules, Tokenized};
use stream_macros::host_fn;

// ===========================================================================
// Shared by both builds: routes, data, context, host functions, styles
// ===========================================================================

pub const FEED: Route = Route::new("feed", "/feed");
pub const SHOP: Route = Route::new("shop", "/shop");
pub const SETTINGS: Route = Route::new("settings", "/settings");
/// The shop's own routes (relative to its tab).
pub const PRODUCTS: Route = Route::new("products", "/");
pub const PRODUCT: Route<ProductId> = Route::new("product", "/item/:id");

#[derive(Clone, PartialEq, Debug)]
pub struct ProductId(pub u32);

impl RouteParams for ProductId {
    fn to_path(&self, pattern: &str) -> String {
        pattern.replace(":id", &self.0.to_string())
    }
    fn from_segments(segs: &std::collections::HashMap<String, String>) -> Option<Self> {
        segs.get("id").and_then(|s| s.parse().ok()).map(ProductId)
    }
}

pub struct Product {
    pub id: u32,
    pub name: &'static str,
    pub price_cents: u32,
    pub blurb: &'static str,
}

pub const CATALOG: &[Product] = &[
    Product { id: 1, name: "Trail Mug", price_cents: 1800, blurb: "Enamel steel, survives the campfire." },
    Product { id: 2, name: "Field Notebook", price_cents: 1200, blurb: "Waterproof paper, 96 pages." },
    Product { id: 3, name: "Pocket Lamp", price_cents: 3400, blurb: "USB-C, three brightness steps." },
    Product { id: 4, name: "Wool Socks", price_cents: 2200, blurb: "Merino blend, two pairs." },
];

pub fn product(id: u32) -> Option<&'static Product> {
    CATALOG.iter().find(|p| p.id == id)
}

fn price(cents: u32) -> String {
    format!("${}.{:02}", cents / 100, cents % 100)
}

pub struct Post {
    pub title: &'static str,
    pub body: &'static str,
}

pub const POSTS: &[Post] = &[
    Post { title: "Shipping update", body: "Orders placed today ship tomorrow morning." },
    Post { title: "New: Pocket Lamp", body: "Our brightest pocket light yet — now in the shop." },
    Post { title: "Field report", body: "Three days on the ridge with nothing but a notebook." },
];

/// The app's theme. Marked `#[remote_context]`, so remote screens read it
/// with a plain `inject` — the accent and the density live, as signals.
#[remote_context]
#[derive(Clone)]
pub struct Theme {
    pub accent: ReadSignal<String>,
    pub compact: ReadSignal<bool>,
}

/// Native code the bundle calls. Sync: answered inline.
#[host_fn]
pub fn device_name() -> String {
    format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Native code the bundle calls. Async: the app runs it on its own
/// executor (here a fake network delay) and the result reaches the
/// bundle's `spawn_then`.
#[host_fn]
pub async fn fetch_reviews(product: u32) -> Vec<String> {
    app::delay(app::REVIEW_DELAY_MS).await;
    match product {
        1 => vec!["“Survived a whole summer.” ★★★★★".into(), "“Handle gets hot.” ★★★".into()],
        3 => vec!["“So bright.” ★★★★★".into()],
        _ => vec!["No reviews yet.".into()],
    }
}

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

fn color(c: &str) -> Option<Tokenized<Color>> {
    Some(Tokenized::Literal(Color(c.to_string())))
}

fn column(gap: f32) -> StyleRules {
    StyleRules { gap: px(gap), padding_top: px(12.0), padding_left: px(12.0), padding_right: px(12.0), padding_bottom: px(12.0), ..StyleRules::default() }
}

/// A screen's scroll view: fill the space its navigator gives it. A
/// scroller has no height of its own (its content scrolls), and a swap
/// navigator's outlet doesn't size its screens — without this a screen
/// whose root is a `scroll_view` collapses to nothing.
fn screen_fill() -> StyleRules {
    StyleRules {
        flex_grow: Some(1.0.into()),
        flex_shrink: Some(1.0.into()),
        flex_basis: px(0.0),
        min_height: px(0.0),
        ..StyleRules::default()
    }
}

fn card_rules() -> StyleRules {
    StyleRules { background: color("#f3f4f6"), gap: px(6.0), ..column(6.0) }
}

/// A theme-colored style, following the app's accent live.
fn accented(theme: Option<Theme>) -> impl Fn() -> Rc<StyleRules> {
    move || {
        let accent = theme.as_ref().map(|t| t.accent.get()).unwrap_or_else(|| "#444444".into());
        Rc::new(StyleRules { color: color(&accent), ..StyleRules::default() })
    }
}

// ===========================================================================
// App components — compiled into the app; the bundle imports them
// ===========================================================================

/// A titled card. In the bundle, `Card(...)` is an import of this one.
#[component]
pub fn Card(title: String, children: Vec<Element>) -> Element {
    ui! {
        view(style = card_rules()) {
            text { "{title}" }
            children
        }
    }
}

/// A small label.
#[component]
pub fn Pill(label: String) -> Element {
    ui! {
        view(style = StyleRules { background: color("#e0e7ff"), padding_left: px(6.0), padding_right: px(6.0), ..StyleRules::default() }) {
            text { "{label}" }
        }
    }
}

// ===========================================================================
// Remote components — shipped in the bundle
// ===========================================================================

/// The feed tab. Bundle state (likes), app context (`Theme`), a sync host
/// function (`device_name`), app components (`Card`, `Pill`).
#[component(remote)]
pub fn FeedScreen() -> Element {
    let theme = runtime_world::inject::<Theme>();
    let compact = theme.as_ref().map(|t| t.compact);
    let device = device_name();
    let likes: Vec<Signal<u32>> = POSTS.iter().map(|_| signal(0)).collect();
    let heading = accented(theme.clone());
    ui! {
        scroll_view(style = screen_fill()) {
            view(style = column(10.0)) {
                text(style = heading) { "Feed — rendered by the bundle" }
                Pill(label = format!("running on {device}"))
                text { move || format!("density: {}", if compact.is_some_and(|c| c.get()) { "compact" } else { "comfortable" }) }
                for (post, like) in POSTS.iter().zip(likes) {
                    Card(title = post.title.to_string()) {
                        if !compact.is_some_and(|c| c.get()) {
                            text { post.body }
                        }
                        button(label = move || format!("♥ {}", like.get()), on_click = move || like.update(|n| n + 1))
                    }
                }
            }
        }
    }
}

/// The shop tab: a whole stack navigator defined in the bundle. Its header
/// reads `StackNav` (back, the screen's title option) and the app's cart;
/// its screens are bundle code with typed params; "Add to cart" writes the
/// app's signal.
#[component(remote)]
pub fn ShopNavigator(cart: Signal<u32>) -> Element {
    use runtime_vocabulary::builders::{button, navigator_outlet, stack_navigator, text, view};
    use runtime_vocabulary::prims::{Screen, StackNav};
    // Built with the vocabulary's builders: a navigator's layout and
    // screens are closures, which `ui!` has no tag form for.
    stack_navigator(&PRODUCTS)
        .layout(move || {
            let nav = runtime_world::inject::<StackNav>().expect("StackNav in the shop's layout");
            let (back, chrome, pop) = (nav.can_go_back, nav.screen_chrome, nav.pop.clone());
            view()
                .style(StyleRules { background: color("#eef2ff"), ..column(4.0) })
                .child(
                    view()
                        .child(button().label("‹ Back").disabled(move || !back.get()).on_press(move || pop()))
                        .child(text().content(move || {
                            let title = chrome.get().options.as_ref().and_then(|o| o.downcast_ref::<ScreenTitle>().map(|t| t.0)).unwrap_or("Shop");
                            format!("{title}   ·   cart: {}", cart.get())
                        }))
                        .build(),
                )
                .child(navigator_outlet().build())
                .build()
        })
        .screen(PRODUCTS, |()| product_list())
        .screen(PRODUCT, move |ProductId(id)| {
            let title = product(id).map_or("Unknown", |p| p.name);
            Screen::new(product_detail(id, cart)).with(ScreenTitle(title))
        })
        .build()
}

/// The shop header's title, set per screen.
pub struct ScreenTitle(pub &'static str);

/// Bundle-only: a plain function, so it ships in the bundle (a
/// `#[component]` here would be an app component, imported).
fn product_list() -> Element {
    use runtime_vocabulary::builders::link;
    let rows: Vec<Element> = CATALOG
        .iter()
        .map(|p| {
            // A route link: its typed params cross as the url and the
            // navigator rebuilds them (`ParamsFromUrl`).
            link().route(&PRODUCT, ProductId(p.id)).child(ui! { Card(title = p.name.to_string()) { text { price(p.price_cents) } } }).build()
        })
        .collect();
    ui! {
        scroll_view(style = screen_fill()) {
            view(style = column(8.0)) {
                rows
            }
        }
    }
}

fn product_detail(id: u32, cart: Signal<u32>) -> Element {
    use runtime_vocabulary::builders::{slider, toggle};
    let Some(p) = product(id) else {
        return ui! { text { "No such product" } };
    };
    let qty = signal(1.0f32);
    let gift = signal(false);
    let reviews = signal(None::<Vec<String>>);
    // An async host function: the app fetches, the bundle applies.
    runtime_core::spawn_then(fetch_reviews(id), move |r| reviews.set(Some(r)));
    let qty_slider = slider().value(move || qty.get()).range(1.0, 5.0).step(1.0).on_change(move |v| qty.set(v.round())).build();
    let gift_toggle = toggle().value(move || gift.get()).on_change(move |v| gift.set(v)).build();
    ui! {
        scroll_view(style = screen_fill()) {
            view(style = column(10.0)) {
                Card(title = p.name.to_string()) {
                    text { p.blurb }
                    text { price(p.price_cents) }
                }
                text { move || format!("Quantity: {}", qty.get() as u32) }
                qty_slider
                text { move || format!("Gift wrap: {}", if gift.get() { "yes" } else { "no" }) }
                gift_toggle
                button(label = "Add to cart", on_click = move || cart.update(|c| c + qty.get() as u32))
                text { move || match reviews.get() {
                    None => "Loading reviews…".to_string(),
                    Some(r) => format!("Reviews: {}", r.join("  ")),
                } }
            }
        }
    }
}

// ===========================================================================
// The app (native only)
// ===========================================================================

#[cfg(not(idealyst_stream_guest))]
pub use app::*;

#[cfg(not(idealyst_stream_guest))]
mod app {
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    use super::*;
    use runtime_vocabulary::builders::{button, navigator_outlet, swap_navigator, text, view};
    use runtime_vocabulary::prims::SwapNav;
    use stream_host::remote::RemoteApp;

    /// The bundle built from this file at compile time — used until a
    /// reload brings a newer one.
    pub const BUILT_IN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bundle.wasm"));
    const SERVED: &str = "http://127.0.0.1:7878/showcase.wasm";
    pub(crate) const REVIEW_DELAY_MS: i32 = 700;

    /// What remote code may call.
    pub fn host_fns() -> Vec<stream_abi::host_fn::HostFnDef> {
        vec![device_name::export(), fetch_reviews::export()]
    }

    thread_local! {
        static REMOTE: RefCell<Option<RemoteApp>> = const { RefCell::new(None) };
    }

    /// Install the built-in bundle (call before mounting `App`).
    pub fn install() {
        let remote = stream_host::remote::install_with(BUILT_IN, host_fns()).unwrap_or_else(|e| panic!("built-in bundle: {e}"));
        REMOTE.with(|r| *r.borrow_mut() = Some(remote));
    }

    fn reload(status: Signal<String>) {
        let result = stream_host::fetch::fetch(SERVED).and_then(|fetched| {
            REMOTE.with(|r| r.borrow().as_ref().expect("installed").reload(&fetched.wasm))?;
            Ok(format!("bundle v{} ({} KB)", fetched.version, fetched.wasm.len() / 1024))
        });
        status.set(result.unwrap_or_else(|e| format!("reload failed: {e}")));
    }

    /// The app: a native tab shell (swap navigator) hosting two remote
    /// screens and a native one, and the state they share.
    #[component]
    pub fn App() -> Element {
        let accent = signal("#2563eb".to_string());
        let compact = signal(false);
        let cart = signal(0u32);
        let status = signal(format!("built-in bundle ({} KB)", BUILT_IN.len() / 1024));
        runtime_world::provide(Theme { accent: accent.read_only(), compact: compact.read_only() });
        swap_navigator(&FEED)
            .layout(move || {
                let nav = runtime_world::inject::<SwapNav>().expect("SwapNav in the shell");
                let (active, on_select) = (nav.active_route, nav.on_select.clone());
                let tab = move |route: &'static str, label: &'static str| {
                    let select = on_select.clone();
                    button()
                        .label(move || if active.get() == route { format!("[{label}]") } else { label.to_string() })
                        .on_press(move || select(route))
                        .build()
                };
                view()
                    .child(
                        view()
                            .style(StyleRules { background: color("#111827"), ..column(6.0) })
                            .child(tab("feed", "Feed"))
                            .child(tab("shop", "Shop"))
                            .child(tab("settings", "Settings"))
                            .child(text().content(move || format!("cart: {}   ·   {}", cart.get(), status.get())))
                            .child(button().label("Reload remote").on_press(move || reload(status)))
                            .build(),
                    )
                    .child(navigator_outlet().build())
                    .build()
            })
            .screen(FEED, |()| ui! { FeedScreen() })
            .screen(SHOP, move |()| ui! { ShopNavigator(cart = cart) })
            .screen(SETTINGS, move |()| settings(accent, compact, cart))
            .build()
    }

    /// Native settings: they drive the `Theme` the remote screens read.
    fn settings(accent: Signal<String>, compact: Signal<bool>, cart: Signal<u32>) -> Element {
        ui! {
            scroll_view(style = screen_fill()) {
                view(style = column(10.0)) {
                    text { "Settings — native" }
                    text { move || format!("accent: {}", accent.get()) }
                    button(label = "Blue accent", on_click = move || accent.set("#2563eb".into()))
                    button(label = "Green accent", on_click = move || accent.set("#059669".into()))
                    button(label = "Orange accent", on_click = move || accent.set("#ea580c".into()))
                    button(label = move || format!("Compact feed: {}", if compact.get() { "on" } else { "off" }), on_click = move || compact.update(|c| !c))
                    Card(title = "Cart".to_string()) {
                        text { move || format!("{} item(s)", cart.get()) }
                        button(label = "Empty cart", on_click = move || cart.set(0))
                    }
                }
            }
        }
    }

    /// A timer future on the platform scheduler (the fake network delay).
    pub(crate) fn delay(ms: i32) -> impl Future<Output = ()> {
        #[derive(Default)]
        struct State {
            fired: bool,
            waker: Option<Waker>,
        }
        struct Delay {
            ms: i32,
            state: Rc<RefCell<State>>,
            timer: Option<runtime_shared::scheduling::ScheduledTask>,
        }
        impl Future for Delay {
            type Output = ();
            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                if self.state.borrow().fired {
                    return Poll::Ready(());
                }
                self.state.borrow_mut().waker = Some(cx.waker().clone());
                if self.timer.is_none() {
                    let state = self.state.clone();
                    let ms = self.ms;
                    self.timer = Some(runtime_shared::scheduling::after_ms(ms, move || {
                        let waker = {
                            let mut s = state.borrow_mut();
                            s.fired = true;
                            s.waker.take()
                        };
                        if let Some(w) = waker {
                            w.wake();
                        }
                    }));
                }
                Poll::Pending
            }
        }
        Delay { ms, state: Rc::default(), timer: None }
    }
}
