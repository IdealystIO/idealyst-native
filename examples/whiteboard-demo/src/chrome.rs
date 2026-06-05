//! The board's floating, capture-excluded chrome: tool rail, color palette
//! popover, record dock + REC pill, and the settings FAB. Every piece is a
//! [`screen_recorder::PrivateLayer`] child — an individually positioned,
//! passthrough overlay so empty areas keep passing touches to the canvas.
//!
//! Each dock keeps its POSITIONED wrapper mounted (so its inset resolves against
//! the full window) and gates only its CONTENT via [`focus_gate`] — the
//! instant-hide presence that vanishes the chrome the same turn a screen is
//! pushed, so the always-on-top capture-excluded window can't float over the
//! pushed screen. Settings / REC / palette additionally nest an inner
//! `presence` that animates their own state toggle (open, recording, …).

use crate::style::{border_all, focus_gate, radius, reactive_style, static_style, styled};
use crate::{
    BoardState, CanvasCapture, RecHandle, Strokes, PALETTE, PREVIEW, REC_FILE, REC_STORE, SETTINGS,
    WIDTH_MEDIUM, WIDTH_THICK, WIDTH_THIN,
};
use camera::{Camera, CameraConfig, CameraFacing, MediaStream};
use icons_lucide::{CAMERA, SETTINGS as ICON_SETTINGS, TRASH_2};
use runtime_core::{
    component, icon, presence, safe_area_insets, ui, AlignItems, Color, Easing, Element,
    FlexDirection, FlexWrap, IntoElement, JustifyContent, Length, Position, PresenceAnim,
    PresenceState, Ref, Signal, StyleRules, Tokenized, TouchPhase, TouchResponse,
};
use stack_navigator::StackHandle;
use std::rc::Rc;

use crate::{RAIL_EDGE, TOOL_BTN};

/// Build the board's floating chrome as the `PrivateLayer`'s children, in the
/// SAME paint order: `[rec_indicator, palette, tool_rail, rec_dock,
/// settings_btn]`. A plain `fn` (not a component): `BoardScreen` splices the
/// returned `Vec<Element>` straight into `PrivateLayer(..)`.
pub fn build_chrome(
    focused: Rc<dyn Fn() -> bool>,
    s: BoardState,
    strokes: Strokes,
    rec_handle: RecHandle,
    version: Signal<u64>,
    capture: CanvasCapture,
) -> Vec<Element> {
    let rec_indicator = ui! {
        RecIndicator(focused = focused.clone(), recording = s.recording)
    };
    let palette = ui! {
        PalettePopover(
            focused = focused.clone(),
            color_css = s.color_css,
            palette_open = s.palette_open,
        )
    };
    let tool_rail = ui! {
        ToolRail(focused = focused.clone(), state = s, strokes = strokes, version = version)
    };
    let rec_dock = ui! {
        RecordDock(
            focused = focused.clone(),
            state = s,
            rec_handle = rec_handle,
            capture = capture,
            version = version,
        )
    };
    let settings_btn = ui! {
        SettingsFab(focused = focused, recording = s.recording, nav = s.nav)
    };
    vec![rec_indicator, palette, tool_rail, rec_dock, settings_btn]
}

// ============================================================================
// Shared chrome helpers
// ============================================================================

/// Position a child vertically centered against the right edge, inset by the
/// safe area. The dock fills the screen but only lays the child out at center-
/// right; the empty area passes touches through (it has no background).
fn dock_right(child: Element) -> Element {
    ui! {
        view(style = reactive_style(move || {
            let ins = safe_area_insets().get();
            StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(0.0).into()),
                bottom: Some(Length::Px(0.0).into()),
                right: Some(Length::Px(RAIL_EDGE + ins.right).into()),
                flex_direction: Some(FlexDirection::Column),
                justify_content: Some(JustifyContent::Center),
                align_items: Some(AlignItems::FlexEnd),
                ..Default::default()
            }
        })) {
            child
        }
    }
}

/// A bare `TOOL_BTN`-sized tap target — no background, content centered. The
/// icon/shape inside is the whole affordance.
fn bare_btn(content: Element, on_press: impl Fn() + 'static) -> Element {
    let style = static_style(StyleRules {
        width: Some(Length::Px(TOOL_BTN).into()),
        height: Some(Length::Px(TOOL_BTN).into()),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });
    ui! {
        view(style = style) {
            content
        }
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                on_press();
            }
            TouchResponse::CONSUMED
        })
    }
}

