//! The board's floating chrome: tool rail, color palette popover, record dock +
//! REC pill, and the settings FAB. Each piece is a normal in-tree sibling of the
//! canvas (no separate window) — an individually-positioned overlay sized to its
//! content, so the empty areas around each control have no view and touches fall
//! straight through to the canvas. (The chrome is never part of a recording
//! because the app records the canvas/GPU stream directly, not the screen.)
//!
//! Each dock keeps its POSITIONED wrapper mounted (so its inset resolves against
//! the full window) and gates only its CONTENT via [`focus_gate`] — the
//! instant-hide presence that vanishes the chrome the same turn a screen is
//! pushed, so it doesn't linger over the pushed screen. Settings / REC / palette
//! additionally nest an inner `presence` that animates their own state toggle
//! (open, recording, …).

use crate::style::{
    border_all_color, focus_gate, radius, reactive_style, static_style, styled, token, token_alpha,
    token_intent,
};
use crate::{
    BoardState, CanvasBg, CanvasCapture, CanvasStore, RecHandle, Strokes, PALETTE, PREVIEW,
    REC_FILE, REC_STORE, SETTINGS, WIDTH_MEDIUM, WIDTH_THICK, WIDTH_THIN,
};
use camera::{Camera, CameraConfig, CameraFacing, MediaStream};
use icons_lucide::{
    CAMERA, CIRCLE, LAYERS, PALETTE as ICON_PALETTE, PLUS, SETTINGS as ICON_SETTINGS, SQUARE,
    TRASH_2,
};
use runtime_core::{
    component, icon, presence, safe_area_insets, ui, AlignItems, Color, Easing, Element,
    FlexDirection, FlexWrap, IconData, IntoElement, JustifyContent, Length, Position,
    PresenceAnim, PresenceState, Ref, Signal, StyleRules, Tokenized, TouchPhase, TouchResponse,
    Transform,
};
use stack_navigator::StackHandle;
use std::rc::Rc;

use crate::{RAIL_EDGE, TOOL_BTN};

/// Gap (in points) from the safe-area edge to the floating corner controls. The
/// navigator is full-screen, so the safe-area inset already clears the notch /
/// home-indicator — these add extra breathing room on top of it.
const FAB_EDGE: f32 = 28.0; // settings FAB: top + left
const RECORD_BOTTOM: f32 = 48.0; // record dock: bottom
const RECORD_RIGHT: f32 = 28.0; // record dock when recording: right

