//! The navigator-pushed screens (`Settings`, `Preview`) and their shared
//! scaffold. Like the board's chrome, these are normal host-tree content, so
//! reactive style + ordinary components are fine.
//!
//! Every screen carries its OWN in-content header (`ScreenHeader`: title + a back
//! `×` that pops the stack), because `header_shown(false)` is set on every route
//! — the stack handler renders native chrome on iOS / Android / web but NOT on
//! macOS / terminal / SSR, so without the in-content header the user couldn't get
//! back there.

use crate::settings::{aspect_label, CanvasBg, ASPECTS, ASPECT_MAX, ASPECT_MIN, CANVAS_BGS};
use crate::style::{border_all_color, radius, reactive_style, static_style, styled};
use crate::{BoardState, REC_FILE, REC_STORE};
use idea_ui::{typography_kind, SegmentOption, SegmentedControl, Switch, Typography};
use icons_lucide::X;
use runtime_core::{
    component, icon, safe_area_insets, ui, view, viewport_size, AlignItems, ChildList, Color,
    Element, FlexDirection, FontWeight, IntoElement, JustifyContent, Length, Overflow, Ref,
    Signal, StyleRules, Tokenized, TouchPhase, TouchResponse,
};
use stack_navigator::StackHandle;
use std::rc::Rc;

/// Resolve a theme color NOW (light/dark) — call inside a `reactive_style` so it
/// re-resolves when the active theme swaps. Leverages idea-ui's token system, so
/// the surface follows light/dark with no per-color plumbing.
fn tc(getter: impl Fn(&idea_ui::Colors) -> Tokenized<Color> + 'static) -> Color {
    crate::style::token(getter)
}

// ============================================================================
// Per-text color (replaces the old `colored_text`)
// ============================================================================

/// Props for [`Label`].
pub struct LabelProps {
    pub text: &'static str,
    pub hex: &'static str,
    pub weight: FontWeight,
    pub size: f32,
}

impl Default for LabelProps {
    fn default() -> Self {
        Self { text: "", hex: "#111827", weight: FontWeight::Normal, size: 15.0 }
    }
}

/// A `text` node carrying its OWN color (+ weight/size).
///
/// Native backends (macOS / iOS) do NOT inherit text color from an ancestor
/// view's `color` the way web/CSS does — each `NSTextField`/`UILabel` keeps its
/// own color and otherwise falls back to the SYSTEM default label color, which is
/// white in dark mode. So every piece of on-screen copy sets color on the text
/// node itself; a `color` on the wrapping container is a web-only cascade that
/// leaves macOS text invisible (the "white text" settings bug). Setting it
/// explicitly is correct on web too.
#[component]
pub fn Label(props: &LabelProps) -> Element {
    let label = props.text.to_string();
    let style = static_style(StyleRules {
        color: Some(Tokenized::Literal(Color(props.hex.into()))),
        font_weight: Some(props.weight),
        font_size: Some(Length::Px(props.size).into()),
        ..Default::default()
    });
    ui! {
        text(style = style) { label }
    }
}

// ============================================================================
// Shared screen scaffold
// ============================================================================

/// Props for [`ScreenScaffold`]. `children` are the screen's content, splatted
/// under the in-content header.
pub struct ScreenScaffoldProps {
    pub title: &'static str,
    pub nav: Ref<StackHandle>,
    /// Screen content. Incoming fragments are flattened via
    /// `ChildList::append_to` before rendering under the header.
    pub children: Vec<Element>,
}

impl Default for ScreenScaffoldProps {
    fn default() -> Self {
        Self { title: "", nav: Ref::new(), children: Vec::new() }
    }
}