/// Wrap an `icon(...)` element so it renders at a consistent 22×22 box.
fn icon_box(el: Element) -> Element {
    let style = static_style(StyleRules {
        width: Some(Length::Px(22.0).into()),
        height: Some(Length::Px(22.0).into()),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });
    ui! {
        view(style = style) {
            el
        }
    }
}

// ============================================================================
// Tool rail (right edge)
// ============================================================================

/// Props for [`ToolRail`]. Bundles the whole [`BoardState`] (the rail reads
/// width / color / camera / palette) plus the shared stroke list + repaint tick
/// (the clear button mutates them).
pub struct ToolRailProps {
    pub focused: Rc<dyn Fn() -> bool>,
    pub state: BoardState,
    pub strokes: Strokes,
    pub version: Signal<u64>,
}

impl Default for ToolRailProps {
    fn default() -> Self {
        Self {
            focused: Rc::new(|| true),
            state: BoardState::default(),
            strokes: Default::default(),
            version: Signal::new(0),
        }
    }
}

/// The floating tool rail: bare icon/shape buttons on a soft frosted pill,
/// docked center-right. Always visible (no collapse) while the board is focused;
/// the content is `focus_gate`d so it unmounts when a screen is pushed, while the
/// `dock_right` wrapper (positioned, transparent, passthrough) stays so the empty
/// area keeps passing touches to the canvas.
#[component]
pub fn ToolRail(props: &ToolRailProps) -> Element {
    let focused = props.focused.clone();
    let s = props.state;
    let strokes = props.strokes.clone();
    let version = props.version;

    let pill = focus_gate(focused, move || {
        let pill_style = static_style(styled(
            StyleRules {
                flex_direction: Some(FlexDirection::Column),
                align_items: Some(AlignItems::Center),
                gap: Some(Length::Px(2.0).into()),
                padding_top: Some(Length::Px(8.0).into()),
                padding_bottom: Some(Length::Px(8.0).into()),
                padding_left: Some(Length::Px(6.0).into()),
                padding_right: Some(Length::Px(6.0).into()),
                background: Some(Tokenized::Literal(Color("rgba(255,255,255,0.92)".into()))),
                ..Default::default()
            },
            [radius(24.0), border_all(1.0, "rgba(17,24,39,0.08)")],
        ));
        ui! {
            view(style = pill_style) {
                WidthButton(w = WIDTH_THIN, width = s.width)
                WidthButton(w = WIDTH_MEDIUM, width = s.width)
                WidthButton(w = WIDTH_THICK, width = s.width)
                RailDivider()
                ColorButton(color_css = s.color_css, palette_open = s.palette_open)
                ClearButton(strokes = strokes.clone(), version = version)
                RailDivider()
                CameraToggle(cam_on = s.cam_on, cam_stream = s.cam_stream)
            }
        }
    });

    dock_right(pill)
}

/// A horizontal divider inside the vertical rail.
#[component]
pub fn RailDivider() -> Element {
    let style = static_style(StyleRules {
        width: Some(Length::Px(24.0).into()),
        height: Some(Length::Px(1.0).into()),
        background: Some(Tokenized::Literal(Color("rgba(17,24,39,0.12)".into()))),
        ..Default::default()
    });
    ui! { view(style = style) {} }
}

/// Props for [`WidthButton`].
pub struct WidthButtonProps {
    pub w: f32,
    pub width: Signal<f32>,
}

impl Default for WidthButtonProps {
    fn default() -> Self {
        Self { w: WIDTH_MEDIUM, width: Signal::new(WIDTH_MEDIUM) }
    }
}

/// A stroke-width button: a bare filled dot whose size tracks the stroke width
/// it sets. Accent-blue when selected, muted grey otherwise — color, not a
/// background box, carries the state.
#[component]
pub fn WidthButton(props: &WidthButtonProps) -> Element {
    let w = props.w;
    let width = props.width;
    let dot_style = reactive_style(move || {
        let selected = (width.get() - w).abs() < f32::EPSILON;
        let d = 6.0 + w; // dot grows with the stroke width it represents
        styled(
            StyleRules {
                width: Some(Length::Px(d).into()),
                height: Some(Length::Px(d).into()),
                background: Some(Tokenized::Literal(Color(
                    if selected { "#2563eb" } else { "#9ca3af" }.to_string(),
                ))),
                ..Default::default()
            },
            [radius(d / 2.0)],
        )
    });
    let dot = ui! { view(style = dot_style) {} };
    bare_btn(dot, move || width.set(w))
}

