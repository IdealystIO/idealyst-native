//! `welcome` — three-act cinematic intro, driven by the framework's
//! animation system (springs + tweens) rather than `Presence`'s
//! enter/exit transitions.
//!
//! Act 1 — "Welcome to Idealyst" rises into a light frame, settles.
//! Act 2 — The frame washes to dark; a warm sun-glare blooms in
//!         from the top-right corner; the welcome phrase exits.
//! Act 3 — Content scales from oversized down to rest, reading as
//!         "focus pulling in."
//!
//! Each animated property is its own `AnimatedValue<f32>` bound to
//! a `Ref<ViewHandle>` via `subscribe_and_apply`. Per-frame the
//! subscriber writes `opacity` / `transform: translate(...) scale(...)`
//! as inline CSS via `web::set_animated_f32`. The act-sequence
//! `effect!` fires `av.animate(SpringTo::new(...))` (or `TweenTo`)
//! calls at the right moments — that's the whole orchestration.
//!
//! Springs (`SpringTo::new(target).stiffness(s).damping(d)`) carry
//! the entrances; tweens with cubic-bezier easing carry fades and
//! exits.
//!
//! `app()` is invoked via `framework_core::mount(backend, super::app)`
//! (see `src/web.rs`) so the `effect!` below adopts the root scope.

#[cfg(target_arch = "wasm32")]
mod web;

use std::rc::Rc;
use std::time::Duration;

use framework_core::animation::{AnimProp, AnimatedValue, SpringTo, TweenTo};
use framework_core::{
    animated, effect, node_ref, on_cleanup, timeline, ui, AlignItems, Color, FlexDirection,
    FontWeight, JustifyContent, Length, Position, Primitive, Ref, Shadow, StyleRules, StyleSheet,
    TextAlign, Tokenized, ViewHandle,
};

// ---- Timing (milliseconds) ----------------------------------------------

/// Pause after page load before Act 1's phrase begins entering.
const INTRO_PAUSE_MS: i32 = 400;

/// Act 1 hold — how long the welcome phrase sits at rest before
/// the dark wash begins.
const ACT_1_HOLD_MS: i32 = 1700;

/// Welcome phrase enter — how long the spring takes to roughly
/// settle (springs don't have a hard duration; this is the lag we
/// schedule against before Act 2 begins).
const PHRASE_ENTER_BUDGET_MS: i32 = 900;

/// Dark wash duration. Slow, deliberate.
const DARK_FADE_MS: u64 = 1300;

/// How long after the sun-glare starts blooming the content arrives.
/// The content enters *during* the glare's bloom so the scene
/// transformation (welcome out → dark + glare in → content in) reads
/// as one composed motion rather than three sequential beats.
///
/// Tuned by ear: long enough that the welcome phrase is well past
/// gone before content lands; short enough that the glare and
/// content are visibly arriving together.
const CONTENT_OFFSET_AFTER_GLARE_MS: i32 = 600;

// ---- Motion / scale constants -------------------------------------------

/// Initial rise distance for the welcome phrase, in CSS pixels.
const PHRASE_ENTER_Y: f32 = 24.0;

/// How far the welcome phrase shuffles UP in Act 3 to make room
/// for the subtitle. Just enough that the subtitle reads as "new
/// information appearing below," not "second line of a paragraph."
const WELCOME_SHUFFLE_Y: f32 = -28.0;

/// Initial scale of the welcome phrase — slightly under 1.0 so the
/// spring eases up into the resting size with a touch of bounce.
const PHRASE_ENTER_SCALE: f32 = 0.95;

/// Initial rise distance for the subtitle. Small — its main motion
/// is the fade-in; the small slide adds character without competing
/// with the welcome's shuffle.
const SUBTITLE_ENTER_Y: f32 = 10.0;

/// Sun-glare bloom — starts small (a tight point of light) and
/// scales up to its resting size. A loose spring (low stiffness,
/// moderate damping) gives the bloom a slow, organic spread.
const GLARE_INITIAL_SCALE: f32 = 0.55;

// ---- Color palette -------------------------------------------------------

