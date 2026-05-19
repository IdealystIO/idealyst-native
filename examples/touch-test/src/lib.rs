//! Touch-system smoke test.
//!
//! Single platform-agnostic crate. [`app`] returns a `Primitive`
//! tree that exercises every case in the raw touch pipeline:
//!
//! 1. **Raw touch** (red box) — bare `on_touch` handler; every
//!    event is logged with phase, id, position, and force.
//! 2. **Tap recognizer** (green box) — `tap()` factory from
//!    `framework_core::touch::recognizers`. Fires once per clean
//!    press-release within slop + timeout.
//! 3. **Long-press recognizer** (blue box) — `long_press()`.
//!    Fires once after holding still for ~500ms.
//! 4. **Responder-chain bubble** (yellow inside black) — inner
//!    handler consumes Began so the outer never sees it. Tap the
//!    yellow inner and the outer ring's handler stays silent;
//!    tap the dark ring and the outer's handler fires.
//! 5. **Claim protocol** (orange box inside a scroll view) — a
//!    Moved handler returning `claim: true` should preempt the
//!    parent scroll. Slowly drag vertically on the orange box;
//!    the scroll view does not move. Drag on gray filler above /
//!    below, and the scroll view scrolls normally.
//! 6. **Cancellation** — implicit. Press a box, then scroll the
//!    parent past the slop without claim; expect a `Cancelled`
//!    phase log.
//!
//! Output goes to `println!` — Xcode console / Android logcat /
//! browser JS console / desktop terminal — and to an on-screen
//! log overlay pinned to the bottom of the viewport.
//!
//! See `docs/native-touch-plan.md` and
//! `docs/native-touch-backends-plan.md` for the design.

#[cfg(target_arch = "wasm32")]
mod web;

use framework_core::{
    long_press, pan, signal, tap, text, view, AlignItems, Bound, Color, Easing, FlexDirection,
    JustifyContent, Length, LongPressRecognizer, PanEvent, PanRecognizer, Primitive, Signal,
    StyleRules, StyleSheet, TapRecognizer, TextHandle, Tokenized, TouchPhase, TouchResponse,
    Transform, Transition, ViewHandle,
};
use framework_theme::{install_theme, ThemeTokens, TokenEntry};
use std::rc::Rc;

/// Cap on lines kept in the visible log overlay. Older lines drop
/// off the top so the overlay doesn't grow without bound during a
/// long test session.
const LOG_MAX_LINES: usize = 16;

/// Root entry. Built once at app start; the framework re-renders
/// the log overlay in place as touch events update the signal.
/// Empty theme — the framework requires `install_theme(...)` to
/// be called before `render(...)`, even when no stylesheet reads
/// tokens. Our static stylesheets ignore the theme; this stub
/// exists to satisfy the runtime invariant.
struct EmptyTheme;
impl ThemeTokens for EmptyTheme {
    fn tokens(&self) -> Vec<TokenEntry> {
        Vec::new()
    }
}

pub fn app() -> Primitive {
    install_theme(EmptyTheme);

    // Pan-only scene. Stripped of every other section + the
    // ScrollView wrapper so the per-event cost during a drag is
    // *only* the pan recognizer's path. Use this to measure jank
    // without unrelated layout / paint contention.
    let log: Signal<String> = signal!(String::new());

    view(vec![
        title_text("Pan smoke").into(),
        text(move || log.get()).with_style(log_inline_sheet()).into(),
        pan_box(log).into(),
    ])
    .with_style(body_sheet())
    .into()
}

// =============================================================================
// Test boxes
// =============================================================================

fn raw_box(log: Signal<String>) -> Bound<ViewHandle> {
    view(vec![text("Tap, hold, drag")
        .with_style(label_text_sheet())
        .into()])
        .with_style(box_sheet("#ff5555"))
        .on_touch(move |ev| {
            log_event(
                log,
                format_args!(
                    "[raw] {:?} id={} pos=({:.1},{:.1}) force={}",
                    ev.phase,
                    ev.id.0 % 1000,
                    ev.position.x,
                    ev.position.y,
                    fmt_force(ev.force),
                ),
            );
            TouchResponse::CONSUMED
        })
}

fn tap_box(log: Signal<String>) -> Bound<ViewHandle> {
    let handler = tap(TapRecognizer::new(), move || {
        log_line(log, "[tap] fired");
    });
    view(vec![text("Tap me").with_style(label_text_sheet()).into()])
        .with_style(box_sheet("#44cc66"))
        .on_touch(move |ev| handler(ev))
}

fn long_press_box(log: Signal<String>) -> Bound<ViewHandle> {
    let handler = long_press(LongPressRecognizer::new(), move || {
        log_line(log, "[long-press] fired");
    });
    view(vec![text("Hold ~500ms")
        .with_style(label_text_sheet())
        .into()])
        .with_style(box_sheet("#5588ff"))
        .on_touch(move |ev| handler(ev))
}

