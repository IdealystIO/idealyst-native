//! The navigator-pushed screens (`Settings`, `Preview`) and their shared
//! scaffold. Like the board's chrome, these are normal host-tree content, so
//! reactive style + ordinary components are fine.
//!
//! Every screen carries its OWN in-content header (`ScreenHeader`: title + a back
//! `×` that pops the stack), because `header_shown(false)` is set on every route
//! — the stack handler renders native chrome on iOS / Android / web but NOT on
//! macOS / terminal / SSR, so without the in-content header the user couldn't get
//! back there.

use crate::settings::{
    aspect_label, CameraShape, CameraSize, CanvasBg, ASPECTS, ASPECT_MAX, ASPECT_MIN,
    CAMERA_SHAPES, CAMERA_SIZES, CANVAS_BGS,
};
use crate::style::{border_all_color, radius, reactive_style, static_style, styled};
use crate::{BoardState, CanvasStore, Strokes, REC_FILE, REC_STORE};
use idea_ui::{typography_kind, Modal, SegmentOption, SegmentedControl, Switch, Typography};
use icons_lucide::X;
use runtime_core::{
    component, icon, safe_area_insets, ui, view, AlignItems, ChildList, Color,
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
    /// The active canvas's live strokes — read to detect drawings before an
    /// aspect change, and cleared by the reset on confirm.
    pub strokes: Strokes,
    /// The saved canvas docs — checked for drawings + reset on aspect change.
    pub canvases: CanvasStore,
    /// Repaint tick — bumped by `reset_canvases` so the cleared board repaints.
    pub version: Signal<u64>,
}

impl Default for SettingsScreenProps {
    fn default() -> Self {
        Self {
            state: BoardState::default(),
            strokes: Default::default(),
            canvases: Default::default(),
            version: Signal::new(0),
        }
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
    let strokes = props.strokes.clone();
    let canvases = props.canvases.clone();
    let version = props.version;
    let active_canvas = state.active_canvas;
    let canvas_ids = state.canvas_ids;
    let next_id = state.next_id;

    // Aspect preset picker. `aspect_sel` mirrors the chosen segment (a preset
    // label or "Custom"); choosing a preset routes through `request_aspect`.
    let (aw0, ah0) = aspect.get();
    let aspect_sel = Signal::new(aspect_label(aw0, ah0).to_string());
    let aspect_options: Vec<SegmentOption> = ASPECTS
        .iter()
        .map(|(l, _, _)| SegmentOption::new(*l, *l))
        .chain(std::iter::once(SegmentOption::new("Custom", "Custom")))
        .collect();

    // The aspect-change guard. An aspect change invalidates every canvas's
    // stage-local strokes, so changing it RESETS the whole board to one empty
    // canvas — with a confirmation when any drawing exists. `pending` holds the
    // requested aspect while the confirm modal is up; once confirmed (board
    // cleared) or when the board is already empty, the change applies directly.
    let pending: Signal<Option<(u32, u32)>> = Signal::new(None);
    let request_aspect: Rc<dyn Fn((u32, u32))> = {
        let canvases = canvases.clone();
        let strokes = strokes.clone();
        Rc::new(move |new: (u32, u32)| {
            if new == aspect.get() {
                return;
            }
            if crate::any_drawings(&canvases, &strokes, active_canvas) {
                pending.set(Some(new));
            } else {
                aspect.set(new);
            }
        })
    };
    let aspect_on_change: Rc<dyn Fn(String)> = {
        let request_aspect = request_aspect.clone();
        Rc::new(move |id: String| {
            aspect_sel.set(id.clone());
            if let Some((_, aw, ah)) = ASPECTS.iter().find(|(l, _, _)| *l == id) {
                request_aspect((*aw, *ah));
            }
            // "Custom" keeps the current aspect; the steppers below adjust it.
        })
    };

    // Confirm → clear the board, then apply the pending aspect. Cancel → discard
    // the pending change and snap the segmented selection back to the real aspect.
    let confirm_change: Rc<dyn Fn()> = {
        let canvases = canvases.clone();
        let strokes = strokes.clone();
        Rc::new(move || {
            if let Some(new) = pending.get() {
                crate::reset_canvases(&canvases, &strokes, active_canvas, version, canvas_ids, next_id);
                aspect.set(new);
                aspect_sel.set(aspect_label(new.0, new.1).to_string());
            }
            pending.set(None);
        })
    };
    let cancel_change: Rc<dyn Fn()> = Rc::new(move || {
        pending.set(None);
        let (w, h) = aspect.get();
        aspect_sel.set(aspect_label(w, h).to_string());
    });

    let dark_on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| {
        dark.set(v);
    });

