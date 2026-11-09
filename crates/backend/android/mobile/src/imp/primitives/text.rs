//! `TextPrim` — `android.widget.TextView`.

use backend_android_core::helpers::set_text;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};

pub(crate) fn create(b: &mut AndroidBackend, content: &str) -> GlobalRef {
    // Built and configured JVM-side in one call: see `RustViewFactory.text`
    // for the font-padding, line-spacing, min-height and gravity settings
    // that make a TextView measure and sit like iOS's UILabel.
    let view = with_env(|env| {
        let content = env.new_string(content).expect("new_string(text content)");
        let local = TEXT_FACTORY
            .with(|slot| {
                let method = super::super::StaticMethod::cached(
                    env,
                    slot,
                    "io/idealyst/runtime/RustViewFactory",
                    "text",
                    "(Landroid/content/Context;Ljava/lang/String;)Landroid/widget/TextView;",
                )?;
                // SAFETY: the arguments match the signature above.
                unsafe {
                    method.call_object(
                        env,
                        &[JValue::Object(b.context.as_obj()).as_jni(), JValue::Object(&content).as_jni()],
                    )
                }
            })
            .expect("RustViewFactory.text failed — bundle the kotlin runtime");
        env.new_global_ref(local).unwrap()
    });
    // Install a Taffy measure function so flex layout can ask the
    // TextView how tall it needs to be for a given available width.
    // Without this, the framework's `Text` collapses to 0×0 and
    // every flex sibling around it gets a 0-height row — the welcome
    // screen's "Welcome to Idealyst" headline was the user-visible
    // symptom.
    let layout = b.layout_for_view(&view);
    let view_for_measure = view.clone();
    b.layout.set_measure_fn(
        layout,
        std::rc::Rc::new(move |known_dimensions, available_space| {
            let avail_w = known_dimensions
                .width
                .unwrap_or(match available_space.width {
                    runtime_layout::AvailableSpace::Definite(w) => w,
                    runtime_layout::AvailableSpace::MaxContent => f32::INFINITY,
                    runtime_layout::AvailableSpace::MinContent => min_content_width(&view_for_measure),
                });
            measure_textview(&view_for_measure, avail_w, known_dimensions)
        }),
    );
    view
}

// =============================================================================
// Styled runs — one TextView whose text is a SpannableString with one
// span-set per styled run. Android's text engine wraps the mixed-style
// paragraph as a single unit, and — unlike AppKit — TextView merges its
// own font/color properties into unspanned ranges, so only the DELTAS
// need spans and `apply_style`'s property writes never disturb them
// (spans live in the CharSequence, not on the view). Theme swaps
// re-realize through `caps::TextOps::update_styled_text` so
// run token colors resolve against the new theme.
// =============================================================================

/// `Spanned.SPAN_EXCLUSIVE_EXCLUSIVE` — the standard "text inserted at
/// the boundaries is not styled" flag.
const SPAN_EXCLUSIVE_EXCLUSIVE: i32 = 33;

pub(crate) fn create_styled(b: &mut AndroidBackend, runs: &[runtime_shared::TextRun]) -> GlobalRef {
    // Reuse the full plain-text path (font-padding strip, gravity,
    // measure_fn — `View.measure` reflects the spannable once set),
    // then swap the plain string for the spanned realization.
    let view = create(b, "");
    set_styled(&view, runs);
    view
}