/// Props for [`ColorButton`].
pub struct ColorButtonProps {
    pub color_css: Signal<&'static str>,
    pub palette_open: Signal<bool>,
}

impl Default for ColorButtonProps {
    fn default() -> Self {
        Self {
            color_css: Signal::new(PALETTE[0].1),
            palette_open: Signal::new(false),
        }
    }
}

/// The color button: a bare disc of the current color with a thin ring (so a
/// light color still reads on the rail). Tapping toggles the palette popover.
#[component]
pub fn ColorButton(props: &ColorButtonProps) -> Element {
    let color_css = props.color_css;
    let palette_open = props.palette_open;
    let disc_style = reactive_style(move || {
        let open = palette_open.get();
        styled(
            StyleRules {
                width: Some(Length::Px(22.0).into()),
                height: Some(Length::Px(22.0).into()),
                background: Some(Tokenized::Literal(Color(color_css.get().to_string()))),
                ..Default::default()
            },
            [
                radius(11.0),
                border_all(
                    if open { 2.0 } else { 1.5 },
                    if open { "#2563eb" } else { "rgba(17,24,39,0.28)" },
                ),
            ],
        )
    });
    let disc = ui! { view(style = disc_style) {} };
    bare_btn(disc, move || palette_open.set(!palette_open.get()))
}

/// Props for [`ClearButton`].
pub struct ClearButtonProps {
    pub strokes: Strokes,
    pub version: Signal<u64>,
}

impl Default for ClearButtonProps {
    fn default() -> Self {
        Self { strokes: Default::default(), version: Signal::new(0) }
    }
}

/// Clear the board — a bare trash icon.
#[component]
pub fn ClearButton(props: &ClearButtonProps) -> Element {
    let strokes = props.strokes.clone();
    let version = props.version;
    let glyph = icon_box(icon(TRASH_2).color(|| Color::from("#374151")).into_element());
    bare_btn(glyph, move || {
        strokes.borrow_mut().clear();
        version.set(version.get().wrapping_add(1));
    })
}

/// Props for [`CameraToggle`].
pub struct CameraToggleProps {
    pub cam_on: Signal<bool>,
    pub cam_stream: Signal<Option<MediaStream>>,
}

impl Default for CameraToggleProps {
    fn default() -> Self {
        Self { cam_on: Signal::new(false), cam_stream: Signal::new(None) }
    }
}

/// Camera on/off toggle: a bare camera icon, green when live, grey when off.
#[component]
pub fn CameraToggle(props: &CameraToggleProps) -> Element {
    let cam_on = props.cam_on;
    let cam_stream = props.cam_stream;
    let glyph = icon_box(
        icon(CAMERA)
            .color(move || {
                if cam_on.get() {
                    Color::from("#16a34a")
                } else {
                    Color::from("#374151")
                }
            })
            .into_element(),
    );
    bare_btn(glyph, move || {
        if cam_on.get() {
            cam_on.set(false);
            cam_stream.set(None);
        } else {
            cam_on.set(true);
            runtime_core::driver::spawn_async(async move {
                let config = CameraConfig {
                    facing: CameraFacing::Front,
                    ..Default::default()
                };
                match Camera::new().open(config).await {
                    Ok(stream) => cam_stream.set(Some(stream)),
                    Err(_) => cam_on.set(false),
                }
            });
        }
    })
}

// ============================================================================
// Color palette popover (left of the rail)
// ============================================================================

/// Props for [`PalettePopover`].
pub struct PalettePopoverProps {
    pub focused: Rc<dyn Fn() -> bool>,
    pub color_css: Signal<&'static str>,
    pub palette_open: Signal<bool>,
}

impl Default for PalettePopoverProps {
    fn default() -> Self {
        Self {
            focused: Rc::new(|| true),
            color_css: Signal::new(PALETTE[0].1),
            palette_open: Signal::new(false),
        }
    }
}