const COLOR_LIGHT_BG: &str = "#f7f5ef";
const COLOR_DARK_BG: &str = "#0a0c11";
const COLOR_HEADLINE_DARK: &str = "#0a0c11";
const COLOR_HEADLINE_LIGHT: &str = "#f4ead8";
const COLOR_SUBTITLE_LIGHT: &str = "#a89a7d";
/// Sun-glare core — near-white with the faintest warmth.
const COLOR_SUN_CORE: &str = "#fff6d8";
/// Sun-glare glow — warm amber. Used as the shadow color on the
/// circular layers so the bloom bleeds outward.
const COLOR_SUN_GLOW: &str = "rgba(255, 196, 120, 0.85)";

// ---- Typography sizes ----------------------------------------------------

const HEADLINE_SIZE_PX: f32 = 56.0;
const SUBTITLE_SIZE_PX: f32 = 18.0;

pub fn app() -> Primitive {
    // ---- Animated values -----------------------------------------------
    //
    // One AV per (element, property) pair. Each AV holds a current
    // value + velocity; `animate(...)` retargets, the clock ticks,
    // subscribers write the new value to the bound element each
    // frame. Springs and tweens compose freely on the same value
    // (e.g. a spring can be retargeted mid-flight to a tween).

    let welcome_opacity = animated!(0.0_f32);
    let welcome_scale = animated!(PHRASE_ENTER_SCALE);
    let welcome_y = animated!(PHRASE_ENTER_Y);
    // Foreground color of the welcome phrase. Starts at the dark
    // ink color (matches the light background); animates to the
    // light cream color during Act 2 so the phrase stays readable
    // once the background goes dark.
    let welcome_color = animated!(srgb_tuple(COLOR_HEADLINE_DARK));

    let dark_opacity = animated!(0.0_f32);

    let glare_opacity = animated!(0.0_f32);
    let glare_scale = animated!(GLARE_INITIAL_SCALE);

    let subtitle_opacity = animated!(0.0_f32);
    let subtitle_y = animated!(SUBTITLE_ENTER_Y);

    // ---- View refs -----------------------------------------------------

    let welcome_ref = node_ref!(ViewHandle);
    let dark_ref = node_ref!(ViewHandle);
    let glare_ref = node_ref!(ViewHandle);
    let subtitle_ref = node_ref!(ViewHandle);

    // Wire each AV to its target node + property. After this,
    // every `animate(...)` call automatically writes per-frame
    // values into the DOM.
    drive_av(&welcome_opacity, welcome_ref, AnimProp::Opacity);
    drive_av(&welcome_scale, welcome_ref, AnimProp::Scale);
    drive_av(&welcome_y, welcome_ref, AnimProp::TranslateY);
    drive_color_av(&welcome_color, welcome_ref, AnimProp::ForegroundColor);
    drive_av(&dark_opacity, dark_ref, AnimProp::Opacity);
    drive_av(&glare_opacity, glare_ref, AnimProp::Opacity);
    drive_av(&glare_scale, glare_ref, AnimProp::Scale);
    drive_av(&subtitle_opacity, subtitle_ref, AnimProp::Opacity);
    drive_av(&subtitle_y, subtitle_ref, AnimProp::TranslateY);

    // ---- Schedule the timeline -----------------------------------------
    //
    // Three acts, each fires a handful of `animate(...)` calls
    // simultaneously to drive the per-property motion. Springs do
    // most of the heavy lifting; tweens carry the longer fades.

    let act_1_start = INTRO_PAUSE_MS;
    let act_2_start = act_1_start + PHRASE_ENTER_BUDGET_MS + ACT_1_HOLD_MS;
    // The glare itself starts 200ms after Act 2 begins (the lag
    // built into its scheduling below). Content enters
    // `CONTENT_OFFSET_AFTER_GLARE_MS` later — so glare and content
    // bloom in parallel rather than sequentially.
    let act_3_start = act_2_start + 200 + CONTENT_OFFSET_AFTER_GLARE_MS;

    effect!({
        // The whole cinematic timeline in one declarative block.
        // Each `t => { ... }` clause fires all its `av: animator`
        // pairs at moment `t`. Tasks are collected into a Vec
        // kept alive by `on_cleanup` for the page lifetime.
        let tasks = timeline! {
            // ── Act 1 — welcome phrase enters with a settle spring.
            act_1_start => {
                welcome_opacity: TweenTo::new(1.0, Duration::from_millis(700)).ease_out(),
                welcome_scale: SpringTo::new(1.0).stiffness(170.0).damping(22.0),
                welcome_y: SpringTo::new(0.0).stiffness(170.0).damping(22.0),
            },
            // ── Act 2 — wash to dark, sun-glare blooms in. The
            // welcome phrase stays mounted; only its color animates
            // (dark ink → light cream) so the phrase remains
            // readable as the background swings under it.
            act_2_start => {
                welcome_color: TweenTo::new(
                    srgb_tuple(COLOR_HEADLINE_LIGHT),
                    Duration::from_millis(DARK_FADE_MS),
                ).ease_in_out(),
                dark_opacity: TweenTo::new(1.0, Duration::from_millis(DARK_FADE_MS)).ease_in_out(),
            },
            // The glare lags the dark by a beat so it reads as
            // arriving INTO the dark scene, not painted with it.
            // Loose spring (low stiffness, gentle damping) gives
            // the bloom a slow, organic spread rather than a snap.
            act_2_start + 200 => {
                glare_opacity: TweenTo::new(1.0, Duration::from_millis(1700)).ease_out(),
                glare_scale: SpringTo::new(1.0).stiffness(55.0).damping(18.0),
            },
            // ── Act 3 — welcome shuffles UP, subtitle materializes
            // beneath it. Two parallel motions, both gentle springs.
            act_3_start => {
                welcome_y: SpringTo::new(WELCOME_SHUFFLE_Y).stiffness(110.0).damping(20.0),
                subtitle_opacity: TweenTo::new(1.0, Duration::from_millis(800)).ease_out(),
                subtitle_y: SpringTo::new(0.0).stiffness(140.0).damping(20.0),
            },
        };
        on_cleanup(move || drop(tasks));
    });

    // ---- Build the tree ------------------------------------------------
    //
    // Five layers in document order = z-order:
    //   1. page (light bg, relative)
    //   2. dark layer (absolute, fills page)
    //   3. glare anchor (absolute, top-right)
    //   4. content layer (absolute, fills page, flex-centered)
    //
    // The content layer holds BOTH the welcome phrase and the
    // Act 3 content — they're both present in the DOM from
    // mount, but each starts with opacity 0 via its initial AV
    // value. Once the schedule fires animations, the AV
    // subscribers paint per-frame values.

    let page = page_sheet();
    let dark_layer = dark_layer_sheet();
    let glare_anchor = glare_anchor_sheet();
    let glare_outer = glare_layer_sheet(true);
    let glare_mid = glare_layer_sheet(false);
    let glare_core = glare_core_sheet();
    let content_layer = content_layer_sheet();
    let welcome_wrap = welcome_wrapper_sheet();
    let subtitle_wrap = subtitle_wrapper_sheet();
    let headline = headline_sheet();
    let subtitle = subtitle_sheet();

    ui! {
        View(style = page) {
            // Dark wash. Opacity driven by `dark_opacity` AV.
            View(style = dark_layer) {}.bind(dark_ref)

            // Sun glare anchor — opacity AND scale driven; the
            // three circular layers inside compose the radial
            // bloom.
            View(style = glare_anchor) {
                View(style = glare_outer) {}
                View(style = glare_mid) {}
                View(style = glare_core) {}
            }.bind(glare_ref)

            // Content layer — holds the welcome phrase + subtitle
            // in a vertical column. Welcome stays mounted the
            // whole time; its color animates with the background
            // swap, its translate_y shuffles up in Act 3 to make
            // room for the subtitle that appears below.
            View(style = content_layer) {
                // Welcome phrase. Opacity / scale / y / color all
                // animated.
                View(style = welcome_wrap) {
                    Text(style = headline) { "Welcome to Idealyst" }
                }.bind(welcome_ref)

                // Subtitle. Hidden at start (opacity 0 via
                // stylesheet); fades in + slides up in Act 3.
                View(style = subtitle_wrap) {
                    Text(style = subtitle) { "Your app starts here." }
                }.bind(subtitle_ref)
            }
        }
    }
}