fn nested_box(log: Signal<String>) -> Bound<ViewHandle> {
    let inner: Primitive = view(vec![text("inner")
        .with_style(label_text_sheet())
        .into()])
        .with_style(box_sheet("#ffdd44"))
        .on_touch(move |ev| {
            if matches!(ev.phase, TouchPhase::Began | TouchPhase::Ended) {
                log_line(log, &format!("[inner] {:?} consumed", ev.phase));
            }
            TouchResponse::CONSUMED
        })
        .into();
    view(vec![inner])
        .with_style(outer_box_sheet())
        .on_touch(move |ev| {
            if matches!(ev.phase, TouchPhase::Began | TouchPhase::Ended) {
                log_line(log, &format!("[outer] {:?}", ev.phase));
            }
            TouchResponse::IGNORED
        })
}

fn claim_box(log: Signal<String>) -> Bound<ViewHandle> {
    view(vec![text("Drag me (vertical)")
        .with_style(label_text_sheet())
        .into()])
        .with_style(box_sheet("#ff9933"))
        .on_touch(move |ev| match ev.phase {
            TouchPhase::Began => {
                log_line(log, "[claim] Began");
                TouchResponse::CONSUMED
            }
            TouchPhase::Moved => {
                // claim: true → suppress the parent scroll. On iOS
                // this cancels UIScrollView.panGestureRecognizer; on
                // Android requestDisallowInterceptTouchEvent; on web
                // setPointerCapture; on wgpu, marks the touch claimed
                // in the dispatcher.
                log_event(log, format_args!("[claim] Moved y={:.1}", ev.position.y));
                TouchResponse {
                    consumed: true,
                    claim: true,
                }
            }
            TouchPhase::Ended => {
                log_line(log, "[claim] Ended");
                TouchResponse::CONSUMED
            }
            TouchPhase::Cancelled => {
                log_line(log, "[claim] Cancelled (parent stole it)");
                TouchResponse::CONSUMED
            }
        })
}

fn pan_box(log: Signal<String>) -> Bound<ViewHandle> {
    // Two signals own the drag state:
    //   - `position` is the live offset; updated per touch event
    //     during drag, then animated back to (0,0) on release.
    //   - `dragging` is the discrete "is the finger down" flag.
    //     Used to switch the transform transition between "instant"
    //     (during drag — we want pixel-perfect follow) and "spring
    //     back" (after release — we want a 250ms ease-out).
    let position: Signal<(f32, f32)> = signal!((0.0, 0.0));
    let dragging: Signal<bool> = signal!(false);
    // Snapshot of `position` at gesture start. Pan deltas are
    // relative to the touch's `Began` point; we want them relative
    // to the box's current rest offset, which may not be zero if a
    // previous release is still spring-animating.
    let start_pos: Signal<(f32, f32)> = signal!((0.0, 0.0));

    let pos = position;
    let drag = dragging;
    let start = start_pos;
    let handler = pan(PanRecognizer::new(), move |ev| match ev {
        PanEvent::Began { .. } => {
            start.set(pos.get());
            drag.set(true);
            // Log only on boundaries — every Move would re-render
            // the log TextView, which on Android forces a measure
            // pass per event and dominates the jank budget.
            log_line(log, "[pan] Began");
        }
        PanEvent::Moved { delta, .. } => {
            // Hot path: ONE signal write per event, nothing else.
            // The reactive stylesheet picks this up and pushes
            // `transform.translate(x, y)` to the backend.
            let (sx, sy) = start.get();
            pos.set((sx + delta.x, sy + delta.y));
        }
        PanEvent::Ended { velocity } => {
            drag.set(false);
            // Spring back to origin. The reactive transition
            // (`pan_box_sheet`) flips to a 250ms ease-out the
            // moment `dragging` goes false, so this set kicks off
            // the animation natively rather than tick-by-tick from
            // Rust.
            pos.set((0.0, 0.0));
            log_event(
                log,
                format_args!(
                    "[pan] Ended v=({:.0},{:.0}) → spring back",
                    velocity.x, velocity.y,
                ),
            );
        }
        PanEvent::Cancelled => {
            drag.set(false);
            pos.set((0.0, 0.0));
            log_line(log, "[pan] Cancelled");
        }
    });

    view(vec![text("Drag me").with_style(label_text_sheet()).into()])
        .with_style(pan_box_sheet(position, dragging))
        .on_touch(move |ev| handler(ev))
}

// =============================================================================
// Layout helpers
// =============================================================================

fn title_text(s: &'static str) -> Bound<TextHandle> {
    text(s).with_style(title_sheet())
}

fn section_label(s: &'static str) -> Bound<TextHandle> {
    text(s).with_style(section_sheet())
}

fn gray_filler(h: f32) -> Bound<ViewHandle> {
    view(vec![]).with_style(filler_sheet(h))
}

// =============================================================================
// Logging
// =============================================================================