/// Build the `SpannableString` and hand it to `TextView.setText`.
pub(crate) fn set_styled(view: &GlobalRef, runs: &[runtime_shared::TextRun]) {
    with_env(|env| {
        let plain = runtime_shared::styled_text::plain_text_of(runs);
        let jplain = env.new_string(&plain).unwrap();
        let sp_class = env.find_class("android/text/SpannableString").unwrap();
        let sp = env
            .new_object(
                &sp_class,
                "(Ljava/lang/CharSequence;)V",
                &[JValue::Object(&jplain)],
            )
            .unwrap();

        // Java string indices are UTF-16 code units, NOT byte offsets —
        // a Rust byte range would corrupt spans on any non-ASCII text.
        let mut start: i32 = 0;
        for run in runs {
            let len = run.text.encode_utf16().count() as i32;
            let end = start + len;
            if let Some(style) = &run.style {
                fn set_span(
                    env: &mut jni::JNIEnv,
                    sp: &jni::objects::JObject,
                    span: jni::objects::JObject,
                    start: i32,
                    end: i32,
                ) {
                    let _ = env.call_method(
                        sp,
                        "setSpan",
                        "(Ljava/lang/Object;III)V",
                        &[
                            JValue::Object(&span),
                            JValue::Int(start),
                            JValue::Int(end),
                            JValue::Int(SPAN_EXCLUSIVE_EXCLUSIVE),
                        ],
                    );
                }
                if let Some(bg) = &style.background {
                    if let Some(packed) =
                        backend_android_core::helpers::parse_color(&bg.resolve().0)
                    {
                        let cls = env.find_class("android/text/style/BackgroundColorSpan").unwrap();
                        let span = env.new_object(&cls, "(I)V", &[JValue::Int(packed)]).unwrap();
                        set_span(env, &sp, span, start, end);
                    }
                }
                if let Some(fg) = &style.color {
                    if let Some(packed) =
                        backend_android_core::helpers::parse_color(&fg.resolve().0)
                    {
                        let cls = env.find_class("android/text/style/ForegroundColorSpan").unwrap();
                        let span = env.new_object(&cls, "(I)V", &[JValue::Int(packed)]).unwrap();
                        set_span(env, &sp, span, start, end);
                    }
                }
                // Font family: generic-role mapping only. A CSS-ish stack
                // containing a monospace/serif generic maps to Android's
                // built-in family alias via `TypefaceSpan(String)`; named
                // faces (SF Mono, Menlo) don't exist on Android and a
                // registered custom `Typeface` span needs API 28 —
                // documented gap, the generic covers the inline-code case.
                if let Some(runtime_shared::FontFamily::System(stack)) = &style.font_family {
                    let lower = stack.to_ascii_lowercase();
                    let family = if lower.contains("monospace") {
                        Some("monospace")
                    } else if lower.contains("serif") && !lower.contains("sans-serif") {
                        Some("serif")
                    } else {
                        None
                    };
                    if let Some(family) = family {
                        let jfam = env.new_string(family).unwrap();
                        let cls = env.find_class("android/text/style/TypefaceSpan").unwrap();
                        let span = env
                            .new_object(&cls, "(Ljava/lang/String;)V", &[JValue::Object(&jfam)])
                            .unwrap();
                        set_span(env, &sp, span, start, end);
                    }
                }
                // Weight: Spannable has no fine-grained weight span below
                // API 28; SemiBold+ maps to the bold StyleSpan, matching
                // how the plain-text path's `Typeface.defaultFromStyle`
                // quantizes weights.
                if let Some(w) = style.font_weight {
                    if matches!(
                        w,
                        runtime_shared::FontWeight::SemiBold
                            | runtime_shared::FontWeight::Bold
                            | runtime_shared::FontWeight::ExtraBold
                            | runtime_shared::FontWeight::Black
                    ) {
                        let cls = env.find_class("android/text/style/StyleSpan").unwrap();
                        // Typeface.BOLD = 1
                        let span = env.new_object(&cls, "(I)V", &[JValue::Int(1)]).unwrap();
                        set_span(env, &sp, span, start, end);
                    }
                }
                // Italic composes with the bold span above rather than
                // replacing it: `StyleSpan.updateDrawState` ORs its own
                // style into the typeface's existing one, so BOLD + ITALIC
                // on the same range resolves to the bold-italic face.
                if style.font_style == Some(runtime_shared::FontStyle::Italic) {
                    let cls = env.find_class("android/text/style/StyleSpan").unwrap();
                    // Typeface.ITALIC = 2
                    let span = env.new_object(&cls, "(I)V", &[JValue::Int(2)]).unwrap();
                    set_span(env, &sp, span, start, end);
                }
                if let Some(u) = &style.underline {
                    // `UnderlineSpan` can only draw a solid line in the
                    // text's own colour, so anything with a pattern or a
                    // colour of its own goes through the framework's
                    // drawing span (see RustUnderlineSpan.kt for why
                    // Spannable cannot express this).
                    let pattern = match u.style {
                        runtime_shared::UnderlineStyle::Solid => 0,
                        runtime_shared::UnderlineStyle::Dotted => 1,
                        runtime_shared::UnderlineStyle::Dashed => 2,
                    };
                    let packed = u
                        .color
                        .as_ref()
                        .and_then(|c| backend_android_core::helpers::parse_color(&c.resolve().0));
                    if pattern == 0 && packed.is_none() {
                        let cls = env.find_class("android/text/style/UnderlineSpan").unwrap();
                        let span = env.new_object(&cls, "()V", &[]).unwrap();
                        set_span(env, &sp, span, start, end);
                    } else if let Ok(cls) =
                        env.find_class("io/idealyst/runtime/RustUnderlineSpan")
                    {
                        // INHERIT_COLOR = 0 (fully transparent black is
                        // never a legible underline, so it is free to
                        // stand in for "follow the text").
                        let span = env.new_object(
                            &cls,
                            "(Landroid/widget/TextView;II)V",
                            &[
                                JValue::Object(view.as_obj()),
                                JValue::Int(packed.unwrap_or(0)),
                                JValue::Int(pattern),
                            ],
                        );
                        match span {
                            Ok(span) => set_span(env, &sp, span, start, end),
                            Err(e) => {
                                // Clear the pending exception before any
                                // further JNI call.
                                if env.exception_check().unwrap_or(false) {
                                    let _ = env.exception_clear();
                                }
                                log::error!("RustUnderlineSpan construction failed: {e}");
                            }
                        }
                    }
                }
                if let Some(runtime_shared::Length::Px(px)) =
                    style.font_size.as_ref().map(|t| t.resolve())
                {
                    // dip=true: framework px are density-independent, the
                    // same convention `setTextSize(COMPLEX_UNIT_DIP, …)`
                    // uses in `apply_style`.
                    let cls = env.find_class("android/text/style/AbsoluteSizeSpan").unwrap();
                    let span = env
                        .new_object(
                            &cls,
                            "(IZ)V",
                            &[JValue::Int(px.round() as i32), JValue::Bool(1)],
                        )
                        .unwrap();
                    set_span(env, &sp, span, start, end);
                }
            }
            start = end;
        }

        let _ = env.call_method(
            view.as_obj(),
            "setText",
            "(Ljava/lang/CharSequence;)V",
            &[JValue::Object(&sp)],
        );
    });
}