/// The color palette popover, docked right of center, offset left of the rail so
/// it sits beside the color button. `focus_gate` (instant hide) handles
/// navigation; the inner `presence` animates the open/close toggle.
#[component]
pub fn PalettePopover(props: &PalettePopoverProps) -> Element {
    let focused = props.focused.clone();
    let color_css = props.color_css;
    let palette_open = props.palette_open;

    let panel = focus_gate(focused, move || {
        presence(move || {
            let grid_style = static_style(StyleRules {
                flex_direction: Some(FlexDirection::Row),
                flex_wrap: Some(FlexWrap::Wrap),
                width: Some(Length::Px(108.0).into()),
                gap: Some(Length::Px(10.0).into()),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                ..Default::default()
            });
            let panel_style = static_style(styled(
                StyleRules {
                    padding_top: Some(Length::Px(12.0).into()),
                    padding_bottom: Some(Length::Px(12.0).into()),
                    padding_left: Some(Length::Px(12.0).into()),
                    padding_right: Some(Length::Px(12.0).into()),
                    background: Some(Tokenized::Literal(Color("rgba(255,255,255,0.97)".into()))),
                    ..Default::default()
                },
                [radius(18.0), border_all(1.0, "rgba(17,24,39,0.08)")],
            ));
            ui! {
                view(style = panel_style) {
                    view(style = grid_style) {
                        for (_label, css) in PALETTE {
                            Swatch(css = *css, color_css = color_css, palette_open = palette_open)
                        }
                    }
                }
            }
        })
        .present(move || palette_open.get())
        .enter(PresenceAnim::new(
            PresenceState {
                opacity: Some(0.0),
                translate_x: Some(12.0),
                scale: Some(0.96),
                ..Default::default()
            },
            170,
            Easing::EaseOut,
        ))
        .exit(PresenceAnim::new(
            PresenceState {
                opacity: Some(0.0),
                translate_x: Some(12.0),
                scale: Some(0.96),
                ..Default::default()
            },
            130,
            Easing::EaseIn,
        ))
        .into_element()
    });

    let dock_style = reactive_style(move || {
        let ins = safe_area_insets().get();
        let rail_w = TOOL_BTN + 16.0 + 12.0; // button + rail padding + gap
        StyleRules {
            position: Some(Position::Absolute),
            top: Some(Length::Px(0.0).into()),
            bottom: Some(Length::Px(0.0).into()),
            right: Some(Length::Px(RAIL_EDGE + ins.right + rail_w).into()),
            flex_direction: Some(FlexDirection::Column),
            justify_content: Some(JustifyContent::Center),
            align_items: Some(AlignItems::FlexEnd),
            ..Default::default()
        }
    });
    ui! {
        view(style = dock_style) {
            panel
        }
    }
}

/// Props for [`Swatch`].
pub struct SwatchProps {
    pub css: &'static str,
    pub color_css: Signal<&'static str>,
    pub palette_open: Signal<bool>,
}

impl Default for SwatchProps {
    fn default() -> Self {
        Self {
            css: PALETTE[0].1,
            color_css: Signal::new(PALETTE[0].1),
            palette_open: Signal::new(false),
        }
    }
}

/// A color swatch in the popover. Tapping sets the color and closes the popover.
#[component]
pub fn Swatch(props: &SwatchProps) -> Element {
    let css = props.css;
    let color_css = props.color_css;
    let palette_open = props.palette_open;
    let style = reactive_style(move || {
        let selected = color_css.get() == css;
        styled(
            StyleRules {
                width: Some(Length::Px(28.0).into()),
                height: Some(Length::Px(28.0).into()),
                background: Some(Tokenized::Literal(Color(css.to_string()))),
                ..Default::default()
            },
            [radius(14.0), border_all(if selected { 3.0 } else { 0.0 }, "#1f2937")],
        )
    });
    ui! {
        view(style = style) {}
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                color_css.set(css);
                palette_open.set(false);
            }
            TouchResponse::CONSUMED
        })
    }
}

// ============================================================================
// Record dock (bottom, camera-style start/stop button)
// ============================================================================

/// Props for [`RecordDock`].
pub struct RecordDockProps {
    pub focused: Rc<dyn Fn() -> bool>,
    pub state: BoardState,
    pub rec_handle: RecHandle,
    pub capture: CanvasCapture,
    pub version: Signal<u64>,
}