/// A navigator-screen scaffold: a full-bleed column on the light surface with
/// safe-area padding, an in-content header (title + a back `×` that pops the
/// stack), then `children`. Reactive style is fine here: navigator screens are
/// normal host-tree content (not the screen-recorder's detached overlay window).
#[component(children)]
pub fn ScreenScaffold(props: ScreenScaffoldProps) -> Element {
    let title = props.title;
    let nav = props.nav;
    let mut body: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut body);
    }

    let style = reactive_style(move || {
        let ins = safe_area_insets().get();
        StyleRules {
            width: Some(Length::pct(100.0).into()),
            height: Some(Length::pct(100.0).into()),
            flex_direction: Some(FlexDirection::Column),
            padding_top: Some(Length::Px(12.0 + ins.top).into()),
            padding_bottom: Some(Length::Px(ins.bottom).into()),
            // Theme background — follows light/dark.
            background: Some(Tokenized::Literal(tc(|c| c.background.clone()))),
            // Web-only cascade fallback; native text sets its own color.
            color: Some(Tokenized::Literal(tc(|c| c.text.clone()))),
            ..Default::default()
        }
    });
    ui! {
        view(style = style) {
            ScreenHeader(title = title, nav = nav)
            body
        }
    }
}

/// Props for [`ScreenHeader`].
pub struct ScreenHeaderProps {
    pub title: &'static str,
    pub nav: Ref<StackHandle>,
}

impl Default for ScreenHeaderProps {
    fn default() -> Self {
        Self { title: "", nav: Ref::new() }
    }
}

/// A screen header: a title + a close (×) button that pops the stack.
#[component]
pub fn ScreenHeader(props: &ScreenHeaderProps) -> Element {
    let title = props.title;
    let nav = props.nav;

    let close_style = reactive_style(|| {
        styled(
            StyleRules {
                width: Some(Length::Px(40.0).into()),
                height: Some(Length::Px(40.0).into()),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                background: Some(Tokenized::Literal(tc(|c| c.surface_alt.clone()))),
                ..Default::default()
            },
            [radius(20.0)],
        )
    });
    let glyph_style = static_style(StyleRules {
        width: Some(Length::Px(22.0).into()),
        height: Some(Length::Px(22.0).into()),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });
    let glyph = icon(X)
        .color(|| tc(|c| c.text_muted.clone()))
        .into_element();
    let row_style = static_style(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::SpaceBetween),
        padding_left: Some(Length::Px(20.0).into()),
        padding_right: Some(Length::Px(16.0).into()),
        padding_top: Some(Length::Px(8.0).into()),
        padding_bottom: Some(Length::Px(12.0).into()),
        ..Default::default()
    });

    ui! {
        view(style = row_style) {
            Typography(content = title, kind = typography_kind::H2)
            view(style = close_style) {
                view(style = glyph_style) { glyph }
            }
            .on_touch(move |ev| {
                if ev.phase == TouchPhase::Ended {
                    if let Some(h) = nav.get() {
                        h.pop();
                    }
                }
                TouchResponse::CONSUMED
            })
        }
    }
}

// ============================================================================
// Settings screen
// ============================================================================

/// Props for [`SettingsScreen`].
pub struct SettingsScreenProps {
    pub state: BoardState,
}

impl Default for SettingsScreenProps {
    fn default() -> Self {
        Self { state: BoardState::default() }
    }
}

/// A surface "card" for a settings section — theme `surface` background, rounded,
/// padded, column-stacked. Reactive so it follows light/dark.
fn card_style() -> impl Fn() -> runtime_core::StyleApplication {
    reactive_style(|| {
        styled(
            StyleRules {
                background: Some(Tokenized::Literal(tc(|c| c.surface.clone()))),
                flex_direction: Some(FlexDirection::Column),
                gap: Some(Length::Px(12.0).into()),
                padding_top: Some(Length::Px(14.0).into()),
                padding_bottom: Some(Length::Px(14.0).into()),
                padding_left: Some(Length::Px(14.0).into()),
                padding_right: Some(Length::Px(14.0).into()),
                ..Default::default()
            },
            [radius(14.0)],
        )
    })
}

