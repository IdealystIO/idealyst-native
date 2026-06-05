//! The navigator-pushed screens (`Settings`, `Preview`) and their shared
//! scaffold. Unlike the board's detached `PrivateLayer` chrome, these are normal
//! host-tree content, so reactive style + ordinary components are fine.
//!
//! Every screen carries its OWN in-content header (`ScreenHeader`: title + a back
//! `×` that pops the stack), because `header_shown(false)` is set on every route
//! — the stack handler renders native chrome on iOS / Android / web but NOT on
//! macOS / terminal / SSR, so without the in-content header the user couldn't get
//! back there.

use crate::style::{radius, reactive_style, static_style, styled};
use crate::{REC_FILE, REC_STORE};
use icons_lucide::X;
use runtime_core::animation::{AnimProp, AnimatedValue, TweenTo};
use runtime_core::{
    component, icon, safe_area_insets, ui, view, viewport_size, AlignItems, ChildList, Color,
    Effect, Element, FlexDirection, FontWeight, IntoElement, JustifyContent, Length, Overflow, Ref,
    Signal, StyleRules, Tokenized, TouchPhase, TouchResponse, ViewHandle,
};
use std::time::Duration;
use stack_navigator::StackHandle;
use std::rc::Rc;

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
            background: Some(Tokenized::Literal(Color("#f7f8fb".into()))),
            // Web-only cascade fallback; native sets color per text node (Label).
            color: Some(Tokenized::Literal(Color("#111827".into()))),
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

    let close_style = static_style(styled(
        StyleRules {
            width: Some(Length::Px(40.0).into()),
            height: Some(Length::Px(40.0).into()),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            background: Some(Tokenized::Literal(Color("rgba(17,24,39,0.05)".into()))),
            ..Default::default()
        },
        [radius(20.0)],
    ));
    let glyph_style = static_style(StyleRules {
        width: Some(Length::Px(22.0).into()),
        height: Some(Length::Px(22.0).into()),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });
    let glyph = icon(X).color(|| Color::from("#374151")).into_element();
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
            Label(text = title, hex = "#111827", weight = FontWeight::Bold, size = 22.0)
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
    pub nav: Ref<StackHandle>,
}

impl Default for SettingsScreenProps {
    fn default() -> Self {
        Self { nav: Ref::new() }
    }
}

/// The Settings screen — placeholder rows + a note, in the shared scaffold.
#[component]
pub fn SettingsScreen(props: &SettingsScreenProps) -> Element {
    let nav = props.nav;

    let rows_style = static_style(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(Length::Px(2.0).into()),
        padding_left: Some(Length::Px(20.0).into()),
        padding_right: Some(Length::Px(20.0).into()),
        ..Default::default()
    });
    let note_box_style = static_style(StyleRules {
        padding_left: Some(Length::Px(20.0).into()),
        padding_top: Some(Length::Px(8.0).into()),
        ..Default::default()
    });

    ui! {
        ScreenScaffold(title = "Settings", nav = nav) {
            view(style = rows_style) {
                SettingRow(label = "Smooth strokes", on = true)
                SettingRow(label = "Show grid", on = false)
                SettingRow(label = "Pressure sensitivity", on = false)
                SettingRow(label = "High-quality recording", on = true)
            }
            view(style = note_box_style) {
                Label(
                    text = "Placeholder — these toggle, but aren't wired up yet.",
                    hex = "#9ca3af",
                    weight = FontWeight::Normal,
                    size = 13.0,
                )
            }
        }
    }
}

/// Props for [`SettingRow`].
pub struct SettingRowProps {
    pub label: &'static str,
    pub on: bool,
}

impl Default for SettingRowProps {
    fn default() -> Self {
        Self { label: "", on: false }
    }
}

/// One settings row: a label + a tappable pill switch. Tapping anywhere in the
/// row flips the switch. The switch holds its own local toggle state (these are
/// demo placeholders — they flip visually but aren't wired to app behavior yet).
#[component]
pub fn SettingRow(props: &SettingRowProps) -> Element {
    let label = props.label;
    // Local, persistent toggle state seeded from the `on` prop. A component body
    // runs ONCE at mount (only its reactive scopes re-run), so this `Signal` is
    // created once and survives re-renders.
    let toggled = Signal::new(props.on);

    // Animated knob slide: the knob is anchored left and its `TranslateX`
    // tweens 0 → TRAVEL. A transform animates; flipping a layout property
    // (justify) would just JUMP — the framework's animation primitive
    // (`AnimatedValue` + `AnimProp::TranslateX`) is what produces the slide.
    // Same pattern as idea-ui's `Switch`.
    const TRAVEL: f32 = 16.0;
    let knob_ref: Ref<ViewHandle> = Ref::new();
    let av: AnimatedValue<f32> = AnimatedValue::new(if props.on { TRAVEL } else { 0.0 });
    av.bind(knob_ref, AnimProp::TranslateX);
    let _slide = Effect::new(move || {
        let target = if toggled.get() { TRAVEL } else { 0.0 };
        av.animate(TweenTo::new(target, Duration::from_millis(160)).ease_out());
    });

    let knob_style = static_style(styled(
        StyleRules {
            width: Some(Length::Px(18.0).into()),
            height: Some(Length::Px(18.0).into()),
            background: Some(Tokenized::Literal(Color("#ffffff".into()))),
            ..Default::default()
        },
        [radius(9.0)],
    ));
    // Track recolors reactively (instant); the knob slide is animated above.
    let track_style = reactive_style(move || {
        styled(
            StyleRules {
                width: Some(Length::Px(40.0).into()),
                height: Some(Length::Px(24.0).into()),
                padding_left: Some(Length::Px(3.0).into()),
                padding_right: Some(Length::Px(3.0).into()),
                flex_direction: Some(FlexDirection::Row),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::FlexStart),
                background: Some(Tokenized::Literal(Color(
                    if toggled.get() { "#2563eb" } else { "#d1d5db" }.into(),
                ))),
                ..Default::default()
            },
            [radius(12.0)],
        )
    });
    let row_style = static_style(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::SpaceBetween),
        padding_top: Some(Length::Px(14.0).into()),
        padding_bottom: Some(Length::Px(14.0).into()),
        ..Default::default()
    });

    ui! {
        view(style = row_style) {
            Label(text = label, hex = "#111827", weight = FontWeight::Normal, size = 15.0)
            view(style = track_style) {
                view(style = knob_style) {}
                .bind(knob_ref)
            }
        }
        .on_touch(move |ev| {
            if ev.phase == TouchPhase::Ended {
                toggled.set(!toggled.get());
            }
            TouchResponse::CONSUMED
        })
    }
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
