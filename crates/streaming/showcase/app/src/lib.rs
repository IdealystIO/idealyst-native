//! A shopping-style app whose screens come from a remote bundle.
//!
//! This file compiles twice: as the app (`remote-showcase`), and as the
//! bundle the app downloads (`remote-showcase-bundle`, wasm32). What runs
//! where:
//!
//! | Piece | Where | Shows |
//! |---|---|---|
//! | Tab shell (`App`, a swap navigator) | app | native code hosting remote screens |
//! | `FeedScreen` | bundle | remote screen, its own stylesheets over idea's theme tokens, bundle state, app context, a sync `#[host_fn]` |
//! | `ShopNavigator` | bundle | a navigator DEFINED in the bundle: typed routes, route links, screen options in its header, an async `#[host_fn]`, writing the app's cart |
//! | `Settings` | app | native controls: the idea theme (light/dark) every screen follows, and the `FeedPrefs` context the feed reads |
//! | idea-ui (`Card`, `Badge`, `Button`, `Typography`, `Switch`, `Slider`) | app | a component library the bundle uses — imported from the app, not bundled |
//!
//! The rules on display:
//! - `#[component(remote)]` ships in the bundle; every other `#[component]`
//!   is the app's — including a library's: in the bundle, idea-ui's `Card`
//!   is an import of the app's copy, which renders natively with the app's
//!   theme. Bundle-only helpers are plain functions (`product_list`, …).
//! - App state crosses as props (`cart: Signal<u32>` — the bundle writes the
//!   app's signal) and as context (`FeedPrefs`, which derives `Remote`).
//! - Theming is NOT context: stylesheets name the theme's tokens
//!   (`t.color.background()`), the names cross, and the app resolves them
//!   against its installed theme — so a theme swap restyles remote screens
//!   exactly as native ones.
//! - Native code the bundle calls is a `#[host_fn]`.
//!
//! Live reload: `cargo run --release -p stream-spike --bin stream-serve`,
//! edit a remote component below, press **Reload** in the window.

#![cfg_attr(idealyst_stream_guest, allow(dead_code, unused_imports))]
// And in an app build the remote bodies are compiled out (the bundle has
// them), so the helpers only they call — `product_list`, `price`, … — are
// dead here. `--features inline` compiles the bodies in and uses them.
#![cfg_attr(not(any(idealyst_stream_guest, feature = "inline")), allow(dead_code, unused_imports))]

use std::rc::Rc;

use idea_ui::{tone, typography_kind, Badge, Button, Card, IdeaThemeRef, Slider, Switch, Typography};
use runtime_core::{component, host_fn, rx, signal, ui, Element, ReadSignal, Remote, Signal};
use runtime_shared::primitives::navigator::{Route, RouteParams};
use runtime_shared::{Color, FontWeight, Length, StyleRules, Tokenized};

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