/// The Settings screen: real, wired controls — aspect ratio (presets + custom),
/// canvas color (with `Auto`), and a light/dark switch. All idea-ui
/// token-driven, so the screen itself flips with the theme.
#[component]
pub fn SettingsScreen(props: &SettingsScreenProps) -> Element {
    let state = props.state;
    let nav = state.nav;
    let aspect = state.aspect;
    let canvas_bg = state.canvas_bg;
    let dark = state.dark;

    // Aspect preset picker. `aspect_sel` mirrors the chosen segment (a preset
    // label or "Custom"); choosing a preset also commits the real aspect.
    let (aw0, ah0) = aspect.get();
    let aspect_sel = Signal::new(aspect_label(aw0, ah0).to_string());
    let aspect_options: Vec<SegmentOption> = ASPECTS
        .iter()
        .map(|(l, _, _)| SegmentOption::new(*l, *l))
        .chain(std::iter::once(SegmentOption::new("Custom", "Custom")))
        .collect();
    let aspect_on_change: Rc<dyn Fn(String)> = Rc::new(move |id: String| {
        aspect_sel.set(id.clone());
        if let Some((_, aw, ah)) = ASPECTS.iter().find(|(l, _, _)| *l == id) {
            aspect.set((*aw, *ah));
        }
        // "Custom" keeps the current aspect; the steppers below adjust it.
    });

    let dark_on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| {
        dark.set(v);
    });

    let list_style = static_style(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(Length::Px(14.0).into()),
        padding_left: Some(Length::Px(16.0).into()),
        padding_right: Some(Length::Px(16.0).into()),
        padding_top: Some(Length::Px(6.0).into()),
        ..Default::default()
    });
    let row_style = static_style(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::SpaceBetween),
        ..Default::default()
    });

    ui! {
        ScreenScaffold(title = "Settings", nav = nav) {
            view(style = list_style) {
                view(style = card_style()) {
                    Typography(content = "Aspect ratio", kind = typography_kind::Caption, muted = true)
                    SegmentedControl(value = aspect_sel, on_change = aspect_on_change, options = aspect_options)
                    if aspect_sel.get() == "Custom" {
                        AspectSteppers(aspect = aspect)
                    }
                }
                view(style = card_style()) {
                    Typography(content = "Canvas color", kind = typography_kind::Caption, muted = true)
                    SwatchRow(canvas_bg = canvas_bg)
                }
                view(style = card_style()) {
                    Typography(content = "Appearance", kind = typography_kind::Caption, muted = true)
                    view(style = row_style) {
                        Typography(content = "Dark mode", kind = typography_kind::Body)
                        Switch(value = dark, on_change = dark_on_change)
                    }
                }
            }
        }
    }
}

/// Props for [`SwatchRow`].
pub struct SwatchRowProps {
    pub canvas_bg: Signal<CanvasBg>,
}

impl Default for SwatchRowProps {
    fn default() -> Self {
        Self { canvas_bg: Signal::new(CanvasBg::Auto) }
    }
}

/// A row of tappable color swatches for the canvas background. The selected one
/// gets a thick `text`-token ring; the rest a thin `border` ring. Tapping commits
/// the choice. `Auto` shows a neutral gray (its real color depends on the theme).
#[component]
pub fn SwatchRow(props: &SwatchRowProps) -> Element {
    let canvas_bg = props.canvas_bg;
    let mut swatches: Vec<Element> = Vec::with_capacity(CANVAS_BGS.len());
    for (_label, bg) in CANVAS_BGS {
        let bg = *bg;
        let style = reactive_style(move || {
            let selected = canvas_bg.get() == bg;
            let ring = if selected {
                tc(|c| c.text.clone())
            } else {
                tc(|c| c.border.clone())
            };
            styled(
                StyleRules {
                    width: Some(Length::Px(38.0).into()),
                    height: Some(Length::Px(38.0).into()),
                    background: Some(Tokenized::Literal(Color(bg.swatch_css().into()))),
                    ..Default::default()
                },
                [radius(19.0), border_all_color(if selected { 3.0 } else { 1.0 }, ring)],
            )
        });
        let sw = ui! {
            view(style = style) {}
            .on_touch(move |ev| {
                if ev.phase == TouchPhase::Ended {
                    canvas_bg.set(bg);
                }
                TouchResponse::CONSUMED
            })
        };
        swatches.push(sw);
    }
    let row = static_style(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(Length::Px(12.0).into()),
        ..Default::default()
    });
    ui! { view(style = row) { swatches } }
}