// =============================================================================
// AnimatedValue → DOM bridge
// =============================================================================

/// Subscribe `av` so every per-frame value writes to `view_ref`'s
/// node under `prop`. Until the ref is filled (the walker hasn't
/// mounted the view yet), the listener silently skips. After mount,
/// each frame writes one inline CSS property.
///
/// The returned `Subscription` is intentionally leaked — this is
/// the page's animation, not a per-component effect, so its
/// lifetime is the page lifetime.
fn drive_av(av: &AnimatedValue<f32>, view_ref: Ref<ViewHandle>, prop: AnimProp) {
    let sub = av.subscribe_and_apply(move |v, _vel| {
        let value = *v;
        view_ref.with(|handle| {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(node) = handle.as_any().downcast_ref::<web_sys::Node>() {
                    crate::web::set_animated_f32(node, prop, value);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = (handle, value, prop);
            }
        });
    });
    std::mem::forget(sub);
}

/// Color-family counterpart of [`drive_av`]. Subscribes a
/// 4-tuple AnimatedValue (sRGB `(r, g, b, a)` in `0..=1`) to a view
/// ref, writing the channels through `set_animated_color` each
/// frame. Used for the welcome phrase's color animation through the
/// dark-wash transition.
fn drive_color_av(
    av: &AnimatedValue<(f32, f32, f32, f32)>,
    view_ref: Ref<ViewHandle>,
    prop: AnimProp,
) {
    let sub = av.subscribe_and_apply(move |v, _vel| {
        let (r, g, b, a) = *v;
        view_ref.with(|handle| {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(node) = handle.as_any().downcast_ref::<web_sys::Node>() {
                    crate::web::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = (handle, r, g, b, a, prop);
            }
        });
    });
    std::mem::forget(sub);
}