/// The TextView's min-content width in dp — its widest word, from the
/// Kotlin `RustTextMeasure.minContentWidth`. A single-line view answers 0:
/// the `AT_MOST 0` probe, which truncates a one-line label to its
/// ellipsis. Asked once per view per layout pass (`MIN_CONTENT_MEMO`).
fn min_content_width(view: &GlobalRef) -> f32 {
    with_env(|env| {
        let view_obj = view.as_obj();
        let density = super::super::cached_density(env, &view_obj);
        let key = view_obj.as_raw() as usize;
        let cached = super::super::MIN_CONTENT_MEMO.with(|m| m.borrow().get(&key));
        let px = cached.unwrap_or_else(|| {
            let _t = crate::phase_timer::PhaseTimer::start("min_content_probe");
            let px = MIN_CONTENT_WIDTH
                .with(|slot| {
                    let method = super::super::StaticMethod::cached(
                        env,
                        slot,
                        "io/idealyst/runtime/RustTextMeasure",
                        "minContentWidth",
                        "(Landroid/widget/TextView;)I",
                    )?;
                    // SAFETY: the argument matches `(Landroid/widget/TextView;)I`.
                    Some(unsafe { method.call_int(env, &[JValue::Object(&view_obj).as_jni()]) })
                })
                .unwrap_or(0);
            super::super::MIN_CONTENT_MEMO.with(|m| m.borrow_mut().put(key, px));
            px
        });
        px as f32 / density
    })
}

runtime_tls::thread_local! {
    /// `RustViewFactory.text`, resolved once.
    static TEXT_FACTORY: std::cell::RefCell<Option<super::super::StaticMethod>> =
        std::cell::RefCell::new(None);

    /// `RustTextMeasure.minContentWidth`, resolved once.
    static MIN_CONTENT_WIDTH: std::cell::RefCell<Option<super::super::StaticMethod>> =
        std::cell::RefCell::new(None);
}

/// Ask the TextView how big it wants to be for a given `available_width`:
/// `View.measure` at `AT_MOST width` (or `UNSPECIFIED` when unbounded), with
/// no height bound, through [`super::super::probe_measure`], in dp.
fn measure_textview(
    view: &GlobalRef,
    avail_w_dp: f32,
    known_dimensions: runtime_layout::Size<Option<f32>>,
) -> runtime_layout::Size<f32> {
    use crate::measure_memo_policy::{at_most_or_unspecified, UNSPECIFIED};
    with_env(|env| {
        let view_obj = view.as_obj();
        let density = super::super::cached_density(env, &view_obj);
        let width_spec = at_most_or_unspecified(avail_w_dp, density);
        // No height constraint: UNSPECIFIED → TextView picks its
        // natural height for the given width. A `max_lines` limit
        // (`setMaxLines` + `setEllipsize(END)`, `imp/style.rs`) is
        // honoured here by TextView's own `onMeasure`: the height is
        // capped at that many lines and a 1-line limit measures one
        // ellipsized line, so no limit-specific code (or cache key) is
        // needed — the node is re-dirtied on every style apply.
        let (measured_w_px, measured_h_px) =
            super::super::probe_measure(env, &view_obj, width_spec, UNSPECIFIED);
        // `View.measure()` returns `text + padding` (Android includes
        // setPadding in the measured size). We do NOT subtract that
        // back out here, because the framework's `apply_style` already
        // strips `padding_*` from the Taffy leaf style for TextView
        // nodes (see `imp::mod.rs::apply_style` text-view branch).
        // Taffy's outer-node size equals what `measure_fn` returns, so
        // returning measured (text + setPadding) yields outer = text +
        // padding once — `setPadding` then insets the glyphs visually
        // within that frame. Subtracting here would give an outer
        // frame too small for the glyphs to render in.
        let w_dp = measured_w_px as f32 / density;
        let h_dp = measured_h_px as f32 / density;
        runtime_layout::Size {
            width: known_dimensions.width.unwrap_or(w_dp.ceil()),
            height: known_dimensions.height.unwrap_or(h_dp.ceil()),
        }
    })
}

pub(crate) fn update_text(node: &GlobalRef, content: &str) {
    with_env(|env| set_text(env, &node.as_obj(), content));
}