impl Default for RecordDockProps {
    fn default() -> Self {
        Self {
            focused: Rc::new(|| true),
            state: BoardState::default(),
            rec_handle: Default::default(),
            capture: CanvasCapture::default(),
            version: Signal::new(0),
        }
    }
}

/// A full-width bottom dock: idle → centered, recording → slid to the right. The
/// button unmounts when the board loses focus; the positioned dock wrapper
/// (transparent, passthrough) stays.
#[component]
pub fn RecordDock(props: &RecordDockProps) -> Element {
    let focused = props.focused.clone();
    let s = props.state;
    let rec_handle = props.rec_handle.clone();
    let capture = props.capture.clone();
    let version = props.version;
    let recording = s.recording;

    let button = focus_gate(focused, move || {
        ui! {
            RecordButton(
                state = s,
                rec_handle = rec_handle.clone(),
                capture = capture.clone(),
                version = version,
            )
        }
    });

    let dock_style = reactive_style(move || {
        let ins = safe_area_insets().get();
        StyleRules {
            position: Some(Position::Absolute),
            left: Some(Length::Px(0.0).into()),
            right: Some(Length::Px(0.0).into()),
            bottom: Some(Length::Px(28.0 + ins.bottom).into()),
            flex_direction: Some(FlexDirection::Row),
            align_items: Some(AlignItems::Center),
            justify_content: Some(if recording.get() {
                JustifyContent::FlexEnd
            } else {
                JustifyContent::Center
            }),
            padding_right: Some(
                Length::Px(if recording.get() { 24.0 + ins.right } else { 0.0 }).into(),
            ),
            ..Default::default()
        }
    });
    ui! {
        view(style = dock_style) {
            button
        }
    }
}

/// Props for [`RecordButton`].
pub struct RecordButtonProps {
    pub state: BoardState,
    pub rec_handle: RecHandle,
    pub capture: CanvasCapture,
    pub version: Signal<u64>,
}

impl Default for RecordButtonProps {
    fn default() -> Self {
        Self {
            state: BoardState::default(),
            rec_handle: Default::default(),
            capture: CanvasCapture::default(),
            version: Signal::new(0),
        }
    }
}