/// Convert a `#rrggbb` (or `#rgb`) hex color to the
/// `(r, g, b, a)` tuple the color AVs use. Channels in `0..=1`,
/// alpha always 1.0 (the welcome phrase doesn't fade alpha — it
/// fades the bulk through the wrapper's opacity, which is a
/// separate property).
fn srgb_tuple(hex: &str) -> (f32, f32, f32, f32) {
    let h = hex.trim_start_matches('#');
    let parse = |s: &str| u8::from_str_radix(s, 16).unwrap_or(0) as f32 / 255.0;
    let (r, g, b) = if h.len() == 6 {
        (parse(&h[0..2]), parse(&h[2..4]), parse(&h[4..6]))
    } else if h.len() == 3 {
        let ch = |c: char| u8::from_str_radix(&c.to_string().repeat(2), 16).unwrap_or(0) as f32
            / 255.0;
        let bytes: Vec<char> = h.chars().collect();
        (ch(bytes[0]), ch(bytes[1]), ch(bytes[2]))
    } else {
        (0.0, 0.0, 0.0)
    };
    (r, g, b, 1.0)
}

// =============================================================================
// Stylesheets
// =============================================================================

/// Root frame. Light background, viewport-filling, relative
/// positioning so absolute children can pin to its edges.
fn page_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Relative),
        width: Some(pct(100.0)),
        height: Some(pct(100.0)),
        background: Some(col(COLOR_LIGHT_BG)),
        ..Default::default()
    })
}

/// Full-page dark wash. Fills the entire viewport with the dark
/// color; its opacity is animated 0 → 1 in Act 2.
fn dark_layer_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        background: Some(col(COLOR_DARK_BG)),
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}

/// Anchor box for the sun glare. Positioned so the bloom's centre
/// sits ABOVE and to the RIGHT of the visible viewport — most of
/// the glow bleeds in from off-screen.
fn glare_anchor_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(-180.0)),
        right: Some(px(-180.0)),
        width: Some(px(360.0)),
        height: Some(px(360.0)),
        overflow: Some(framework_core::Overflow::Visible),
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}