/// Build the board's floating chrome as in-tree sibling overlays, in paint
/// order: `[rec_indicator, palette, tool_rail, rec_dock, settings_btn]`. A plain
/// `fn` (not a component): `BoardScreen` splices the returned `Vec<Element>`
/// straight into the board root alongside the canvas stage.
pub fn build_chrome(
    focused: Rc<dyn Fn() -> bool>,
    s: BoardState,
    strokes: Strokes,
    canvases: CanvasStore,
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
            canvas_bg = s.canvas_bg,
            dark = s.dark,
        )
    };
    let layers = ui! {
        LayersPopover(
            focused = focused.clone(),
            state = s,
            strokes = strokes.clone(),
            canvases = canvases.clone(),
            version = version,
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
    vec![rec_indicator, palette, layers, tool_rail, rec_dock, settings_btn]
}

// ============================================================================
// Shared chrome helpers
// ============================================================================

/// Position a child vertically centered against the right edge, inset by the
/// safe area. The wrapper is sized to the CHILD (not the full screen) and pulled
/// to the vertical center via a `-50%` self-translate, so only the child's box
/// captures touches — the rest of the right edge falls through to the canvas
/// (a full-height wrapper would swallow every stroke started down that column).
fn dock_right(child: Element) -> Element {
    ui! {
        view(style = reactive_style(move || {
            let ins = safe_area_insets().get();
            StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::pct(50.0).into()),
                right: Some(Length::Px(RAIL_EDGE + ins.right).into()),
                transform: Some(vec![Transform::TranslateY(Length::pct(-50.0))]),
                flex_direction: Some(FlexDirection::Column),
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
        let pill_style = reactive_style(move || {
            styled(
                StyleRules {
                    flex_direction: Some(FlexDirection::Column),
                    align_items: Some(AlignItems::Center),
                    gap: Some(Length::Px(2.0).into()),
                    padding_top: Some(Length::Px(8.0).into()),
                    padding_bottom: Some(Length::Px(8.0).into()),
                    padding_left: Some(Length::Px(6.0).into()),
                    padding_right: Some(Length::Px(6.0).into()),
                    background: Some(Tokenized::Literal(token_alpha(|c| c.surface.clone(), 0.92))),
                    ..Default::default()
                },
                [radius(24.0), border_all_color(1.0, token_alpha(|c| c.border.clone(), 0.7))],
            )
        });
        ui! {
            view(style = pill_style) {
                WidthButton(w = WIDTH_THIN, width = s.width)
                WidthButton(w = WIDTH_MEDIUM, width = s.width)
                WidthButton(w = WIDTH_THICK, width = s.width)
                RailDivider()
                ColorButton(color_css = s.color_css, palette_open = s.palette_open, layers_open = s.layers_open, canvas_bg = s.canvas_bg, dark = s.dark)
                ClearButton(strokes = strokes.clone(), version = version)
                RailDivider()
                LayersButton(layers_open = s.layers_open, palette_open = s.palette_open)
                CameraToggle(cam_on = s.cam_on, cam_stream = s.cam_stream)
            }
        }
    });

    dock_right(pill)
}

/// A horizontal divider inside the vertical rail.
#[component]
pub fn RailDivider() -> Element {
    let style = reactive_style(|| StyleRules {
        width: Some(Length::Px(24.0).into()),
        height: Some(Length::Px(1.0).into()),
        background: Some(Tokenized::Literal(token_alpha(|c| c.border.clone(), 0.6))),
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
/// it sets — the dot IS the preview of the line weight, which a repeated pen
/// glyph can't convey. Accent-blue when selected, muted grey otherwise; color,
/// not a background box, carries the state.
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
                background: Some(Tokenized::Literal(if selected {
                    token_intent(|i| i.primary.solid_bg.clone())
                } else {
                    token(|c| c.text_muted.clone())
                })),
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
    /// Closed when the palette opens (the two rail popovers are exclusive).
    pub layers_open: Signal<bool>,
    pub canvas_bg: Signal<CanvasBg>,
    pub dark: Signal<bool>,
}

impl Default for ColorButtonProps {
    fn default() -> Self {
        Self {
            color_css: Signal::new(PALETTE[0].1),
            palette_open: Signal::new(false),
            layers_open: Signal::new(false),
            canvas_bg: Signal::new(CanvasBg::Auto),
            dark: Signal::new(false),
        }
    }
}

/// The color button: a lucide palette glyph TINTED with the current draw color
/// (so it previews what will actually draw, adaptive-ink resolved). Tapping
/// toggles the palette popover.
#[component]
pub fn ColorButton(props: &ColorButtonProps) -> Element {
    let color_css = props.color_css;
    let palette_open = props.palette_open;
    let layers_open = props.layers_open;
    let canvas_bg = props.canvas_bg;
    let dark = props.dark;
    let glyph = icon_box(
        icon(ICON_PALETTE)
            .color(move || {
                // Resolve the adaptive ink slot so the glyph shows the real draw color.
                let css = crate::resolve_color(color_css.get(), canvas_bg.get(), dark.get());
                Color(css.to_string())
            })
            .into_element(),
    );
    bare_btn(glyph, move || {
        layers_open.set(false);
        palette_open.set(!palette_open.get());
    })
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
    let glyph = icon_box(icon(TRASH_2).color(|| token(|c| c.text.clone())).into_element());
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
                    token(|c| c.text.clone())
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
                    Err(e) => {
                        // Don't swallow it — e.g. on Android first tap this is
                        // `PermissionDenied` while the system dialog shows; the
                        // user grants, then taps again to open.
                        eprintln!("[whiteboard] camera open failed: {e:?}");
                        cam_on.set(false);
                    }
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
    pub canvas_bg: Signal<CanvasBg>,
    pub dark: Signal<bool>,
}

impl Default for PalettePopoverProps {
    fn default() -> Self {
        Self {
            focused: Rc::new(|| true),
            color_css: Signal::new(PALETTE[0].1),
            palette_open: Signal::new(false),
            canvas_bg: Signal::new(CanvasBg::Auto),
            dark: Signal::new(false),
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
    let canvas_bg = props.canvas_bg;
    let dark = props.dark;

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
            let panel_style = reactive_style(|| {
                styled(
                    StyleRules {
                        padding_top: Some(Length::Px(12.0).into()),
                        padding_bottom: Some(Length::Px(12.0).into()),
                        padding_left: Some(Length::Px(12.0).into()),
                        padding_right: Some(Length::Px(12.0).into()),
                        background: Some(Tokenized::Literal(token_alpha(|c| c.surface.clone(), 0.97))),
                        ..Default::default()
                    },
                    [radius(18.0), border_all_color(1.0, token_alpha(|c| c.border.clone(), 0.7))],
                )
            });
            ui! {
                view(style = panel_style) {
                    view(style = grid_style) {
                        for (_label, css) in PALETTE {
                            Swatch(css = *css, color_css = color_css, palette_open = palette_open, canvas_bg = canvas_bg, dark = dark)
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
            // Content-sized + self-centered (not full-height) so the popover's
            // column doesn't capture touches when closed/empty — see `dock_right`.
            top: Some(Length::pct(50.0).into()),
            right: Some(Length::Px(RAIL_EDGE + ins.right + rail_w).into()),
            transform: Some(vec![Transform::TranslateY(Length::pct(-50.0))]),
            flex_direction: Some(FlexDirection::Column),
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
    pub canvas_bg: Signal<CanvasBg>,
    pub dark: Signal<bool>,
}

impl Default for SwatchProps {
    fn default() -> Self {
        Self {
            css: PALETTE[0].1,
            color_css: Signal::new(PALETTE[0].1),
            palette_open: Signal::new(false),
            canvas_bg: Signal::new(CanvasBg::Auto),
            dark: Signal::new(false),
        }
    }
}

/// A color swatch in the popover. Tapping sets the color and closes the popover.
/// The adaptive ink slot renders its resolved contrast color against the backdrop.
#[component]
pub fn Swatch(props: &SwatchProps) -> Element {
    let css = props.css;
    let color_css = props.color_css;
    let palette_open = props.palette_open;
    let canvas_bg = props.canvas_bg;
    let dark = props.dark;
    let style = reactive_style(move || {
        let selected = color_css.get() == css;
        let shown = crate::resolve_color(css, canvas_bg.get(), dark.get());
        styled(
            StyleRules {
                width: Some(Length::Px(28.0).into()),
                height: Some(Length::Px(28.0).into()),
                background: Some(Tokenized::Literal(Color(shown.to_string()))),
                ..Default::default()
            },
            [
                radius(14.0),
                border_all_color(if selected { 3.0 } else { 0.0 }, token(|c| c.text.clone())),
            ],
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
// Layers (canvas list) button + popover
// ============================================================================

/// Props for [`LayersButton`].
pub struct LayersButtonProps {
    pub layers_open: Signal<bool>,
    pub palette_open: Signal<bool>,
}

impl Default for LayersButtonProps {
    fn default() -> Self {
        Self { layers_open: Signal::new(false), palette_open: Signal::new(false) }
    }
}

/// Opens the canvas Layers popover — a lucide layers glyph, accent when open.
/// Mutually exclusive with the palette (opening it closes the palette).
#[component]
pub fn LayersButton(props: &LayersButtonProps) -> Element {
    let layers_open = props.layers_open;
    let palette_open = props.palette_open;
    let glyph = icon_box(
        icon(LAYERS)
            .color(move || {
                if layers_open.get() {
                    token_intent(|i| i.primary.solid_bg.clone())
                } else {
                    token(|c| c.text.clone())
                }
            })
            .into_element(),
    );
    bare_btn(glyph, move || {
        palette_open.set(false);
        layers_open.set(!layers_open.get());
    })
}

/// Props for [`LayersPopover`].
pub struct LayersPopoverProps {
    pub focused: Rc<dyn Fn() -> bool>,
    pub state: BoardState,
    pub strokes: Strokes,
    pub canvases: CanvasStore,
    pub version: Signal<u64>,
}

impl Default for LayersPopoverProps {
    fn default() -> Self {
        Self {
            focused: Rc::new(|| true),
            state: BoardState::default(),
            strokes: Default::default(),
            canvases: Default::default(),
            version: Signal::new(0),
        }
    }
}

/// The canvas list popover: a row per canvas (tap to jump, active highlighted, a
/// trash to delete unless it's the last) plus an "add new" row. Same dock +
/// `focus_gate` + open/close `presence` shape as [`PalettePopover`] (they're
/// mutually exclusive, so they share the dock position).
#[component]
pub fn LayersPopover(props: &LayersPopoverProps) -> Element {
    let focused = props.focused.clone();
    let s = props.state;
    let strokes = props.strokes.clone();
    let canvases = props.canvases.clone();
    let version = props.version;
    let layers_open = s.layers_open;
    let canvas_ids = s.canvas_ids;

    let panel = focus_gate(focused, move || {
        let strokes = strokes.clone();
        let canvases = canvases.clone();
        presence(move || {
            let panel_style = reactive_style(|| {
                styled(
                    StyleRules {
                        flex_direction: Some(FlexDirection::Column),
                        width: Some(Length::Px(204.0).into()),
                        gap: Some(Length::Px(4.0).into()),
                        padding_top: Some(Length::Px(8.0).into()),
                        padding_bottom: Some(Length::Px(8.0).into()),
                        padding_left: Some(Length::Px(8.0).into()),
                        padding_right: Some(Length::Px(8.0).into()),
                        background: Some(Tokenized::Literal(token_alpha(|c| c.surface.clone(), 0.97))),
                        ..Default::default()
                    },
                    [radius(16.0), border_all_color(1.0, token_alpha(|c| c.border.clone(), 0.7))],
                )
            });
            // A plain column for the canvas list. (A reactive `for` over a
            // `Signal<Vec<_>>` renders as an `Element::Each`; that reconciles
            // correctly inside a plain container — the proven pattern — but NOT
            // inside a `scroll_view`, which silently dropped the rows. Few
            // canvases in practice, so the popover just grows.)
            let list_style = static_style(StyleRules {
                flex_direction: Some(FlexDirection::Column),
                gap: Some(Length::Px(4.0).into()),
                ..Default::default()
            });
            // Separate clones for the row list vs the add row (each consumer owns
            // its own; the `for` body re-clones per iteration).
            let strokes_rows = strokes.clone();
            let canvases_rows = canvases.clone();
            let strokes_add = strokes.clone();
            let canvases_add = canvases.clone();
            ui! {
                view(style = panel_style) {
                    view(style = list_style) {
                        for id in canvas_ids, key = *id {
                            CanvasRow(
                                id = id,
                                state = s,
                                strokes = strokes_rows.clone(),
                                canvases = canvases_rows.clone(),
                                version = version,
                            )
                        }
                    }
                    AddCanvasRow(state = s, strokes = strokes_add.clone(), canvases = canvases_add.clone(), version = version)
                }
            }
        })
        .present(move || layers_open.get())
        .enter(PresenceAnim::new(
            PresenceState {
                opacity: Some(0.0),
                translate_x: Some(12.0),
                scale: Some(0.96),
                ..Default::default()
            },
            crate::LAYERS_ENTER_MS,
            Easing::EaseOut,
        ))
        .exit(PresenceAnim::new(
            PresenceState {
                opacity: Some(0.0),
                translate_x: Some(12.0),
                scale: Some(0.96),
                ..Default::default()
            },
            crate::LAYERS_EXIT_MS,
            Easing::EaseIn,
        ))
        .into_element()
    });

    let dock_style = reactive_style(move || {
        let ins = safe_area_insets().get();
        let rail_w = TOOL_BTN + 16.0 + 12.0; // button + rail padding + gap
        StyleRules {
            position: Some(Position::Absolute),
            top: Some(Length::pct(50.0).into()),
            right: Some(Length::Px(RAIL_EDGE + ins.right + rail_w).into()),
            transform: Some(vec![Transform::TranslateY(Length::pct(-50.0))]),
            flex_direction: Some(FlexDirection::Column),
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

/// Props for [`CanvasRow`].
pub struct CanvasRowProps {
    pub id: u64,
    pub state: BoardState,
    pub strokes: Strokes,
    pub canvases: CanvasStore,
    pub version: Signal<u64>,
}

impl Default for CanvasRowProps {
    fn default() -> Self {
        Self {
            id: 0,
            state: BoardState::default(),
            strokes: Default::default(),
            canvases: Default::default(),
            version: Signal::new(0),
        }
    }
}

/// One canvas entry in the Layers list: "Canvas {n}" (n = its 1-based position),
/// accent-highlighted when active. Tapping the row jumps to it; the trailing
/// trash deletes it (hidden when it's the only canvas). Its position is derived
/// reactively from `canvas_ids`, so it relabels/retargets after a delete.
#[component]
pub fn CanvasRow(props: &CanvasRowProps) -> Element {
    let id = props.id;
    let s = props.state;
    let strokes = props.strokes.clone();
    let canvases = props.canvases.clone();
    let version = props.version;
    let canvas_ids = s.canvas_ids;
    let active = s.active_canvas;

    // This row's current index in the (reactive) id list.
    let index_of = move || canvas_ids.get().iter().position(|x| *x == id).unwrap_or(0);

    let row_style = reactive_style(move || {
        let is_active = active.get() == index_of();
        styled(
            StyleRules {
                flex_direction: Some(FlexDirection::Row),
                align_items: Some(AlignItems::Center),
                gap: Some(Length::Px(8.0).into()),
                padding_top: Some(Length::Px(8.0).into()),
                padding_bottom: Some(Length::Px(8.0).into()),
                padding_left: Some(Length::Px(10.0).into()),
                padding_right: Some(Length::Px(8.0).into()),
                background: Some(Tokenized::Literal(if is_active {
                    token_alpha(|c| c.text.clone(), 0.08)
                } else {
                    Color("#00000000".into())
                })),
                ..Default::default()
            },
            [radius(10.0)],
        )
    });
    // Label carries its own color (native text doesn't inherit): accent when active.
    let label_style = reactive_style(move || {
        let is_active = active.get() == index_of();
        StyleRules {
            color: Some(Tokenized::Literal(if is_active {
                token_intent(|i| i.primary.solid_bg.clone())
            } else {
                token(|c| c.text.clone())
            })),
            font_size: Some(Length::Px(15.0).into()),
            flex_grow: Some(Tokenized::Literal(1.0)),
            ..Default::default()
        }
    });
    let label = move || format!("Canvas {}", index_of() + 1);

    // Delete affordance — only when more than one canvas exists. Rendered via a
    // reactive `if` so it's truly ABSENT for the sole canvas (NOT just sized 0):
    // a lucide icon renders at its intrinsic size and the box isn't layer-backed
    // on macOS, so a 0×0 box neither shrinks nor clips it — collapsing the box
    // left the trash glyph visible. Building it as a `#[component]` keeps the
    // `Rc` captures out of the `if` branch.
    //
    // `del_visible` is a `memo` (a `Signal<bool>`), NOT a plain closure: `ui!`
    // is type-driven, so `if del_visible` is reactive *because its type is a
    // reactive Signal* — the same way `for x in sig` is reactive by type. A
    // bare `move || …` closure would be an opaque `fn() -> bool`, which the
    // macro treats as STATIC (built once) — that was the original
    // "delete button won't disappear on the last canvas" bug.
    let del_visible = runtime_core::memo(move || canvas_ids.get().len() > 1);
    let strokes_for_del = strokes.clone();
    let canvases_for_del = canvases.clone();
    ui! {
        view(style = row_style) {
            text(style = label_style) { label() }
            if del_visible {
                DeleteCanvasButton(
                    id = id,
                    state = s,
                    strokes = strokes_for_del.clone(),
                    canvases = canvases_for_del.clone(),
                    version = version,
                )
            }
        }
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                // Close immediately, then DEFER the canvas switch to a microtask:
                // the switch deep-clones strokes + repaints the canvas, so running
                // it inline makes the popover's dismissal visibly lag the tap.
                // Deferring lets the close flush this frame; the canvas swaps the
                // next microtask (imperceptible).
                s.layers_open.set(false);
                let target = index_of();
                let canvases = canvases.clone();
                let strokes = strokes.clone();
                runtime_core::scheduling::schedule_microtask(move || {
                    crate::switch_canvas(&canvases, &strokes, active, version, target);
                });
            }
            TouchResponse::CONSUMED
        })
    }
}

/// Props for [`DeleteCanvasButton`].
pub struct DeleteCanvasButtonProps {
    pub id: u64,
    pub state: BoardState,
    pub strokes: Strokes,
    pub canvases: CanvasStore,
    pub version: Signal<u64>,
}

impl Default for DeleteCanvasButtonProps {
    fn default() -> Self {
        Self {
            id: 0,
            state: BoardState::default(),
            strokes: Default::default(),
            canvases: Default::default(),
            version: Signal::new(0),
        }
    }
}

/// The trailing trash button on a [`CanvasRow`] — a 15px glyph in a 28px tap
/// target. Deletes the canvas whose stable `id` this row carries (resolved to a
/// live index against `canvas_ids` at tap time). Only mounted when more than one
/// canvas exists (its caller gates it behind a reactive `if`).
#[component]
pub fn DeleteCanvasButton(props: &DeleteCanvasButtonProps) -> Element {
    let id = props.id;
    let s = props.state;
    let strokes = props.strokes.clone();
    let canvases = props.canvases.clone();
    let version = props.version;
    let canvas_ids = s.canvas_ids;
    let active = s.active_canvas;

    let box_style = static_style(StyleRules {
        width: Some(Length::Px(24.0).into()),
        height: Some(Length::Px(24.0).into()),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });
    let glyph_style = static_style(StyleRules {
        width: Some(Length::Px(13.0).into()),
        height: Some(Length::Px(13.0).into()),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });
    let glyph = icon(TRASH_2).color(|| token(|c| c.text_muted.clone())).into_element();
    ui! {
        view(style = box_style) {
            view(style = glyph_style) {
                glyph
            }
        }
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                let idx = canvas_ids.get().iter().position(|x| *x == id).unwrap_or(0);
                crate::delete_canvas(&canvases, &strokes, active, version, canvas_ids, idx);
            }
            TouchResponse::CLAIMED
        })
    }
}

/// Props for [`AddCanvasRow`].
pub struct AddCanvasRowProps {
    pub state: BoardState,
    pub strokes: Strokes,
    pub canvases: CanvasStore,
    pub version: Signal<u64>,
}

impl Default for AddCanvasRowProps {
    fn default() -> Self {
        Self {
            state: BoardState::default(),
            strokes: Default::default(),
            canvases: Default::default(),
            version: Signal::new(0),
        }
    }
}

/// The "add new canvas" row at the bottom of the Layers list: a lucide plus +
/// "New canvas". Appends an empty canvas, switches to it, and closes the popover
/// (same as tapping a canvas row — you're now on the new canvas to draw).
#[component]
pub fn AddCanvasRow(props: &AddCanvasRowProps) -> Element {
    let s = props.state;
    let strokes = props.strokes.clone();
    let canvases = props.canvases.clone();
    let version = props.version;
    let row_style = reactive_style(|| {
        styled(
            StyleRules {
                flex_direction: Some(FlexDirection::Row),
                align_items: Some(AlignItems::Center),
                gap: Some(Length::Px(8.0).into()),
                padding_top: Some(Length::Px(8.0).into()),
                padding_bottom: Some(Length::Px(8.0).into()),
                padding_left: Some(Length::Px(10.0).into()),
                padding_right: Some(Length::Px(8.0).into()),
                ..Default::default()
            },
            [
                radius(10.0),
                border_all_color(1.0, token_alpha(|c| c.border.clone(), 0.6)),
            ],
        )
    });
    let plus = icon_box(icon(PLUS).color(|| token_intent(|i| i.primary.solid_bg.clone())).into_element());
    let label_style = reactive_style(|| StyleRules {
        color: Some(Tokenized::Literal(token_intent(|i| i.primary.solid_bg.clone()))),
        font_size: Some(Length::Px(15.0).into()),
        ..Default::default()
    });
    ui! {
        view(style = row_style) {
            plus
            text(style = label_style) { "New canvas".to_string() }
        }
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                // Close the popover, then defer the add until AFTER its exit
                // animation finishes. `add_canvas` pushes a new id into
                // `canvas_ids` (a new row); running it now made that row pop into
                // the still-closing panel — the stutter the user reported. Waiting
                // for the exit means the panel is gone before the list changes; the
                // stage cross-fade (driven by the active-canvas change) then covers
                // the swap. The `+24` clears the exit's final unmount frame.
                s.layers_open.set(false);
                let canvases = canvases.clone();
                let strokes = strokes.clone();
                runtime_core::scheduling::after_ms_detached(
                    crate::LAYERS_EXIT_MS as i32 + 24,
                    move || {
                        crate::add_canvas(&canvases, &strokes, s.active_canvas, version, s.canvas_ids, s.next_id);
                    },
                );
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
        let rec = recording.get();
        // Content-sized wrapper (not full-width) so the bottom band around the
        // button passes touches through to the canvas — see `dock_right`. Idle:
        // horizontally centered via a `-50%` self-translate. Recording: anchored
        // bottom-right (the button slides out of the way of the stage).
        //
        // Set `left`/`right`/`transform` on BOTH states (toggle the VALUE, not the
        // presence): the backend doesn't reset a property a reactive restyle omits,
        // so leaving the idle `left:50%`+translate unset while recording would keep
        // it pinned near center instead of moving fully right.
        StyleRules {
            position: Some(Position::Absolute),
            bottom: Some(Length::Px(RECORD_BOTTOM + ins.bottom).into()),
            left: Some(if rec { Length::Auto } else { Length::pct(50.0) }.into()),
            right: Some(
                if rec {
                    Length::Px(RECORD_RIGHT + ins.right)
                } else {
                    Length::Auto
                }
                .into(),
            ),
            transform: Some(if rec {
                vec![]
            } else {
                vec![Transform::TranslateX(Length::pct(-50.0))]
            }),
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

    // Inner glyph: a red FILLED lucide circle (record) ↔ rounded square (stop),
    // swapped reactively. `filled: true` makes the outline glyph a solid shape so
    // it reads as bold as the old hand-drawn core. The box scales (bigger circle
    // when idle, smaller square when recording); the icon fills the box.
    let rec_glyph = IconData { filled: true, ..CIRCLE };
    let stop_glyph = IconData { filled: true, ..SQUARE };
    let glyph_box = reactive_style(move || {
        let size = if recording.get() { 26.0 } else { 44.0 };
        StyleRules {
            width: Some(Length::Px(size).into()),
            height: Some(Length::Px(size).into()),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            ..Default::default()
        }
    });
    let ring_style = reactive_style(|| {
        styled(
            StyleRules {
                width: Some(Length::Px(64.0).into()),
                height: Some(Length::Px(64.0).into()),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                background: Some(Tokenized::Literal(token_alpha(|c| c.surface.clone(), 0.96))),
                ..Default::default()
            },
            [radius(32.0), border_all_color(3.0, token_alpha(|c| c.border.clone(), 0.6))],
        )
    });

    ui! {
        view(style = ring_style) {
            view(style = glyph_box) {
                if recording.get() {
                    icon(data = stop_glyph, color = || Color::from("#ef4444"))
                } else {
                    icon(data = rec_glyph, color = || Color::from("#ef4444"))
                }
            }
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
            // The badge inverts against the app background: `text` token (near-black
            // in light, near-white in dark) carries the frosted pill; `text_inverse`
            // the "REC" label. So the badge reads on both the light and dark stage.
            let pill_style = reactive_style(|| {
                styled(
                    StyleRules {
                        flex_direction: Some(FlexDirection::Row),
                        align_items: Some(AlignItems::Center),
                        gap: Some(Length::Px(7.0).into()),
                        padding_top: Some(Length::Px(6.0).into()),
                        padding_bottom: Some(Length::Px(6.0).into()),
                        padding_left: Some(Length::Px(12.0).into()),
                        padding_right: Some(Length::Px(12.0).into()),
                        background: Some(Tokenized::Literal(token_alpha(|c| c.text.clone(), 0.82))),
                        color: Some(Tokenized::Literal(token(|c| c.text_inverse.clone()))),
                        ..Default::default()
                    },
                    [radius(13.0)],
                )
            });
            // Native text doesn't inherit container `color`; set it on the node too.
            let rec_color = reactive_style(|| StyleRules {
                color: Some(Tokenized::Literal(token(|c| c.text_inverse.clone()))),
                ..Default::default()
            });
            ui! {
                view(style = pill_style) {
                    view(style = dot_style) {}
                    text(style = rec_color) { "REC".to_string() }
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
            top: Some(Length::Px(FAB_EDGE + ins.top).into()),
            // Content-sized + self-centered (not full-width) so the top band
            // doesn't capture touches — see `dock_right`.
            left: Some(Length::pct(50.0).into()),
            transform: Some(vec![Transform::TranslateX(Length::pct(-50.0))]),
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
            let glyph = icon_box(
                icon(ICON_SETTINGS).color(|| token(|c| c.text.clone())).into_element(),
            );
            let fab_style = reactive_style(|| {
                styled(
                    StyleRules {
                        width: Some(Length::Px(44.0).into()),
                        height: Some(Length::Px(44.0).into()),
                        align_items: Some(AlignItems::Center),
                        justify_content: Some(JustifyContent::Center),
                        background: Some(Tokenized::Literal(token_alpha(|c| c.surface.clone(), 0.92))),
                        ..Default::default()
                    },
                    [radius(22.0), border_all_color(1.0, token_alpha(|c| c.border.clone(), 0.7))],
                )
            });
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
            top: Some(Length::Px(FAB_EDGE + ins.top).into()),
            left: Some(Length::Px(FAB_EDGE + ins.left).into()),
            ..Default::default()
        }
    });
    ui! {
        view(style = dock_style) {
            fab
        }
    }
}