/// The record button: a white ring with a red core. Idle = red disc (record);
/// recording = red rounded square (stop). Records the canvas's OWN output
/// (self-capture): it subscribes a media-writer recording to the app-owned
/// canvas `MediaStream` and drives a frame-rate cadence loop (ticking the canvas
/// `version` so the renderer re-renders → reads back a frame each tick). Stopping
/// finalizes the file, stops the cadence, and opens the Preview screen.
#[component]
pub fn RecordButton(props: &RecordButtonProps) -> Element {
    let s = props.state;
    let rec_handle = props.rec_handle.clone();
    let capture = props.capture.clone();
    let version = props.version;
    let recording = s.recording;
    let rec_path = s.rec_path;
    let cam_on = s.cam_on;
    let cam_stream = s.cam_stream;
    let nav = s.nav;

    // Inner core morphs disc ↔ square via reactive radius + size.
    let core_style = reactive_style(move || {
        let rec = recording.get();
        let size = if rec { 26.0 } else { 44.0 };
        styled(
            StyleRules {
                width: Some(Length::Px(size).into()),
                height: Some(Length::Px(size).into()),
                background: Some(Tokenized::Literal(Color("#ef4444".into()))),
                ..Default::default()
            },
            [radius(if rec { 7.0 } else { 22.0 })],
        )
    });
    let ring_style = static_style(styled(
        StyleRules {
            width: Some(Length::Px(64.0).into()),
            height: Some(Length::Px(64.0).into()),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            background: Some(Tokenized::Literal(Color("rgba(255,255,255,0.96)".into()))),
            ..Default::default()
        },
        [radius(32.0), border_all(3.0, "rgba(17,24,39,0.12)")],
    ));

    ui! {
        view(style = ring_style) {
            view(style = core_style) {}
        }
        .on_touch(move |ev| {
            if ev.phase != TouchPhase::Ended {
                return TouchResponse::CONSUMED;
            }
            if recording.get() {
                // STOP → finalize the file, then open the Preview screen.
                recording.set(false);
                // Stop the capture cadence loop: the canvas no longer needs to
                // re-render every frame. (`rec.stop()` also drops the recorder's
                // subscription, so `writer.wants_cpu_frames()` goes false and the
                // renderer stops reading back — the app-owned stream stays alive.)
                *capture.raf.borrow_mut() = None;
                // End the camera too (if running): it shouldn't keep streaming
                // behind the Preview screen. Dropping the stream stops capture.
                if cam_on.get() {
                    cam_on.set(false);
                    cam_stream.set(None);
                }
                let rec_handle = rec_handle.clone();
                runtime_core::driver::spawn_async(async move {
                    // Bind the take() out of the RefMut so we don't hold the
                    // borrow across `.await` (see refmut-lifetime memory).
                    let taken = rec_handle.borrow_mut().take();
                    if let Some(rec) = taken {
                        // Don't swallow a finalize failure: a recording that
                        // produced no preview should say why, not vanish.
                        match rec.stop().await {
                            Ok(path) => {
                                rec_path.set(Some(path));
                                // Push the Preview screen onto the stack.
                                match nav.get() {
                                    Some(h) => h.push(&PREVIEW, ()),
                                    None => eprintln!("[whiteboard] stop: nav handle missing, can't push preview"),
                                }
                            }
                            Err(e) => eprintln!("[whiteboard] recording stop failed: {e}"),
                        }
                    } else {
                        eprintln!("[whiteboard] stop: no active recording handle");
                    }
                });
            } else {
                // START → record the canvas's OWN output (self-capture).
                recording.set(true);
                // Drive the canvas at frame rate so it re-renders (and the vello
                // renderer reads back a frame) every frame while recording —
                // otherwise the canvas only repaints on a stroke mutation and the
                // recording would be a frozen frame. Stored in the app-owned
                // `raf` so it survives until STOP clears it.
                *capture.raf.borrow_mut() = Some(runtime_core::scheduling::raf_loop(move || {
                    version.set(version.get().wrapping_add(1));
                }));
                let rec_handle = rec_handle.clone();
                let capture = capture.clone();
                runtime_core::driver::spawn_async(async move {
                    let store = match files::app_files(REC_STORE) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("[whiteboard] record: open store failed: {e}");
                            recording.set(false);
                            *capture.raf.borrow_mut() = None;
                            return;
                        }
                    };
                    let cfg = media_writer::RecordConfig::new(store, REC_FILE);
                    match media_writer::MediaWriter::new()
                        .record(media_writer::MediaInputs::video(&capture.stream), cfg)
                        .await
                    {
                        Ok(rec) => {
                            *rec_handle.borrow_mut() = Some(rec);
                        }
                        Err(e) => {
                            eprintln!("[whiteboard] record: start failed: {e}");
                            recording.set(false);
                            *capture.raf.borrow_mut() = None;
                        }
                    }
                });
            }
            TouchResponse::CONSUMED
        })
    }
}

// ============================================================================
// REC indicator (top-center)
// ============================================================================

/// Props for [`RecIndicator`].
pub struct RecIndicatorProps {
    pub focused: Rc<dyn Fn() -> bool>,
    pub recording: Signal<bool>,
}

impl Default for RecIndicatorProps {
    fn default() -> Self {
        Self { focused: Rc::new(|| true), recording: Signal::new(false) }
    }
}