    // Controls toggles: enable/disable the keyboard shortcuts + swipe gesture.
    let keys_enabled = state.keys_enabled;
    let gestures_enabled = state.gestures_enabled;
    let keys_on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| keys_enabled.set(v));
    let gestures_on_change: Rc<dyn Fn(bool)> = Rc::new(move |v| gestures_enabled.set(v));

    // Camera shape + size pickers.
    let camera_shape = state.camera_shape;
    let camera_size = state.camera_size;
    let shape_label = |s: CameraShape| {
        CAMERA_SHAPES.iter().find(|(_, v)| *v == s).map(|(l, _)| *l).unwrap_or("Rounded")
    };
    let size_label = |s: CameraSize| {
        CAMERA_SIZES.iter().find(|(_, v)| *v == s).map(|(l, _)| *l).unwrap_or("M")
    };
    let shape_sel = Signal::new(shape_label(camera_shape.get()).to_string());
    let size_sel = Signal::new(size_label(camera_size.get()).to_string());
    let shape_options: Vec<SegmentOption> =
        CAMERA_SHAPES.iter().map(|(l, _)| SegmentOption::new(*l, *l)).collect();
    let size_options: Vec<SegmentOption> =
        CAMERA_SIZES.iter().map(|(l, _)| SegmentOption::new(*l, *l)).collect();
    let shape_on_change: Rc<dyn Fn(String)> = Rc::new(move |id: String| {
        shape_sel.set(id.clone());
        if let Some((_, v)) = CAMERA_SHAPES.iter().find(|(l, _)| *l == id) {
            camera_shape.set(*v);
        }
    });
    let size_on_change: Rc<dyn Fn(String)> = Rc::new(move |id: String| {
        size_sel.set(id.clone());
        if let Some((_, v)) = CAMERA_SIZES.iter().find(|(l, _)| *l == id) {
            camera_size.set(*v);
        }
    });

    // Show the CURRENT selection in each section caption — the segmented/​swatch
    // highlights can read subtly, so this makes the chosen value unmistakable and
    // updates live as the user picks.
    let aspect_caption = Signal::new(String::new());
    runtime_core::effect!({
        let (w, h) = aspect.get();
        aspect_caption.set(format!("Aspect ratio · {}", aspect_label(w, h)));
    });
    let color_caption = Signal::new(String::new());
    runtime_core::effect!({
        let cur = canvas_bg.get();
        let lbl = CANVAS_BGS
            .iter()
            .find(|(_, b)| *b == cur)
            .map(|(l, _)| *l)
            .unwrap_or("Auto");
        color_caption.set(format!("Canvas color · {lbl}"));
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

    let dialog_actions_style = static_style(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        gap: Some(Length::Px(10.0).into()),
        justify_content: Some(JustifyContent::FlexEnd),
        padding_top: Some(Length::Px(6.0).into()),
        ..Default::default()
    });

    ui! {
        ScreenScaffold(title = "Settings", nav = nav) {
            view(style = list_style) {
                view(style = card_style()) {
                    Typography(content = aspect_caption, kind = typography_kind::Caption, muted = true)
                    SegmentedControl(value = aspect_sel, on_change = aspect_on_change, options = aspect_options)
                    if aspect_sel.get() == "Custom" {
                        AspectSteppers(aspect = aspect, on_set = request_aspect.clone())
                    }
                }
                view(style = card_style()) {
                    Typography(content = color_caption, kind = typography_kind::Caption, muted = true)
                    SwatchRow(canvas_bg = canvas_bg)
                }
                view(style = card_style()) {
                    Typography(content = "Appearance", kind = typography_kind::Caption, muted = true)
                    view(style = row_style.clone()) {
                        Typography(content = "Dark mode", kind = typography_kind::Body)
                        Switch(value = dark, on_change = dark_on_change)
                    }
                }
                view(style = card_style()) {
                    Typography(content = "Camera shape", kind = typography_kind::Caption, muted = true)
                    SegmentedControl(value = shape_sel, on_change = shape_on_change, options = shape_options)
                    Typography(content = "Camera size", kind = typography_kind::Caption, muted = true)
                    SegmentedControl(value = size_sel, on_change = size_on_change, options = size_options)
                }
                view(style = card_style()) {
                    Typography(content = "Controls", kind = typography_kind::Caption, muted = true)
                    view(style = row_style.clone()) {
                        Typography(content = "Keyboard shortcuts", kind = typography_kind::Body)
                        Switch(value = keys_enabled, on_change = keys_on_change)
                    }
                    view(style = row_style.clone()) {
                        Typography(content = "Swipe gestures", kind = typography_kind::Body)
                        Switch(value = gestures_enabled, on_change = gestures_on_change)
                    }
                }
            }
            // Confirm clearing the board before an aspect change wipes drawings.
            if pending.get().is_some() {
                Modal(on_dismiss = Some(cancel_change.clone())) {
                    Typography(content = "Change aspect ratio?", kind = typography_kind::H3)
                    Typography(content = "A new aspect ratio starts a fresh board — every canvas and its drawings will be cleared. This can't be undone.", muted = true)
                    view(style = dialog_actions_style.clone()) {
                        ActionButton(label = "Cancel", primary = false, on_press = Some(cancel_change.clone()))
                        ActionButton(label = "Clear & change", primary = true, on_press = Some(confirm_change.clone()))
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

/// Props for [`Swatch`].
pub struct SwatchProps {
    /// This chip's canvas background.
    pub bg: CanvasBg,
    /// The shared selected-background signal — tapping commits `bg`, and the
    /// chip lights when `canvas_bg == bg`.
    pub canvas_bg: Signal<CanvasBg>,
}

impl Default for SwatchProps {
    fn default() -> Self {
        Self { bg: CanvasBg::Auto, canvas_bg: Signal::new(CanvasBg::Auto) }
    }
}

/// One tappable canvas-background chip. Selected GROWS and gets a thick accent
/// ring (a size change reads clearly even where a thin border doesn't); the rest
/// stay small with a hairline border. `Auto` carries an "A" so it's identifiable
/// at a glance (its swatch color is a neutral placeholder, not its theme color).
#[component]
pub fn Swatch(props: &SwatchProps) -> Element {
    let bg = props.bg;
    let canvas_bg = props.canvas_bg;
    let is_auto = matches!(bg, CanvasBg::Auto);
    let style = reactive_style(move || {
        let selected = canvas_bg.get() == bg;
        let d = if selected { 46.0 } else { 34.0 };
        let ring = if selected { tc(|c| c.text.clone()) } else { tc(|c| c.border.clone()) };
        styled(
            StyleRules {
                width: Some(Length::Px(d).into()),
                height: Some(Length::Px(d).into()),
                background: Some(Tokenized::Literal(Color(bg.swatch_css().into()))),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                ..Default::default()
            },
            [radius(d / 2.0), border_all_color(if selected { 3.0 } else { 1.0 }, ring)],
        )
    });
    let a_style = static_style(StyleRules {
        color: Some(Tokenized::Literal(Color("#1f2937".into()))),
        font_weight: Some(FontWeight::Bold),
        font_size: Some(Length::Px(16.0).into()),
        ..Default::default()
    });
    ui! {
        view(style = style) {
            if is_auto {
                text(style = a_style) { "A".to_string() }
            }
        }
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                canvas_bg.set(bg);
            }
            TouchResponse::CONSUMED
        })
    }
}

/// A row of tappable canvas-background [`Swatch`]es.
#[component]
pub fn SwatchRow(props: &SwatchRowProps) -> Element {
    let canvas_bg = props.canvas_bg;
    let row = static_style(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(Length::Px(12.0).into()),
        ..Default::default()
    });
    ui! {
        view(style = row) {
            for (_label, bg) in CANVAS_BGS {
                Swatch(bg = *bg, canvas_bg = canvas_bg)
            }
        }
    }
}

/// Props for [`AspectSteppers`].
pub struct AspectStepperProps {
    pub aspect: Signal<(u32, u32)>,
    /// The guarded aspect setter — `±` route the requested ratio through it so a
    /// custom change clears the board (with confirmation) like a preset change.
    pub on_set: Rc<dyn Fn((u32, u32))>,
}

impl Default for AspectStepperProps {
    fn default() -> Self {
        Self {
            aspect: Signal::new(crate::settings::DEFAULT_ASPECT),
            on_set: Rc::new(|_| {}),
        }
    }
}

/// Width/height steppers for a CUSTOM aspect ratio, bounded to
/// `[ASPECT_MIN, ASPECT_MAX]`. Two rows, each `label  −  value  +`.
#[component]
pub fn AspectSteppers(props: &AspectStepperProps) -> Element {
    let aspect = props.aspect;
    let on_set = props.on_set.clone();
    ui! {
        view(style = aspect_steppers_col()) {
            StepperRow(label = "Width", aspect = aspect, is_width = true, on_set = on_set.clone())
            StepperRow(label = "Height", aspect = aspect, is_width = false, on_set = on_set.clone())
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
    /// Guarded aspect setter — `±` request the new ratio through it (so a custom
    /// change clears the board with confirmation, like a preset change).
    pub on_set: Rc<dyn Fn((u32, u32))>,
}

impl Default for StepperRowProps {
    fn default() -> Self {
        Self {
            label: "",
            aspect: Signal::new(crate::settings::DEFAULT_ASPECT),
            is_width: true,
            on_set: Rc::new(|_| {}),
        }
    }
}

/// One stepper row: `label  [−] value [+]`, clamped to the aspect bounds.
#[component]
pub fn StepperRow(props: &StepperRowProps) -> Element {
    let label = props.label;
    let aspect = props.aspect;
    let is_width = props.is_width;

    // `±` request the new ratio through the guarded setter, so changing a custom
    // dimension clears the board (with confirmation) just like a preset change.
    let dec = {
        let on_set = props.on_set.clone();
        move || {
            let (w, h) = aspect.get();
            let cur = if is_width { w } else { h };
            let v = cur.saturating_sub(1).max(ASPECT_MIN);
            on_set(if is_width { (v, h) } else { (w, v) });
        }
    };
    let inc = {
        let on_set = props.on_set.clone();
        move || {
            let (w, h) = aspect.get();
            let cur = if is_width { w } else { h };
            let v = (cur + 1).min(ASPECT_MAX);
            on_set(if is_width { (v, h) } else { (w, v) });
        }
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
    /// The board aspect the clip was recorded at — the preview stage takes this
    /// ratio so the video fills it with no letterbox/crop.
    pub aspect: Signal<(u32, u32)>,
    pub nav: Ref<StackHandle>,
}

impl Default for PreviewScreenProps {
    fn default() -> Self {
        Self {
            rec_path: Signal::new(None),
            playback_url: Signal::new(String::new()),
            aspect: Signal::new((9, 16)),
            nav: Ref::new(),
        }
    }
}

/// The Preview / export screen (shown after a recording stops): a looping video
/// stage + Discard / Export actions.
#[component]
pub fn PreviewScreen(props: &PreviewScreenProps) -> Element {
    let rec_path = props.rec_path;
    let preview_aspect = props.aspect;
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
    // edge-to-edge with NO letterbox. The recording IS the board stage (an
    // aspect-locked box), so its frame aspect is exactly the board `aspect` —
    // `width / height` of the chosen ratio. Height-bound to the wrapper;
    // `max_width: 100%` clamps a wide (landscape) clip to the available width.
    let stage_box_style = reactive_style(move || {
        let (aw, ah) = preview_aspect.get();
        let aspect = (aw.max(1) as f32 / ah.max(1) as f32).max(0.05);
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

/// A labeled action button. `primary` → filled accent; else a neutral surface
/// chip with a border. Theme-aware (reactive tokens) so the neutral variant stays
/// visible in dark mode — the old hardcoded dark-on-dark colors vanished there.
#[component]
pub fn ActionButton(props: &ActionButtonProps) -> Element {
    let label = props.label;
    let primary = props.primary;
    let on_press = props.on_press.clone();

    let btn_style = reactive_style(move || {
        let bg = if primary {
            crate::style::token_intent(|i| i.primary.solid_bg.clone())
        } else {
            tc(|c| c.surface_alt.clone())
        };
        let mut extras = vec![radius(23.0)];
        if !primary {
            // A border gives the neutral chip a visible edge on any backdrop.
            extras.push(border_all_color(1.0, tc(|c| c.border.clone())));
        }
        styled(
            StyleRules {
                height: Some(Length::Px(46.0).into()),
                padding_left: Some(Length::Px(28.0).into()),
                padding_right: Some(Length::Px(28.0).into()),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                background: Some(Tokenized::Literal(bg)),
                ..Default::default()
            },
            extras,
        )
    });
    // Label color on the text node (native doesn't inherit), reactive on theme.
    let label_style = reactive_style(move || StyleRules {
        color: Some(Tokenized::Literal(if primary {
            crate::style::token_intent(|i| i.primary.solid_text.clone())
        } else {
            tc(|c| c.text.clone())
        })),
        font_weight: Some(FontWeight::SemiBold),
        font_size: Some(Length::Px(15.0).into()),
        ..Default::default()
    });

    // Conditional callback (§9.6): bind `on_touch` only when a handler is
    // present. The label child is `ui!`-composed; the touch-carrying wrapper uses
    // the `view()` builder so the bind can be applied conditionally.
    let inner = ui! {
        text(style = label_style) { label.to_string() }
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

/// Resolve a playable URL for a recorded file in the `REC_STORE`. The
/// file:// (native) vs blob: (web) split lives in the `files` SDK's
/// [`loadable_url`](files::FileStore::loadable_url) — no per-platform code here.
async fn resolve_playback_url(path: &str) -> Option<String> {
    let store = files::app_files(REC_STORE).ok()?;
    store.loadable_url(path).await.ok().flatten()
}