/// Props for [`AspectSteppers`].
pub struct AspectStepperProps {
    pub aspect: Signal<(u32, u32)>,
}

impl Default for AspectStepperProps {
    fn default() -> Self {
        Self { aspect: Signal::new(crate::settings::DEFAULT_ASPECT) }
    }
}

/// Width/height steppers for a CUSTOM aspect ratio, bounded to
/// `[ASPECT_MIN, ASPECT_MAX]`. Two rows, each `label  −  value  +`.
#[component]
pub fn AspectSteppers(props: &AspectStepperProps) -> Element {
    let aspect = props.aspect;
    ui! {
        view(style = aspect_steppers_col()) {
            StepperRow(label = "Width", aspect = aspect, is_width = true)
            StepperRow(label = "Height", aspect = aspect, is_width = false)
        }
    }
}

fn aspect_steppers_col() -> Rc<runtime_core::StyleSheet> {
    static_style(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(Length::Px(8.0).into()),
        padding_top: Some(Length::Px(4.0).into()),
        ..Default::default()
    })
}

/// Props for [`StepperRow`].
pub struct StepperRowProps {
    pub label: &'static str,
    pub aspect: Signal<(u32, u32)>,
    pub is_width: bool,
}

impl Default for StepperRowProps {
    fn default() -> Self {
        Self { label: "", aspect: Signal::new(crate::settings::DEFAULT_ASPECT), is_width: true }
    }
}

/// One stepper row: `label  [−] value [+]`, clamped to the aspect bounds.
#[component]
pub fn StepperRow(props: &StepperRowProps) -> Element {
    let label = props.label;
    let aspect = props.aspect;
    let is_width = props.is_width;

    // `aspect` (Signal) + `is_width` (bool) are Copy, so each closure captures
    // them independently — no shared non-Copy state to move.
    let dec = move || {
        let (w, h) = aspect.get();
        let cur = if is_width { w } else { h };
        let v = cur.saturating_sub(1).max(ASPECT_MIN);
        aspect.set(if is_width { (v, h) } else { (w, v) });
    };
    let inc = move || {
        let (w, h) = aspect.get();
        let cur = if is_width { w } else { h };
        let v = (cur + 1).min(ASPECT_MAX);
        aspect.set(if is_width { (v, h) } else { (w, v) });
    };

    let row_style = static_style(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::SpaceBetween),
        ..Default::default()
    });
    let controls_style = static_style(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(Length::Px(14.0).into()),
        ..Default::default()
    });
    // Derived reactive label for the value — a `Signal<String>` so Typography's
    // `content` (Reactive<String>) gets the live value via `From<Signal>`.
    let value_sig: Signal<String> = Signal::new(String::new());
    runtime_core::effect!({
        let (w, h) = aspect.get();
        value_sig.set((if is_width { w } else { h }).to_string());
    });

    ui! {
        view(style = row_style) {
            Typography(content = label, kind = typography_kind::Body)
            view(style = controls_style) {
                stepper_btn("\u{2212}", dec)
                Typography(content = value_sig, kind = typography_kind::Body)
                stepper_btn("+", inc)
            }
        }
    }
}

/// A round `−`/`+` stepper button: `surface_alt` fill, `text` glyph.
fn stepper_btn(glyph: &'static str, on_press: impl Fn() + 'static) -> Element {
    let style = reactive_style(|| {
        styled(
            StyleRules {
                width: Some(Length::Px(32.0).into()),
                height: Some(Length::Px(32.0).into()),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                background: Some(Tokenized::Literal(tc(|c| c.surface_alt.clone()))),
                ..Default::default()
            },
            [radius(16.0)],
        )
    });
    ui! {
        view(style = style) {
            Typography(content = glyph, kind = typography_kind::Body)
        }
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                on_press();
            }
            TouchResponse::CONSUMED
        })
    }
    .into_element()
}

// ============================================================================
// Preview / export screen
// ============================================================================