fn log_line(log: Signal<String>, line: &str) {
    // Mirror to platform stdout (Xcode console, logcat, browser
    // console, terminal).
    #[cfg(not(target_arch = "wasm32"))]
    println!("{line}");
    #[cfg(target_arch = "wasm32")]
    web_sys::console::log_1(&line.into());

    let mut s = log.get();
    s.push_str(line);
    s.push('\n');
    let lines: Vec<&str> = s.lines().collect();
    let trimmed = if lines.len() > LOG_MAX_LINES {
        lines[lines.len() - LOG_MAX_LINES..].join("\n")
    } else {
        s.clone()
    };
    log.set(trimmed);
}

fn log_event(log: Signal<String>, args: std::fmt::Arguments<'_>) {
    log_line(log, &std::fmt::format(args));
}

fn fmt_force(f: Option<f32>) -> String {
    match f {
        Some(v) => format!("{v:.2}"),
        None => "none".into(),
    }
}

// =============================================================================
// Stylesheets — flat, theme-less, literal-color. The example
// exercises the touch pipeline; styling stays out of the way.
// =============================================================================

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

fn pct(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Percent(v))
}

fn col(hex: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(hex.to_string()))
}

fn radius(rules: &mut StyleRules, r: f32) {
    rules.border_top_left_radius = Some(px(r));
    rules.border_top_right_radius = Some(px(r));
    rules.border_bottom_left_radius = Some(px(r));
    rules.border_bottom_right_radius = Some(px(r));
}

fn body_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Stretch),
        gap: Some(px(12.0)),
        padding_top: Some(px(24.0)),
        padding_right: Some(px(16.0)),
        padding_bottom: Some(px(180.0)), // room for log overlay
        padding_left: Some(px(16.0)),
        ..Default::default()
    })
}

fn scroll_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        background: Some(col("#f2f3f6")),
        ..Default::default()
    })
}

fn root_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        min_height: Some(pct(100.0)),
        ..Default::default()
    })
}

fn title_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(22.0)),
        color: Some(col("#11141a")),
        ..Default::default()
    })
}

fn section_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(13.0)),
        color: Some(col("#555a66")),
        padding_top: Some(px(8.0)),
        ..Default::default()
    })
}

fn label_text_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        color: Some(col("#11141a")),
        font_size: Some(px(14.0)),
        ..Default::default()
    })
}

fn box_sheet(bg_hex: &str) -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        background: Some(col(bg_hex)),
        width: Some(px(160.0)),
        height: Some(px(110.0)),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    };
    radius(&mut rules, 8.0);
    static_sheet(rules)
}

fn outer_box_sheet() -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        background: Some(col("#222222")),
        width: Some(px(180.0)),
        height: Some(px(140.0)),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        padding_top: Some(px(15.0)),
        padding_right: Some(px(15.0)),
        padding_bottom: Some(px(15.0)),
        padding_left: Some(px(15.0)),
        ..Default::default()
    };
    radius(&mut rules, 8.0);
    static_sheet(rules)
}

fn filler_sheet(h: f32) -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        background: Some(col("#dcdfe6")),
        height: Some(px(h)),
        ..Default::default()
    };
    radius(&mut rules, 4.0);
    static_sheet(rules)
}

fn log_text_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        color: Some(col("#dce1ea")),
        font_size: Some(px(11.0)),
        ..Default::default()
    })
}

fn log_inline_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        color: Some(col("#11141a")),
        font_size: Some(px(11.0)),
        background: Some(col("#dcdfe6")),
        padding_top: Some(px(8.0)),
        padding_right: Some(px(10.0)),
        padding_bottom: Some(px(8.0)),
        padding_left: Some(px(10.0)),
        ..Default::default()
    })
}

fn static_sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

/// Reactive stylesheet for the pan demo. Reads `position` to set
/// the transform, and `dragging` to flip the transition between
/// "instant" (live drag) and "250ms ease-out" (spring back). The
/// closure is invoked inside an Effect; both signals subscribe
/// it, so any `.set()` re-resolves the style and the backend
/// pushes the new transform.
///
/// Note: each Move event currently re-runs this whole closure on
/// the framework thread, which goes through `apply_style`. For
/// 60Hz pan on a phone-class device the cost is negligible (one
/// transform-only style apply per frame); when the motion-value
/// plan lands, the per-event hop disappears entirely.
fn pan_box_sheet(
    position: Signal<(f32, f32)>,
    dragging: Signal<bool>,
) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(move |_vs: &framework_core::VariantSet| {
        let (x, y) = position.get();
        let is_dragging = dragging.get();
        let mut rules = StyleRules {
            background: Some(col("#9b59ff")),
            width: Some(px(160.0)),
            height: Some(px(110.0)),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            transform: Some(vec![
                Transform::TranslateX(Length::Px(x)),
                Transform::TranslateY(Length::Px(y)),
            ]),
            // Only animate the transform on RELEASE — during the
            // drag we want pixel-for-pixel finger-follow, which
            // means no transition.
            transform_transition: if is_dragging {
                None
            } else {
                Some(Transition::new(250, Easing::EaseOut))
            },
            ..Default::default()
        };
        radius(&mut rules, 8.0);
        rules
    }))
}