/// App state remote screens read as context: it derives `Remote`, so the
/// feed reads it with a plain `inject` — the density live, as a signal.
/// (Not the theme: that's tokens; see the stylesheets below.)
#[derive(Clone, Remote)]
pub struct FeedPrefs {
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

/// A handler prop (idea-ui takes `Rc<dyn Fn()>`).
fn handler(f: impl Fn() + 'static) -> Rc<dyn Fn()> {
    Rc::new(f)
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

/// The style of every screen root (and navigator layout root) here: fill
/// the parent. A screen sizes ITSELF — its navigator doesn't size it — and
/// a root with no size of its own (a `scroll_view`, whose content scrolls)
/// otherwise collapses to nothing.
fn screen_fill() -> StyleRules {
    StyleRules {
        width: Some(Length::Percent(100.0).into()),
        flex_grow: Some(1.0.into()),
        flex_shrink: Some(1.0.into()),
        flex_basis: px(0.0),
        min_height: px(0.0),
        ..StyleRules::default()
    }
}

// Stylesheets over idea's theme tokens, used by remote and native screens
// alike. Each names tokens (`t.color.background()` → `color-background`);
// the app resolves them against its installed theme, so `set_idea_theme`
// restyles every screen, remote ones included.

runtime_core::stylesheet! {
    /// A screen root: fills its parent, painted with the theme's page color.
    pub Page<IdeaThemeRef> {
        base(t) {
            width: Length::pct(100.0),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 0.0,
            min_height: 0.0,
            background: t.color.background(),
        }
    }
}

runtime_core::stylesheet! {
    /// Plain text in the theme's text color.
    pub Body<IdeaThemeRef> {
        base(t) {
            color: t.color.text(),
        }
    }
}

runtime_core::stylesheet! {
    /// Secondary text.
    pub Muted<IdeaThemeRef> {
        base(t) {
            color: t.color.text_muted(),
        }
    }
}

runtime_core::stylesheet! {
    /// A heading in the primary intent's foreground.
    pub Heading<IdeaThemeRef> {
        base(t) {
            color: t.intent.primary.fg(),
            font_weight: FontWeight::Bold,
        }
    }
}

runtime_core::stylesheet! {
    /// A primary-filled banner.
    pub Banner<IdeaThemeRef> {
        base(t) {
            background: t.intent.primary.solid_bg(),
            padding: t.spacing.md(),
            border_radius: t.radius.md(),
        }
    }
}

runtime_core::stylesheet! {
    /// Text on a `Banner`.
    pub BannerText<IdeaThemeRef> {
        base(t) {
            color: t.intent.primary.solid_text(),
        }
    }
}

// ===========================================================================
// Remote components — shipped in the bundle
// ===========================================================================

/// The feed tab. Its own stylesheets over idea's theme tokens (the app's
/// light/dark switch restyles it), bundle state (likes), app context
/// (`FeedPrefs`), a sync host function (`device_name`), idea-ui components
/// from the app.
#[component(remote)]
pub fn FeedScreen() -> Element {
    let prefs = runtime_world::inject::<FeedPrefs>();
    let compact = prefs.map(|p| p.compact);
    let device = device_name();
    let likes: Vec<Signal<u32>> = POSTS.iter().map(|_| signal(0)).collect();
    ui! {
        scroll_view(style = Page()) {
            view(style = column(10.0)) {
                text(style = Heading()) { "Feed — rendered by the bundle" }
                view(style = Banner()) {
                    text(style = BannerText()) { "This banner's colors are the app's theme tokens, named by the bundle's own stylesheet." }
                }
                // The APP's theme and toast queue, switched from remote code:
                // idea-ui's global functions are `#[host_fn]`s.
                view(style = StyleRules { flex_direction: Some(runtime_shared::FlexDirection::Row), gap: px(8.0), ..StyleRules::default() }) {
                    Button(label = "☀ Light".to_string(), on_click = handler(|| idea_ui::set_idea_color_scheme(runtime_core::ColorScheme::Light)), tone = tone::Neutral)
                    Button(label = "☾ Dark".to_string(), on_click = handler(|| idea_ui::set_idea_color_scheme(runtime_core::ColorScheme::Dark)), tone = tone::Neutral)
                    Button(label = "Toast".to_string(), on_click = handler(|| { idea_ui::push_toast("Hello from the bundle", tone::Success); }), tone = tone::Neutral)
                }
                Badge(label = format!("running on {device}"), tone = tone::Info)
                text(style = Muted()) { move || format!("density: {}", if compact.is_some_and(|c| c.get()) { "compact" } else { "comfortable" }) }
                for (post, like) in POSTS.iter().zip(likes) {
                    Card() {
                        Typography(content = post.title.to_string(), kind = typography_kind::H3)
                        if !compact.is_some_and(|c| c.get()) {
                            Typography(content = post.body.to_string())
                        }
                        Button(label = rx!(format!("♥ {}", like.get())), on_click = handler(move || like.update(|n| n + 1)), tone = tone::Primary)
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
                .style(Page())
                .child(
                    view()
                        .style(column(4.0))
                        .child(button().label("‹ Back").disabled(move || !back.get()).on_press(move || pop()))
                        .child(text().style(Body()).content(move || {
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
            link()
                .route(&PRODUCT, ProductId(p.id))
                .child(ui! {
                    Card() {
                        Typography(content = p.name.to_string(), kind = typography_kind::H3)
                        Typography(content = price(p.price_cents), muted = true)
                    }
                })
                .build()
        })
        .collect();
    ui! {
        scroll_view(style = Page()) {
            view(style = column(8.0)) {
                rows
            }
        }
    }
}

fn product_detail(id: u32, cart: Signal<u32>) -> Element {
    let Some(p) = product(id) else {
        return ui! {
            view(style = Page()) {
                text(style = Body()) { "No such product" }
            }
        };
    };
    let qty = signal(1.0f32);
    let gift = signal(false);
    let reviews = signal(None::<Vec<String>>);
    let on_qty: Rc<dyn Fn(f32)> = Rc::new(move |v| qty.set(v.round()));
    let on_gift: Rc<dyn Fn(bool)> = Rc::new(move |v| gift.set(v));
    let add_to_cart: Rc<dyn Fn()> = Rc::new(move || cart.update(|c| c + qty.get() as u32));
    // An async host function: the app fetches, the bundle applies.
    runtime_core::spawn_then(fetch_reviews(id), move |r| reviews.set(Some(r)));
    ui! {
        scroll_view(style = Page()) {
            view(style = column(10.0)) {
                Card() {
                    Typography(content = p.name.to_string(), kind = typography_kind::H2)
                    Typography(content = p.blurb.to_string())
                    Typography(content = price(p.price_cents), muted = true)
                }
                text(style = Body()) { move || format!("Quantity: {}", qty.get() as u32) }
                Slider(value = rx!(qty.get()), on_change = on_qty, min = 1.0, max = 5.0, step = 1.0)
                text(style = Body()) { move || format!("Gift wrap: {}", if gift.get() { "yes" } else { "no" }) }
                Switch(value = gift, on_change = on_gift, label = "Gift wrap".to_string())
                Button(label = "Add to cart".to_string(), on_click = add_to_cart, tone = tone::Primary)
                text(style = Muted()) { move || match reviews.get() {
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
    use idea_ui::{ToastHost, ToastPlacement};
    use runtime_vocabulary::prims::SwapNav;
    use stream_host::remote::RemoteApp;

    /// The bundle built from this file at compile time — used until a
    /// reload brings a newer one.
    pub const BUILT_IN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bundle.wasm"));
    const SERVED: &str = "http://127.0.0.1:7878/showcase.wasm";
    pub(crate) const REVIEW_DELAY_MS: i32 = 700;

    /// What remote code may call.
    pub fn host_fns() -> Vec<runtime_vocabulary::remote::HostFnDef> {
        let mut fns = vec![device_name::export(), fetch_reviews::export()];
        // What remote code calling idea-ui's global functions needs
        // (`set_idea_color_scheme`, `push_toast`, …).
        fns.extend(idea_ui::host_fns());
        fns
    }

    thread_local! {
        static REMOTE: RefCell<Option<RemoteApp>> = const { RefCell::new(None) };
    }

    /// Install the built-in bundle (call before mounting `App`).
    pub fn install() {
        install_from(BUILT_IN).unwrap_or_else(|e| panic!("built-in bundle: {e}"));
    }

    /// Install `wasm` instead of the built-in bundle (a served build, or
    /// another build of it to measure).
    pub fn install_from(wasm: &[u8]) -> Result<(), String> {
        let remote = stream_host::remote::install_with(wasm, host_fns())?;
        REMOTE.with(|r| *r.borrow_mut() = Some(remote));
        Ok(())
    }

    /// The app's root, for `idealyst::entry!` and the platform shells:
    /// installs the built-in bundle (once), then the tab shell.
    pub fn app() -> Element {
        if REMOTE.with(|r| r.borrow().is_none()) {
            install();
        }
        ui! { App() }
    }

    /// The Android shell's name for [`app`].
    pub fn scene_app() -> Element {
        app()
    }

    /// The platform shells' registration seam. The showcase renders only
    /// builtin primitives, so there is nothing to register.
    pub fn register_scene_extensions<H: runtime_scene::Host>(_registry: &mut runtime_scene::Registry<H>) {}

    /// The installed bundle's linear memory, in bytes (0 before `install`).
    pub fn bundle_memory_bytes() -> usize {
        REMOTE.with(|r| r.borrow().as_ref().map_or(0, RemoteApp::memory_bytes))
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
        // idea-ui's theme lives in the app: the components the bundle
        // imports render here, natively, against it.
        idea_ui::install_idea_theme_schemes(idea_ui::light_theme(), idea_ui::dark_theme());
        let compact = signal(false);
        let cart = signal(0u32);
        let status = signal(format!("built-in bundle ({} KB)", BUILT_IN.len() / 1024));
        runtime_world::provide(FeedPrefs { compact: compact.read_only() });
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
                // The shell is the root: it keeps clear of the status bar,
                // notch and home indicator (safe area, read live).
                view()
                    .style(|| {
                        let ins = runtime_core::safe_area_insets().get();
                        // The home-indicator strip shows the theme's page
                        // color too (a token, like the screens' sheets).
                        Rc::new(StyleRules {
                            padding_bottom: px(ins.bottom),
                            background: Some(Tokenized::token("color-background", Color("#f6f7f9".into()))),
                            ..screen_fill()
                        })
                    })
                    .child(
                        view()
                            .style(|| {
                                let ins = runtime_core::safe_area_insets().get();
                                Rc::new(StyleRules { background: color("#111827"), padding_top: px(12.0 + ins.top), ..column(6.0) })
                            })
                            .child(tab("feed", "Feed"))
                            .child(tab("shop", "Shop"))
                            .child(tab("settings", "Settings"))
                            .child(
                                text()
                                    .style(StyleRules { color: color("#e5e7eb"), ..StyleRules::default() })
                                    .content(move || format!("cart: {}   ·   {}", cart.get(), status.get())),
                            )
                            .child(button().label("Reload remote").on_press(move || reload(status)))
                            .build(),
                    )
                    .child(navigator_outlet().build())
                    // The app's toast queue — where `push_toast` from remote
                    // code lands too.
                    .child(ui! { ToastHost(placement = ToastPlacement::BottomCenter) })
                    .build()
            })
            .screen(FEED, |()| ui! { FeedScreen() })
            .screen(SHOP, move |()| ui! { ShopNavigator(cart = cart) })
            .screen(SETTINGS, move |()| settings(compact, cart))
            .build()
    }

    /// Native settings: the idea theme every screen follows (remote ones
    /// through their token stylesheets), and the `FeedPrefs` the feed reads.
    fn settings(compact: Signal<bool>, cart: Signal<u32>) -> Element {
        ui! {
            scroll_view(style = Page()) {
                view(style = column(10.0)) {
                    text(style = Heading()) { "Settings — native" }
                    button(label = "Light theme", on_click = || idea_ui::set_idea_color_scheme(runtime_core::ColorScheme::Light))
                    button(label = "Dark theme", on_click = || idea_ui::set_idea_color_scheme(runtime_core::ColorScheme::Dark))
                    button(label = move || format!("Compact feed: {}", if compact.get() { "on" } else { "off" }), on_click = move || compact.update(|c| !c))
                    Card() {
                        Typography(content = "Cart".to_string(), kind = typography_kind::H3)
                        text(style = Body()) { move || format!("{} item(s)", cart.get()) }
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