/// Props for [`PreviewScreen`].
pub struct PreviewScreenProps {
    pub rec_path: Signal<Option<String>>,
    pub playback_url: Signal<String>,
    pub nav: Ref<StackHandle>,
}

impl Default for PreviewScreenProps {
    fn default() -> Self {
        Self {
            rec_path: Signal::new(None),
            playback_url: Signal::new(String::new()),
            nav: Ref::new(),
        }
    }
}

/// The Preview / export screen (shown after a recording stops): a looping video
/// stage + Discard / Export actions.
#[component]
pub fn PreviewScreen(props: &PreviewScreenProps) -> Element {
    let rec_path = props.rec_path;
    let playback_url = props.playback_url;
    let nav = props.nav;

    // Resolve a playable URL ASYNCHRONOUSLY into `playback_url` (root-scoped; see
    // `app`), which the video reads reactively. Native → a `file://` to the
    // store's real path; web has no `file://`, so we read the IndexedDB-stored
    // bytes and wrap them in a blob URL (`URL.createObjectURL`). Reset to empty
    // first so a prior recording's frame doesn't flash before this one resolves.
    playback_url.set(String::new());
    runtime_core::driver::spawn_async(async move {
        if let Some(p) = rec_path.get() {
            if let Some(url) = resolve_playback_url(&p).await {
                playback_url.set(url);
            }
        }
    });

    let video_fill = static_style(StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    });
    let stage_video = video::Video(video::VideoProps {
        source: video::url(move || playback_url.get()),
        autoplay: true,
        controls: true,
        loop_playback: true,
        // Cover, not Contain: the stage box is already sized to the recording's
        // aspect, so Cover fills it edge-to-edge (no stage-bg letterbox sliver —
        // the viewport-derived aspect can be ~1px off, which Contain would show
        // as a hairline bar). The crop is sub-pixel given the matched aspect.
        object_fit: video::ObjectFit::Cover,
    })
    .with_style(video_fill)
    .into_element();

    // Outer: fills the space between the header and the actions, and CENTERS the
    // stage box within it (both axes).
    let stage_wrapper_style = static_style(StyleRules {
        flex_grow: Some(Tokenized::Literal(1.0)),
        margin_left: Some(Length::Px(20.0).into()),
        margin_right: Some(Length::Px(20.0).into()),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });
    // Inner stage: takes the RECORDED clip's aspect ratio so the video fills it
    // edge-to-edge with NO letterbox. The recording is the full-screen canvas
    // MINUS the system bars (status + nav) — which the safe-area insets cover —
    // so `width / (height - insets)` matches the recorded frame's aspect closely.
    // Height-bound to the wrapper; `max_width: 100%` clamps the rare wide clip.
    let stage_box_style = reactive_style(move || {
        let vp = viewport_size().get();
        let ins = safe_area_insets().get();
        let content_h = (vp.height - ins.top - ins.bottom).max(1.0);
        let aspect = (vp.width / content_h).max(0.05);
        styled(
            StyleRules {
                height: Some(Length::pct(100.0).into()),
                aspect_ratio: Some(aspect),
                max_width: Some(Length::pct(100.0).into()),
                background: Some(Tokenized::Literal(Color("#0b1220".into()))),
                overflow: Some(Overflow::Hidden),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                ..Default::default()
            },
            [radius(16.0)],
        )
    });
    let actions_style = static_style(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        gap: Some(Length::Px(12.0).into()),
        justify_content: Some(JustifyContent::Center),
        padding_top: Some(Length::Px(16.0).into()),
        padding_bottom: Some(Length::Px(20.0).into()),
        padding_left: Some(Length::Px(20.0).into()),
        padding_right: Some(Length::Px(20.0).into()),
        ..Default::default()
    });

    // Actions: Discard (delete + pop back to the board) · Export (save via the
    // native picker; stays on the screen).
    let on_discard: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(p) = rec_path.get() {
            runtime_core::driver::spawn_async(async move {
                if let Ok(store) = files::app_files(REC_STORE) {
                    let _ = store.delete(&p).await;
                }
            });
        }
        rec_path.set(None);
        if let Some(h) = nav.get() {
            h.pop();
        }
    });
    let on_export: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(p) = rec_path.get() {
            runtime_core::driver::spawn_async(async move {
                if let Ok(store) = files::app_files(REC_STORE) {
                    if let Ok(Some(bytes)) = store.read(&p).await {
                        let req = file_export::SaveRequest::bytes(REC_FILE, "video/mp4", bytes);
                        let _ = file_export::FileExport::new().save(req).await;
                    }
                }
            });
        }
    });

    ui! {
        ScreenScaffold(title = "Recording", nav = nav) {
            view(style = stage_wrapper_style) {
                view(style = stage_box_style) {
                    stage_video
                }
            }
            view(style = actions_style) {
                ActionButton(label = "Discard", primary = false, on_press = on_discard)
                ActionButton(label = "Export", primary = true, on_press = on_export)
            }
        }
    }
}