/// The minimal REC pill, docked top-center. `focus_gate` (instant hide) handles
/// navigation; the inner `presence` animates the recording on/off toggle.
#[component]
pub fn RecIndicator(props: &RecIndicatorProps) -> Element {
    let focused = props.focused.clone();
    let recording = props.recording;

    let pill = focus_gate(focused, move || {
        presence(move || {
            let dot_style = static_style(styled(
                StyleRules {
                    width: Some(Length::Px(9.0).into()),
                    height: Some(Length::Px(9.0).into()),
                    background: Some(Tokenized::Literal(Color("#ef4444".into()))),
                    ..Default::default()
                },
                [radius(5.0)],
            ));
            let pill_style = static_style(styled(
                StyleRules {
                    flex_direction: Some(FlexDirection::Row),
                    align_items: Some(AlignItems::Center),
                    gap: Some(Length::Px(7.0).into()),
                    padding_top: Some(Length::Px(6.0).into()),
                    padding_bottom: Some(Length::Px(6.0).into()),
                    padding_left: Some(Length::Px(12.0).into()),
                    padding_right: Some(Length::Px(12.0).into()),
                    background: Some(Tokenized::Literal(Color("rgba(17,24,39,0.82)".into()))),
                    color: Some(Tokenized::Literal(Color("#ffffff".into()))),
                    ..Default::default()
                },
                [radius(13.0)],
            ));
            ui! {
                view(style = pill_style) {
                    view(style = dot_style) {}
                    text() { "REC".to_string() }
                }
            }
        })
        .present(move || recording.get())
        .enter(PresenceAnim::new(
            PresenceState { opacity: Some(0.0), translate_y: Some(-8.0), ..Default::default() },
            180,
            Easing::EaseOut,
        ))
        .exit(PresenceAnim::new(
            PresenceState { opacity: Some(0.0), translate_y: Some(-8.0), ..Default::default() },
            130,
            Easing::EaseIn,
        ))
        .into_element()
    });

    let dock_style = reactive_style(move || {
        let ins = safe_area_insets().get();
        StyleRules {
            position: Some(Position::Absolute),
            top: Some(Length::Px(16.0 + ins.top).into()),
            left: Some(Length::Px(0.0).into()),
            right: Some(Length::Px(0.0).into()),
            flex_direction: Some(FlexDirection::Row),
            justify_content: Some(JustifyContent::Center),
            ..Default::default()
        }
    });
    ui! {
        view(style = dock_style) {
            pill
        }
    }
}

// ============================================================================
// Settings FAB (top-left, while not recording)
// ============================================================================

/// Props for [`SettingsFab`].
pub struct SettingsFabProps {
    pub focused: Rc<dyn Fn() -> bool>,
    pub recording: Signal<bool>,
    pub nav: Ref<StackHandle>,
}

impl Default for SettingsFabProps {
    fn default() -> Self {
        Self {
            focused: Rc::new(|| true),
            recording: Signal::new(false),
            nav: Ref::new(),
        }
    }
}

/// The settings FAB, docked top-left. `focus_gate` (instant hide) handles
/// navigation; the inner `presence` animates the not-recording on/off toggle.
#[component]
pub fn SettingsFab(props: &SettingsFabProps) -> Element {
    let focused = props.focused.clone();
    let recording = props.recording;
    let nav = props.nav;

    let fab = focus_gate(focused, move || {
        presence(move || {
            let glyph =
                icon_box(icon(ICON_SETTINGS).color(|| Color::from("#374151")).into_element());
            let fab_style = static_style(styled(
                StyleRules {
                    width: Some(Length::Px(44.0).into()),
                    height: Some(Length::Px(44.0).into()),
                    align_items: Some(AlignItems::Center),
                    justify_content: Some(JustifyContent::Center),
                    background: Some(Tokenized::Literal(Color("rgba(255,255,255,0.92)".into()))),
                    ..Default::default()
                },
                [radius(22.0), border_all(1.0, "rgba(17,24,39,0.08)")],
            ));
            ui! {
                view(style = fab_style) {
                    glyph
                }
                .on_touch(move |ev| {
                    if ev.phase == TouchPhase::Ended {
                        // Push the Settings screen onto the stack.
                        if let Some(h) = nav.get() {
                            h.push(&SETTINGS, ());
                        }
                    }
                    TouchResponse::CONSUMED
                })
            }
        })
        // Only while NOT recording.
        .present(move || !recording.get())
        .enter(PresenceAnim::new(
            PresenceState { opacity: Some(0.0), scale: Some(0.9), ..Default::default() },
            160,
            Easing::EaseOut,
        ))
        .exit(PresenceAnim::new(
            PresenceState { opacity: Some(0.0), scale: Some(0.9), ..Default::default() },
            120,
            Easing::EaseIn,
        ))
        .into_element()
    });

    let dock_style = reactive_style(move || {
        let ins = safe_area_insets().get();
        StyleRules {
            position: Some(Position::Absolute),
            top: Some(Length::Px(16.0 + ins.top).into()),
            left: Some(Length::Px(16.0 + ins.left).into()),
            ..Default::default()
        }
    });
    ui! {
        view(style = dock_style) {
            fab
        }
    }
}