/// One ring of the sun glare. `outer = true` is the wide soft halo;
/// `outer = false` is the medium corona. Both are circular Views
/// stacked at the same anchor point. Their box backgrounds are
/// half-transparent; their `Shadow` is what does the bulk of the
/// bleed.
fn glare_layer_sheet(outer: bool) -> Rc<StyleSheet> {
    let (size, blur, bg_alpha) = if outer {
        (340.0_f32, 220.0_f32, "rgba(255, 196, 120, 0.18)")
    } else {
        (200.0_f32, 160.0_f32, "rgba(255, 220, 160, 0.45)")
    };
    let inset = (360.0 - size) / 2.0;
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(inset)),
        left: Some(px(inset)),
        width: Some(px(size)),
        height: Some(px(size)),
        border_top_left_radius: Some(px(999.0)),
        border_top_right_radius: Some(px(999.0)),
        border_bottom_left_radius: Some(px(999.0)),
        border_bottom_right_radius: Some(px(999.0)),
        background: Some(col(bg_alpha)),
        shadow: Some(Shadow {
            x: 0.0,
            y: 0.0,
            blur,
            color: Color(COLOR_SUN_GLOW.into()),
        }),
        ..Default::default()
    })
}

/// The bright core at the centre of the sun. Small, near-white,
/// with a tight intense shadow that adds the brightest part of the
/// bloom.
fn glare_core_sheet() -> Rc<StyleSheet> {
    let size = 100.0_f32;
    let inset = (360.0 - size) / 2.0;
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(inset)),
        left: Some(px(inset)),
        width: Some(px(size)),
        height: Some(px(size)),
        border_top_left_radius: Some(px(999.0)),
        border_top_right_radius: Some(px(999.0)),
        border_bottom_left_radius: Some(px(999.0)),
        border_bottom_right_radius: Some(px(999.0)),
        background: Some(col(COLOR_SUN_CORE)),
        shadow: Some(Shadow {
            x: 0.0,
            y: 0.0,
            blur: 80.0,
            color: Color("rgba(255, 246, 216, 0.95)".into()),
        }),
        ..Default::default()
    })
}

/// Absolutely-positioned column that holds the welcome phrase and
/// the subtitle. Flex-centered so the pair sits in the middle of
/// the page; the welcome ends up slightly above center, subtitle
/// slightly below.
fn content_layer_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        // Modest gap between the headline and where the subtitle
        // lands. The welcome's Act 3 shuffle-up adds visual breathing
        // room on top of this.
        gap: Some(px(14.0)),
        ..Default::default()
    })
}

/// Wrapper around the welcome phrase. Opacity 0 at start (the
/// `welcome_opacity` AV animates it up in Act 1).
fn welcome_wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        opacity: Some(Tokenized::Literal(0.0)),
        // Dark ink at rest; `welcome_color` AV transitions this to
        // the light cream during Act 2. The animated inline color
        // overrides this initial value.
        color: Some(col(COLOR_HEADLINE_DARK)),
        ..Default::default()
    })
}

/// Wrapper around the subtitle. Opacity 0 at start — Act 3 fades
/// it in and slides it up.
fn subtitle_wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}

/// Headline text style — the welcome phrase wears this. Color is
/// driven by the parent wrapper's animated `color` property, which
/// CSS inheritance carries to the text node.
fn headline_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(HEADLINE_SIZE_PX)),
        font_weight: Some(FontWeight::Bold),
        letter_spacing: Some(Tokenized::Literal(-1.6)),
        line_height: Some(Tokenized::Literal(HEADLINE_SIZE_PX + 8.0)),
        text_align: Some(TextAlign::Center),
        ..Default::default()
    })
}

/// Subtitle under the Act 3 headline.
fn subtitle_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(SUBTITLE_SIZE_PX)),
        font_weight: Some(FontWeight::Normal),
        letter_spacing: Some(Tokenized::Literal(0.6)),
        line_height: Some(Tokenized::Literal(SUBTITLE_SIZE_PX + 8.0)),
        text_align: Some(TextAlign::Center),
        color: Some(col(COLOR_SUBTITLE_LIGHT)),
        ..Default::default()
    })
}

fn static_sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

fn pct(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Percent(v))
}

fn col(s: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(s.into()))
}