/// Props for [`ActionButton`].
pub struct ActionButtonProps {
    pub label: &'static str,
    pub primary: bool,
    pub on_press: Option<Rc<dyn Fn()>>,
}

impl Default for ActionButtonProps {
    fn default() -> Self {
        Self { label: "", primary: false, on_press: None }
    }
}

/// A labeled action button. `primary` → filled blue; else neutral.
#[component]
pub fn ActionButton(props: &ActionButtonProps) -> Element {
    let label = props.label;
    let (bg, fg) = if props.primary {
        ("#2563eb", "#ffffff")
    } else {
        ("rgba(17,24,39,0.06)", "#111827")
    };
    let on_press = props.on_press.clone();

    let btn_style = static_style(styled(
        StyleRules {
            height: Some(Length::Px(46.0).into()),
            padding_left: Some(Length::Px(28.0).into()),
            padding_right: Some(Length::Px(28.0).into()),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            background: Some(Tokenized::Literal(Color(bg.into()))),
            ..Default::default()
        },
        [radius(23.0)],
    ));

    // Conditional callback (§9.6): bind `on_touch` only when a handler is
    // present. The label child is `ui!`-composed; the touch-carrying wrapper uses
    // the `view()` builder so the bind can be applied conditionally.
    let inner = ui! {
        Label(text = label, hex = fg, weight = FontWeight::SemiBold, size = 15.0)
    };
    let mut bound = view(vec![inner]).with_style(btn_style);
    if let Some(cb) = on_press {
        bound = bound.on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                (cb)();
            }
            TouchResponse::CONSUMED
        });
    }
    bound.into_element()
}

// ============================================================================
// Playback-URL resolution (native file:// vs web blob URL)
// ============================================================================

/// Resolve a playable URL for a recorded file in the `REC_STORE`.
///
/// Native → a `file://` URL via the store's real on-disk path (synchronous, but
/// `async` for a uniform signature). Web has no filesystem: the file lives as
/// bytes in IndexedDB, so we read them and wrap them in a blob URL
/// (`URL.createObjectURL`) the `<video>` can play.
async fn resolve_playback_url(path: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let store = files::app_files(REC_STORE).ok()?;
        let p = store.local_path(path)?;
        Some(format!("file://{}", p.display()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let store = files::app_files(REC_STORE).ok()?;
        let bytes = match store.read(path).await {
            Ok(Some(b)) => b,
            _ => return None,
        };
        blob_url_from_bytes(&bytes)
    }
}

/// Wrap recorded bytes in an object URL the browser `<video>` can load. The
/// container is browser-chosen (Chromium → WebM, Safari → MP4); we sniff `ftyp`
/// to label MP4, else assume WebM, so the media element picks the right decoder.
///
/// The URL is intentionally not revoked — a demo records occasionally and the
/// blob is released on page reload; a production app would `revokeObjectURL` when
/// the preview unmounts.
#[cfg(target_arch = "wasm32")]
fn blob_url_from_bytes(bytes: &[u8]) -> Option<String> {
    let arr = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&arr);
    let opts = web_sys::BlobPropertyBag::new();
    let mime = if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        "video/mp4"
    } else {
        "video/webm"
    };
    opts.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}
