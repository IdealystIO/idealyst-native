//! Style application: walks a `StyleRules` and applies each property
//! to the native View. Animated properties spawn (or restart) an
//! animator from [`super::animation`]; snap properties go through the
//! straightforward setter call.
//!
//! For nodes with border or non-zero corner radius we route through a
//! per-node `GradientDrawable` so corner/stroke animations can mutate
//! a single drawable instead of rebuilding it every frame.

use super::animation::*;
use backend_android_core::helpers::*;
use super::font::{apply_resolved_font_to_textview, FontRegistry};
use super::NodeAnim;
use runtime_core::StyleRules;
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;

pub(crate) fn apply_rules(
    env: &mut JNIEnv,
    node: &GlobalRef,
    state: &mut NodeAnim,
    rules: &StyleRules,
    font_registry: &FontRegistry,
) {
    let _t_total = crate::phase_timer::PhaseTimer::start("apply_style_total");
    let view = node.as_obj();

    // --- Padding.
    //
    // **Container views**: padding is a Taffy concept — it shifts
    // children's computed positions inside the parent's box, and
    // Taffy writes those shifted positions to each child's
    // `LayoutParams.leftMargin` / `topMargin`. Calling Android's
    // `setPadding` on top would double-shift, so we explicitly write
    // zero to clear any prior values.
    //
    // **Text views**: padding goes to `TextView.setPadding(L, T, R,
    // B)` instead. TextView is a Taffy leaf — there are no children
    // to shift. The mobile backend's `apply_style` (in `imp::mod.rs`)
    // already strips `padding_*` from the Taffy style it hands to
    // `LayoutTree::set_style` for TextView nodes, so Taffy's outer
    // size is just what `measure_textview` returns — which is the
    // widget's full measured size including its own setPadding.
    // `setPadding` then insets the glyphs visually within that frame.
    // (If apply_style ever stops stripping padding for text leaves,
    // `measure_textview` would have to subtract `getPadding*` to
    // avoid double-counting — they're paired.)
    //
    // Net result: authors can write `padding_*` on a text style and
    // it just works — no `XStyle + XText` split required.
    let is_text_view = super::is_text_view(env, &view);
    let want_padding = if is_text_view {
        [
            dp_to_px(env, &view, px_or(rules.padding_left.as_ref(), 0.0)),
            dp_to_px(env, &view, px_or(rules.padding_top.as_ref(), 0.0)),
            dp_to_px(env, &view, px_or(rules.padding_right.as_ref(), 0.0)),
            dp_to_px(env, &view, px_or(rules.padding_bottom.as_ref(), 0.0)),
        ]
    } else {
        [0, 0, 0, 0]
    };
    let padding_transitions = [
        rules.padding_left_transition,
        rules.padding_top_transition,
        rules.padding_right_transition,
        rules.padding_bottom_transition,
    ];
    let any_padding_changed = (0..4).any(|i| state.last_padding[i] != Some(want_padding[i]));
    if any_padding_changed {
        // Snap: any side without a transition just gets its new value
        // applied immediately via setPadding. We do this in one
        // setPadding call covering all four sides — animated sides
        // will overwrite their own value on each tick.
        env.call_method(
            &view,
            "setPadding",
            "(IIII)V",
            &[
                JValue::Int(want_padding[0]),
                JValue::Int(want_padding[1]),
                JValue::Int(want_padding[2]),
                JValue::Int(want_padding[3]),
            ],
        )
        .unwrap();
        // For sides with a transition, start an animator from the
        // PREVIOUS value to the new value. This intentionally runs
        // *after* the setPadding above — the animator will write
        // intermediate values on its update callback, overriding
        // the snap-applied target until it reaches `to`.
        for i in 0..4 {
            let new_val = want_padding[i];
            let old_val = state.last_padding[i];
            state.last_padding[i] = Some(new_val);
            if let (Some(from), Some(t)) = (old_val, padding_transitions[i]) {
                if from != new_val {
                    cancel_animator(env, state.anim_padding[i].take());
                    let side_index = i as i32; // 0..3 = L,T,R,B
                    let anim =
                        start_padding_animator(env, node, side_index, from, new_val, t);
                    state.anim_padding[i] = anim;
                }
            }
        }
    }

    // --- Width / height. Read the current LayoutParams, mutate the
    //     relevant fields, and set them back. Length::Px is
    //     interpreted as dp (matches how `padding_*` is treated).
    //     Length::Percent isn't expressible in Android's flat
    //     LayoutParams; treat as MATCH_PARENT (-1). Length::Auto
    //     means WRAP_CONTENT (-2). If neither width nor height is
    //     set, we don't touch LayoutParams at all so the parent's
    //     defaults stand.
    if rules.width.is_some() || rules.height.is_some() {
        // Resolve up front to avoid mutably borrowing `env` from
        // inside a closure (the JNI calls all take `&mut JNIEnv`).
        // `Tokenized<Length>` — resolve to underlying via `.resolve()`;
        // native has no CSS-variable equivalent so the resolved
        // current value is the value at apply-time, and the
        // subscription ties this Effect to the token's signal.
        // `.resolve()` subscribes the apply-style Effect to the per-
        // token signal for any tokenized width/height — native has no
        // CSS-variable equivalent, so we always materialize to the
        // current value.
        let w_lp = rules.width.as_ref().map(|tok| match tok.resolve() {
            runtime_core::Length::Px(v) => dp_to_px(env, &view, v),
            runtime_core::Length::Percent(_) => -1,
            runtime_core::Length::Auto => -2,
        });
        let h_lp = rules.height.as_ref().map(|tok| match tok.resolve() {
            runtime_core::Length::Px(v) => dp_to_px(env, &view, v),
            runtime_core::Length::Percent(_) => -1,
            runtime_core::Length::Auto => -2,
        });
        // getLayoutParams() may return null if the view hasn't been
        // attached to a parent yet. Build a fresh
        // ViewGroup.LayoutParams in that case; the parent will swap
        // it for its own subclass on attach, copying our width/height
        // values across.
        let lp_obj = env
            .call_method(
                &view,
                "getLayoutParams",
                "()Landroid/view/ViewGroup$LayoutParams;",
                &[],
            )
            .ok()
            .and_then(|v| v.l().ok());
        let lp = match lp_obj {
            Some(o) if !o.is_null() => o,
            _ => {
                let lp_class = env
                    .find_class("android/view/ViewGroup$LayoutParams")
                    .unwrap();
                env.new_object(&lp_class, "(II)V", &[JValue::Int(-2), JValue::Int(-2)])
                    .unwrap()
            }
        };
        if let Some(w) = w_lp {
            let _ = env.set_field(&lp, "width", "I", JValue::Int(w));
        }
        if let Some(h) = h_lp {
            let _ = env.set_field(&lp, "height", "I", JValue::Int(h));
        }
        let _ = env.call_method(
            &view,
            "setLayoutParams",
            "(Landroid/view/ViewGroup$LayoutParams;)V",
            &[JValue::Object(&lp)],
        );
    }

    // --- Text color + font size (no-op for views that aren't TextView).
    // Reuse the `is_text_view` we computed above so we don't pay for a
    // second `find_class` + `is_instance_of` on every styled view.
    let is_textview = is_text_view;

    if is_textview {
        if let Some(c) = rules.color.as_ref().map(|t| t.resolve()) {
            if let Some(packed) = parse_color(&c.0) {
                let prev = state.last_text_color;
                let changed = prev != Some(packed);
                state.last_text_color = Some(packed);
                if changed {
                    match (prev, rules.color_transition) {
                        (Some(from), Some(t)) if from != packed => {
                            cancel_animator(env, state.anim_text_color.take());
                            state.anim_text_color =
                                start_argb_animator(env, node, "textColor", from, packed, t);
                        }
                        _ => {
                            let _ = env.call_method(
                                &view,
                                "setTextColor",
                                "(I)V",
                                &[JValue::Int(packed)],
                            );
                        }
                    }
                }
            }
        }
        if let Some(runtime_core::Length::Px(size)) =
            rules.font_size.as_ref().map(|t| t.resolve())
        {
            // font-size isn't animatable in v1; snap.
            let _ = env.call_method(
                &view,
                "setTextSize",
                "(IF)V",
                &[JValue::Int(1), JValue::Float(size)],
            );
        }
        // Font family / weight / style — route through the registry
        // so a `font_family: &INTER` style targets the right
        // registered face, and `FontFamily::System("monospace")` /
        // bare `font_weight: Bold` reach `Typeface.create` /
        // `Typeface.defaultFromStyle`. Apply only when any typography
        // slot is set; views without an explicit typography rule
        // keep their platform default.
        let has_typography_family = rules.font_family.is_some()
            || rules.font_weight.is_some()
            || rules.font_style.is_some();
        if has_typography_family {
            let weight = rules
                .font_weight
                .as_ref()
                .copied()
                .unwrap_or(runtime_core::FontWeight::Normal);
            let fstyle = rules
                .font_style
                .as_ref()
                .copied()
                .unwrap_or(runtime_core::FontStyle::Normal);
            apply_resolved_font_to_textview(
                env,
                &view,
                font_registry,
                rules.font_family.as_ref(),
                weight,
                fstyle,
            );
        }
        // text-align → TextView.setGravity. Without this multi-line
        // wrapped text lays out left-justified inside the (often
        // wider-than-the-line) text-box, which makes a `text_align:
        // Center` author intent silently no-op on Android. Gravity
        // constants: LEFT=3, CENTER_HORIZONTAL=1, RIGHT=5. Default
        // gravity is LEFT|TOP — we keep TOP and only override the
        // horizontal axis.
        if let Some(align) = rules.text_align {
            let gravity = match align {
                runtime_core::TextAlign::Left => 3 | 48,           // LEFT | TOP
                runtime_core::TextAlign::Center => 1 | 48,         // CENTER_HORIZONTAL | TOP
                runtime_core::TextAlign::Right => 5 | 48,          // RIGHT | TOP
                runtime_core::TextAlign::Justify => 3 | 48,        // No JUSTIFY mode on TextView v1; fall back to LEFT
            };
            let _ = env.call_method(
                &view,
                "setGravity",
                "(I)V",
                &[JValue::Int(gravity)],
            );
        }
        // Caret color → `setTextCursorDrawable` with a GradientDrawable
        // fill. API 29+ only; on older Android we silently drop back to
        // the theme default (the JNI call resolves to a missing method
        // and errors, which we ignore). `caret_color_transition` is
        // declared on `StyleRules` but not honored here — animating the
        // cursor drawable would require a custom Drawable subclass; for
        // v1 we snap on Android even when iOS/web tween smoothly. The
        // mismatch is documented; revisit if a use case demands parity.
        if let Some(c) = rules.caret_color.as_ref().map(|t| t.resolve()) {
            if let Some(packed) = parse_color(&c.0) {
                let prev = state.last_caret_color;
                let changed = prev != Some(packed);
                state.last_caret_color = Some(packed);
                if changed {
                    let _ = (|| -> jni::errors::Result<()> {
                        let gd_class = env
                            .find_class("android/graphics/drawable/GradientDrawable")?;
                        let drawable = env.new_object(&gd_class, "()V", &[])?;
                        env.call_method(
                            &drawable,
                            "setColor",
                            "(I)V",
                            &[JValue::Int(packed)],
                        )?;
                        // Intrinsic width = 2 px (matches the system
                        // default caret thickness). Height is ignored:
                        // TextView always passes its own line-height
                        // bounds via `setBounds` before drawing.
                        env.call_method(
                            &drawable,
                            "setSize",
                            "(II)V",
                            &[JValue::Int(2), JValue::Int(0)],
                        )?;
                        env.call_method(
                            &view,
                            "setTextCursorDrawable",
                            "(Landroid/graphics/drawable/Drawable;)V",
                            &[JValue::Object(&drawable)],
                        )?;
                        Ok(())
                    })();
                }
            }
        }
    }

    // --- Opacity (View.alpha). Animatable via ObjectAnimator.ofFloat.
    //     `rules.opacity` is `Option<Tokenized<f32>>`; resolve to the
    //     concrete value (native has no token system).
    if let Some(o) = rules.opacity.as_ref().map(|t| t.resolve()) {
        let changed = state.last_alpha.map(|p| (p - o).abs() > 0.001).unwrap_or(true);
        let prev = state.last_alpha;
        state.last_alpha = Some(o);
        if changed {
            match (prev, rules.opacity_transition) {
                (Some(from), Some(t)) if (from - o).abs() > 0.001 => {
                    cancel_animator(env, state.anim_alpha.take());
                    state.anim_alpha = start_float_animator(env, node, "alpha", from, o, t);
                }
                _ => {
                    let _ = env.call_method(&view, "setAlpha", "(F)V", &[JValue::Float(o)]);
                }
            }
        }
    }

    // --- Background + border + radius. If any border or radius is
    //     present we route through a persistent `GradientDrawable`
    //     so we can mutate corners/stroke/fill on each animator
    //     tick instead of rebuilding the drawable. Otherwise the
    //     simple `setBackgroundColor` path covers it.
    let has_border = rules.border_top_width.is_some()
        || rules.border_right_width.is_some()
        || rules.border_bottom_width.is_some()
        || rules.border_left_width.is_some();
    let has_radius = rules.border_top_left_radius.is_some()
        || rules.border_top_right_radius.is_some()
        || rules.border_bottom_left_radius.is_some()
        || rules.border_bottom_right_radius.is_some();
    let has_gradient = rules.background_gradient.is_some();

    if has_border || has_radius || has_gradient {
        let _t = crate::phase_timer::PhaseTimer::start("apply_drawable_path");
        apply_drawable_path(env, node, state, rules);
    } else if let Some(c) = rules.background.as_ref().map(|t| t.resolve()) {
        if let Some(packed) = parse_color(&c.0) {
            let prev = state.last_bg;
            let changed = prev != Some(packed);
            state.last_bg = Some(packed);
            if changed {
                match (prev, rules.background_transition) {
                    (Some(from), Some(t)) if from != packed => {
                        cancel_animator(env, state.anim_bg.take());
                        state.anim_bg = start_argb_animator(
                            env,
                            node,
                            "backgroundColor",
                            from,
                            packed,
                            t,
                        );
                    }
                    _ => {
                        let _ = env.call_method(
                            &view,
                            "setBackgroundColor",
                            "(I)V",
                            &[JValue::Int(packed)],
                        );
                    }
                }
            }
        }
    }

    // --- Transform. Walks the optional `Vec<Transform>` and applies
    //     each operation via the matching `View` setter:
    //     `setTranslationX/Y`, `setScaleX/Y`, `setRotation`. Length
    //     values are dp (Android convention), converted to px before
    //     setting. Skew isn't supported on `View` directly — would
    //     need a `Matrix` + custom drawable — skipped for now.
    //
    //     `None` resets all transform properties to identity so a
    //     style change that *removes* the transform reverts the
    //     view. This is the hot path for pan / drag interactions.
    apply_transform(env, &view, state, rules);
}

fn apply_transform(
    env: &mut JNIEnv,
    view: &JObject,
    state: &mut NodeAnim,
    rules: &StyleRules,
) {
    use runtime_core::{Length, Transform};
    // Default identity values. The loop overwrites them if matching
    // ops appear in `transform`. Percent translates are stashed on
    // `state` instead of converted here — translate-% is CSS-spec
    // BOX-relative, and the box's pixel size isn't known until
    // Taffy lays out. `sync_transform_translate_percent` (called
    // from the layout pass) reads the stashed values and writes
    // `setTranslationX/Y` with the resolved px.
    let mut tx_dp: f32 = 0.0;
    let mut ty_dp: f32 = 0.0;
    let mut pct_x: Option<f32> = None;
    let mut pct_y: Option<f32> = None;
    let mut sx: f32 = 1.0;
    let mut sy: f32 = 1.0;
    let mut rot_deg: f32 = 0.0;
    if let Some(ops) = rules.transform.as_ref() {
        for op in ops {
            match op {
                Transform::TranslateX(Length::Px(v)) => tx_dp = *v,
                Transform::TranslateY(Length::Px(v)) => ty_dp = *v,
                Transform::TranslateX(Length::Percent(v)) => pct_x = Some(*v),
                Transform::TranslateY(Length::Percent(v)) => pct_y = Some(*v),
                Transform::TranslateX(Length::Auto) | Transform::TranslateY(Length::Auto) => {
                    // `Auto` makes no sense for translate — treat as 0.
                }
                Transform::Scale(v) => {
                    sx = *v;
                    sy = *v;
                }
                Transform::ScaleXY { x, y } => {
                    sx = *x;
                    sy = *y;
                }
                Transform::Rotate(deg) => rot_deg = *deg,
                // Skew not representable as a flat `View` property —
                // would require a `Matrix` on a custom drawable. Skip.
                Transform::SkewX(_) | Transform::SkewY(_) => {}
            }
        }
    }
    state.transform_translate_pct_x = pct_x;
    state.transform_translate_pct_y = pct_y;
    // Convert dp → px for px translations; the percent translates
    // are resolved later when the view has real bounds.
    let tx_px = dp_to_px(env, view, tx_dp) as f32;
    let ty_px = dp_to_px(env, view, ty_dp) as f32;
    let _ = env.call_method(view, "setTranslationX", "(F)V", &[JValue::Float(tx_px)]);
    let _ = env.call_method(view, "setTranslationY", "(F)V", &[JValue::Float(ty_px)]);
    let _ = env.call_method(view, "setScaleX", "(F)V", &[JValue::Float(sx)]);
    let _ = env.call_method(view, "setScaleY", "(F)V", &[JValue::Float(sy)]);
    let _ = env.call_method(view, "setRotation", "(F)V", &[JValue::Float(rot_deg)]);
}

/// Resolve any `transform: translate(%, %)` requests stashed on
/// `state` against the view's just-laid-out dp dimensions
/// (`width_dp` / `height_dp` come straight from the Taffy frame,
/// which reasons in dp — see `viewport_size()`). Writes
/// `setTranslationX/Y` only when a percent translate is actually
/// requested — px-only translates were applied at style time and
/// don't go through this path.
///
/// CSS `translate: %` is BOX-relative, hence the multiply against
/// the box's own width / height. Android's `setTranslationX/Y`
/// expects DEVICE PIXELS (not dp), so the resolved dp shift is
/// converted via the same `dp_to_px` helper the px-path uses.
/// Resolve a radial `background_gradient`'s reference radius
/// against the view's just-laid-out pixel dimensions and call
/// `GradientDrawable.setGradientRadius`. At apply-style time the
/// view hadn't been measured (`getMeasuredWidth/Height` returned
/// 0) so the apply path wrote a placeholder; this overwrites it
/// with the real radius. Iterates `state.gradient_radial_extent`
/// + `state.gradient_radial_radius_factor` + `state.drawable`,
/// and skips when any of them is `None` (linear / no-gradient
/// path).
pub(crate) fn sync_radial_gradient_radius(
    env: &mut JNIEnv,
    state: &NodeAnim,
    width_dp: f32,
    height_dp: f32,
    density: f32,
) {
    let (Some(extent), Some(factor), Some(drawable)) = (
        state.gradient_radial_extent,
        state.gradient_radial_radius_factor,
        state.drawable.as_ref(),
    ) else {
        return;
    };
    if width_dp <= 0.0 || height_dp <= 0.0 {
        return;
    }
    let half_w_px = width_dp * 0.5 * density;
    let half_h_px = height_dp * 0.5 * density;
    let reference_px = match extent {
        runtime_core::RadialExtent::ClosestSide => half_w_px.min(half_h_px),
        runtime_core::RadialExtent::FarthestCorner => {
            (half_w_px * half_w_px + half_h_px * half_h_px).sqrt()
        }
    };
    let radius_px = reference_px * factor;
    let _ = env.call_method(
        drawable.as_obj(),
        "setGradientRadius",
        "(F)V",
        &[JValue::Float(radius_px)],
    );
}

pub(crate) fn sync_transform_translate_percent(
    env: &mut JNIEnv,
    view: &JObject,
    state: &NodeAnim,
    width_dp: f32,
    height_dp: f32,
) {
    if let Some(pct_x) = state.transform_translate_pct_x {
        let tx_dp = width_dp * (pct_x / 100.0);
        let tx_px = dp_to_px(env, view, tx_dp) as f32;
        let _ = env.call_method(view, "setTranslationX", "(F)V", &[JValue::Float(tx_px)]);
    }
    if let Some(pct_y) = state.transform_translate_pct_y {
        let ty_dp = height_dp * (pct_y / 100.0);
        let ty_px = dp_to_px(env, view, ty_dp) as f32;
        let _ = env.call_method(view, "setTranslationY", "(F)V", &[JValue::Float(ty_px)]);
    }
}

/// Background path for nodes that have a border or non-zero corner
/// radius. Uses a per-node `GradientDrawable` so corner radius and
/// stroke can animate without re-allocating.
fn apply_drawable_path(
    env: &mut JNIEnv,
    node: &GlobalRef,
    state: &mut NodeAnim,
    rules: &StyleRules,
) {
    let view = node.as_obj();

    // Ensure the drawable exists and is attached as the view's
    // background. We do this once per node — subsequent applies
    // mutate the drawable in place.
    if state.drawable.is_none() {
        let class = env
            .find_class("android/graphics/drawable/GradientDrawable")
            .unwrap();
        let drawable_local = env.new_object(&class, "()V", &[]).unwrap();
        let _ = env.call_method(
            &view,
            "setBackground",
            "(Landroid/graphics/drawable/Drawable;)V",
            &[JValue::Object(&drawable_local)],
        );
        state.drawable = Some(env.new_global_ref(&drawable_local).unwrap());
    }
    let drawable = state.drawable.as_ref().unwrap().clone();
    let drawable_obj = drawable.as_obj();

    // --- Gradient fill. `GradientDrawable` natively supports linear,
    //     radial, and sweep gradients. We honor the `background_gradient`
    //     field by calling `setColors(int[])` + `setGradientType(..)`
    //     + the type-specific setters (`setOrientation` for linear,
    //     `setGradientRadius` for radial). When the gradient slot is
    //     `None`, fall through to the flat-fill `setColor` below so
    //     authors can toggle gradient ↔ solid via reactive updates.
    if let Some(g) = rules.background_gradient.as_ref() {
        apply_gradient_to_drawable(env, &view, &drawable_obj, g, state);
    } else {
        // Re-set the gradient type to the default (linear, all-equal
        // colors → solid) so a previous gradient drawable becomes a
        // plain fill again. `GradientDrawable.LINEAR_GRADIENT = 0`.
        let _ = env.call_method(
            &drawable_obj,
            "setGradientType",
            "(I)V",
            &[JValue::Int(0)],
        );
    }

    // --- Fill color (no-op when gradient is set — `setColors` already
    //     wrote the per-stop fill, and authors mixing both would be
    //     overridden one way or the other; the gradient wins).
    if rules.background_gradient.is_none() {
        if let Some(c) = rules.background.as_ref().map(|t| t.resolve()) {
            if let Some(packed) = parse_color(&c.0) {
                let prev = state.last_bg;
                let changed = prev != Some(packed);
                state.last_bg = Some(packed);
                if changed {
                    match (prev, rules.background_transition) {
                        (Some(from), Some(t)) if from != packed => {
                            cancel_animator(env, state.anim_bg.take());
                            state.anim_bg =
                                start_drawable_argb_animator(env, &drawable, "color", from, packed, t);
                        }
                        _ => {
                            let _ = env.call_method(
                                &drawable_obj,
                                "setColor",
                                "(I)V",
                                &[JValue::Int(packed)],
                            );
                        }
                    }
                }
            }
        }
    }

    // --- Per-side borders.
    //
    // Android's `GradientDrawable.setStroke(width, color)` is uniform-
    // only (all four sides get the same width + color). The framework's
    // CSS-style API lets authors set `border_bottom_width: 1` without
    // touching the other three sides, which never had a sane mapping
    // onto `setStroke`. We instead paint each side ourselves via the
    // `RustBorderDrawable` Kotlin helper, attached as the View's
    // foreground (renders over content + children, matching the
    // CSS "border sits on the box edge" model).
    //
    // This mirrors the iOS backend's per-side `UIView` subview
    // approach (`install_border_side` in
    // `backend-ios-core/src/style.rs`). A custom Drawable is cleaner
    // than adding chrome subviews on Android because it doesn't
    // create extra Views that Taffy / the layout-pass walker /
    // hit-testing need to special-case.
    //
    // Snap-only for now — animating per-side borders would need a
    // ValueAnimator per side. Authors transitioning whole-card
    // border-color (the only common case) won't notice; the previous
    // `setStroke` animator path only covered the uniform border
    // anyway.
    //
    // Clear any stale `GradientDrawable` stroke from before this fix
    // landed on a node — otherwise the bottom of a card would show
    // the per-side bottom border PLUS a uniform 1px stroke from the
    // drawable underneath.
    if state.last_stroke_w.is_some() {
        let _ = env.call_method(
            &drawable_obj,
            "setStroke",
            "(II)V",
            &[JValue::Int(0), JValue::Int(0)],
        );
        state.last_stroke_w = None;
        state.last_stroke_color = None;
        cancel_animator(env, state.anim_stroke_w.take());
    }

    let widths_px: [i32; 4] = [
        rules
            .border_top_width
            .as_ref()
            .map(|t| dp_to_px(env, &view, t.resolve()))
            .unwrap_or(0),
        rules
            .border_right_width
            .as_ref()
            .map(|t| dp_to_px(env, &view, t.resolve()))
            .unwrap_or(0),
        rules
            .border_bottom_width
            .as_ref()
            .map(|t| dp_to_px(env, &view, t.resolve()))
            .unwrap_or(0),
        rules
            .border_left_width
            .as_ref()
            .map(|t| dp_to_px(env, &view, t.resolve()))
            .unwrap_or(0),
    ];
    // Per-CSS, an unset per-side color falls back to the first
    // border-color that IS set on any side. Lets authors write
    // `border_color: "black"` once and `border_*_width` per side
    // without having to repeat the color four times.
    let fallback_color = rules
        .border_top_color
        .as_ref()
        .or(rules.border_right_color.as_ref())
        .or(rules.border_bottom_color.as_ref())
        .or(rules.border_left_color.as_ref())
        .and_then(|c| parse_color(&c.resolve().0))
        .unwrap_or(0); // 0 = fully transparent; renders nothing
    let colors_argb: [i32; 4] = [
        rules
            .border_top_color
            .as_ref()
            .and_then(|c| parse_color(&c.resolve().0))
            .unwrap_or(fallback_color),
        rules
            .border_right_color
            .as_ref()
            .and_then(|c| parse_color(&c.resolve().0))
            .unwrap_or(fallback_color),
        rules
            .border_bottom_color
            .as_ref()
            .and_then(|c| parse_color(&c.resolve().0))
            .unwrap_or(fallback_color),
        rules
            .border_left_color
            .as_ref()
            .and_then(|c| parse_color(&c.resolve().0))
            .unwrap_or(fallback_color),
    ];

    let _t_border = crate::phase_timer::PhaseTimer::start("border_path");
    let any_border = widths_px.iter().any(|w| *w > 0);
    let state_changed = (0..4).any(|i| {
        state.last_border_widths[i] != Some(widths_px[i])
            || state.last_border_colors[i] != Some(colors_argb[i])
    });
    // Pick a single transition for all sides — first non-None in
    // (top, right, bottom, left) order, across both width and color.
    // Matches the iOS path's "borders snap" simplification at the
    // semantic level: authors typically transition a single
    // `border_color` across all sides at the same timing, and the
    // ValueAnimator drives all four sides at the same fraction.
    let any_transition = rules
        .border_top_color_transition
        .or(rules.border_right_color_transition)
        .or(rules.border_bottom_color_transition)
        .or(rules.border_left_color_transition)
        .or(rules.border_top_width_transition)
        .or(rules.border_right_width_transition)
        .or(rules.border_bottom_width_transition)
        .or(rules.border_left_width_transition);
    if state_changed {
        let prev_w = [
            state.last_border_widths[0],
            state.last_border_widths[1],
            state.last_border_widths[2],
            state.last_border_widths[3],
        ];
        let prev_c = [
            state.last_border_colors[0],
            state.last_border_colors[1],
            state.last_border_colors[2],
            state.last_border_colors[3],
        ];
        for i in 0..4 {
            state.last_border_widths[i] = Some(widths_px[i]);
            state.last_border_colors[i] = Some(colors_argb[i]);
        }
        if any_border {
            // Create the drawable on first use; reuse the same
            // instance on subsequent applies so we don't churn the
            // GC. `setForeground` is idempotent — calling it on the
            // same instance multiple times is fine.
            let drawable_ref = match state.border_drawable.clone() {
                Some(d) => d,
                None => {
                    let class = match env.find_class("io/idealyst/runtime/RustBorderDrawable") {
                        Ok(c) => c,
                        Err(e) => {
                            log::error!(
                                "RustBorderDrawable class missing — make sure the CLI was \
                                 reinstalled after adding the runtime registry entry. \
                                 Underlying: {:?}",
                                e
                            );
                            return;
                        }
                    };
                    let Ok(local) = env.new_object(&class, "()V", &[]) else {
                        log::error!("new RustBorderDrawable failed");
                        return;
                    };
                    let Ok(g) = env.new_global_ref(local) else {
                        return;
                    };
                    let _ = env.call_method(
                        &view,
                        "setForeground",
                        "(Landroid/graphics/drawable/Drawable;)V",
                        &[JValue::Object(&g.as_obj())],
                    );
                    state.border_drawable = Some(g.clone());
                    g
                }
            };
            // Decide: animator-driven interpolation or snap.
            // Animator only if (a) author declared a transition on
            // any border axis AND (b) we have a previous state to
            // interpolate FROM (first apply always snaps).
            let all_prev_known = prev_w.iter().all(|p| p.is_some())
                && prev_c.iter().all(|p| p.is_some());
            if let (Some(transition), true) = (any_transition, all_prev_known) {
                cancel_animator(env, state.anim_border.take());
                let from_w = [
                    prev_w[0].unwrap(),
                    prev_w[1].unwrap(),
                    prev_w[2].unwrap(),
                    prev_w[3].unwrap(),
                ];
                let from_c = [
                    prev_c[0].unwrap(),
                    prev_c[1].unwrap(),
                    prev_c[2].unwrap(),
                    prev_c[3].unwrap(),
                ];
                state.anim_border = start_border_animator(
                    env,
                    &drawable_ref,
                    from_w,
                    widths_px,
                    from_c,
                    colors_argb,
                    transition,
                );
            } else {
                cancel_animator(env, state.anim_border.take());
                let _ = env.call_method(
                    drawable_ref.as_obj(),
                    "update",
                    "(IIIIIIII)V",
                    &[
                        JValue::Int(widths_px[0]),
                        JValue::Int(colors_argb[0]),
                        JValue::Int(widths_px[1]),
                        JValue::Int(colors_argb[1]),
                        JValue::Int(widths_px[2]),
                        JValue::Int(colors_argb[2]),
                        JValue::Int(widths_px[3]),
                        JValue::Int(colors_argb[3]),
                    ],
                );
            }
        } else if let Some(_) = state.border_drawable.take() {
            // All four sides cleared and we previously had borders —
            // detach the foreground so the view doesn't keep
            // referencing the drawable (and the GC can collect it).
            let _ = env.call_method(
                &view,
                "setForeground",
                "(Landroid/graphics/drawable/Drawable;)V",
                &[JValue::Object(&JObject::null())],
            );
        }
    }

    // --- Per-corner radii. setCornerRadii([f32; 8]) takes all four
    //     corners at once; for animation we run a single
    //     ValueAnimator that interpolates each corner's px value and
    //     re-invokes setCornerRadii every tick.
    let want_radii = [
        dp_to_px(env, &view, px_or(rules.border_top_left_radius.as_ref(), 0.0)) as f32,
        dp_to_px(env, &view, px_or(rules.border_top_right_radius.as_ref(), 0.0)) as f32,
        dp_to_px(env, &view, px_or(rules.border_bottom_right_radius.as_ref(), 0.0)) as f32,
        dp_to_px(env, &view, px_or(rules.border_bottom_left_radius.as_ref(), 0.0)) as f32,
    ];
    let radii_changed = (0..4).any(|i| state.last_radii[i] != Some(want_radii[i]));
    let radii_transitions = [
        rules.border_top_left_radius_transition,
        rules.border_top_right_radius_transition,
        rules.border_bottom_right_radius_transition,
        rules.border_bottom_left_radius_transition,
    ];
    if radii_changed {
        let prev: [Option<f32>; 4] = state.last_radii;
        for i in 0..4 {
            state.last_radii[i] = Some(want_radii[i]);
        }
        // Pick a transition: if any corner has one, use it. We
        // animate all corners together since setCornerRadii is the
        // single setter.
        let trans = radii_transitions.iter().copied().find_map(|t| t);
        let all_prev_set = prev.iter().all(|p| p.is_some());
        if all_prev_set
            && trans.is_some()
            && (0..4).any(|i| prev[i].unwrap() != want_radii[i])
        {
            let from = [
                prev[0].unwrap(),
                prev[1].unwrap(),
                prev[2].unwrap(),
                prev[3].unwrap(),
            ];
            cancel_animator(env, state.anim_radii[0].take());
            state.anim_radii[0] =
                start_radii_animator(env, &drawable, from, want_radii, trans.unwrap());
        } else {
            set_corner_radii(env, &drawable_obj, want_radii);
        }
    }
    // --- Outline clipping so children honor the rounded background.
    //
    // Android renders the GradientDrawable's rounded corners
    // correctly on the background, but by default the View does NOT
    // clip its children to that outline — a Card with rounded
    // corners would still paint its child views as square rectangles
    // overhanging the rounded edges. The fix is `setClipToOutline(true)`
    // combined with the default `ViewOutlineProvider` (already
    // installed by `setBackground(GradientDrawable)`), which derives
    // a rounded-rect outline from the background drawable.
    //
    // Only enable when there's a corner radius — `clipToOutline` on
    // a 0-radius view is harmless but the extra outline-pass cost
    // isn't free, and we'd lose the ability to render shadows /
    // overflow content on unrounded views.
    let has_any_radius = want_radii.iter().any(|r| *r > 0.0);
    if has_any_radius {
        let _ = env.call_method(
            &view,
            "setClipToOutline",
            "(Z)V",
            &[JValue::Bool(1)],
        );
    }
    // Mirror corner radii onto the per-side border drawable (if
    // installed) so its `drawRoundRect`/edge-clipping path respects
    // the same curves the GradientDrawable background paints with.
    // Without this, border foregrounds would paint square corners
    // overlapping the rounded background.
    if let Some(border_drawable) = state.border_drawable.clone() {
        let _ = env.call_method(
            border_drawable.as_obj(),
            "setCornerRadii",
            "(FFFF)V",
            &[
                JValue::Float(want_radii[0]),
                JValue::Float(want_radii[1]),
                JValue::Float(want_radii[2]),
                JValue::Float(want_radii[3]),
            ],
        );
    }
}

fn set_corner_radii(env: &mut JNIEnv, drawable: &JObject, r: [f32; 4]) {
    // GradientDrawable.setCornerRadii expects [tl, tl, tr, tr, br,
    // br, bl, bl] in px (X-radius and Y-radius per corner — we pass
    // the same value for both).
    let radii = [r[0], r[0], r[1], r[1], r[2], r[2], r[3], r[3]];
    let arr = env.new_float_array(radii.len() as i32).unwrap();
    env.set_float_array_region(&arr, 0, &radii).unwrap();
    let _ = env.call_method(
        drawable,
        "setCornerRadii",
        "([F)V",
        &[JValue::Object(&JObject::from(arr))],
    );
}

/// Mirror a [`runtime_core::Gradient`] onto an existing
/// `GradientDrawable`. Each call rebuilds the colors + locations
/// arrays — the GradientDrawable's color path is opaque (no
/// per-stop setters) so we hand it the full array each time. The
/// drawable is reused across applies (kept on `NodeAnim::drawable`),
/// so this is one set of JNI calls per style application, not per
/// frame.
fn apply_gradient_to_drawable(
    env: &mut JNIEnv,
    view: &JObject,
    drawable: &JObject,
    g: &runtime_core::Gradient,
    state: &mut NodeAnim,
) {
    // Sort stops by ascending offset. GradientDrawable's setColors
    // takes the colors array in stop order with positions inferred
    // (uniform spacing); we approximate non-uniform spacing by
    // densifying the array — see below.
    let mut stops = g.stops.clone();
    stops.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap_or(std::cmp::Ordering::Equal));

    // Resolve each stop to sRGB `[r, g, b, a]` and stash on the node
    // state. Per-frame `set_animated_color(GradientStopColor(idx))`
    // mutates `state.gradient_stops[idx]` and re-emits the ARGB int
    // array without re-resolving the stylesheet.
    let stops_srgb: Vec<[f32; 4]> = stops.iter().map(|s| color_to_srgb(&s.color)).collect();
    let offsets: Vec<f32> = stops.iter().map(|s| s.offset.clamp(0.0, 1.0)).collect();
    state.gradient_stops = stops_srgb;
    state.gradient_offsets = offsets;

    if state.gradient_stops.is_empty() {
        return;
    }
    push_gradient_colors_to_drawable(
        env,
        drawable,
        &state.gradient_stops,
        &state.gradient_offsets,
    );

    match g.kind {
        runtime_core::GradientKind::Linear { angle_deg } => {
            // LINEAR_GRADIENT = 0
            let _ = env.call_method(
                drawable,
                "setGradientType",
                "(I)V",
                &[JValue::Int(0)],
            );
            // GradientDrawable.Orientation is an enum: snap the angle
            // to the nearest of the 8 supported directions and look
            // up its constant inline. Keeping the lookup local avoids
            // a function-return-lifetime tangle with `JObject<'local>`.
            let name = nearest_orientation_name(angle_deg);
            if let Ok(class) =
                env.find_class("android/graphics/drawable/GradientDrawable$Orientation")
            {
                if let Ok(field_id) = env.get_static_field_id(
                    &class,
                    name,
                    "Landroid/graphics/drawable/GradientDrawable$Orientation;",
                ) {
                    if let Ok(obj) = env
                        .get_static_field_unchecked(
                            &class,
                            field_id,
                            jni::signature::JavaType::Object(
                                "Landroid/graphics/drawable/GradientDrawable$Orientation;".into(),
                            ),
                        )
                        .and_then(|v| v.l())
                    {
                        let _ = env.call_method(
                            drawable,
                            "setOrientation",
                            "(Landroid/graphics/drawable/GradientDrawable$Orientation;)V",
                            &[JValue::Object(&obj)],
                        );
                    }
                }
            }
        }
        runtime_core::GradientKind::Radial { center: _, radius, extent } => {
            // RADIAL_GRADIENT = 1
            let _ = env.call_method(
                drawable,
                "setGradientType",
                "(I)V",
                &[JValue::Int(1)],
            );
            // Stash extent + radius factor so the layout pass can
            // recompute the real px radius once the view has been
            // laid out. `getMeasuredWidth/Height` here are usually
            // 0 (the measure pass hasn't run yet), so any radius
            // we compute now is a placeholder — `sync_radial_gradient_radius`
            // overwrites it once Taffy has produced a frame.
            state.gradient_radial_extent = Some(extent);
            state.gradient_radial_radius_factor = Some(radius);
            // Initial placeholder: try the view's currently-measured
            // size, fall back to 100dp. The layout pass below will
            // call `sync_radial_gradient_radius` with the real frame
            // dimensions and overwrite this value.
            let w_px: i32 = env
                .call_method(view, "getMeasuredWidth", "()I", &[])
                .and_then(|v| v.i())
                .unwrap_or(0);
            let h_px: i32 = env
                .call_method(view, "getMeasuredHeight", "()I", &[])
                .and_then(|v| v.i())
                .unwrap_or(0);
            let half_w = (w_px as f32 * 0.5).max(0.0);
            let half_h = (h_px as f32 * 0.5).max(0.0);
            let reference_px = match extent {
                runtime_core::RadialExtent::ClosestSide => half_w.min(half_h),
                runtime_core::RadialExtent::FarthestCorner => (half_w * half_w + half_h * half_h).sqrt(),
            };
            let radius_px = if reference_px > 0.0 {
                reference_px * radius
            } else {
                dp_to_px(env, view, 100.0) as f32
            };
            let _ = env.call_method(
                drawable,
                "setGradientRadius",
                "(F)V",
                &[JValue::Float(radius_px)],
            );
            // Center: `setGradientCenter(float x, float y)` takes
            // normalized 0..1 coords. Matches the framework's
            // convention exactly.
            // (Note: GradientDrawable.setGradientCenter exists in
            // API level 1 but is poorly documented — uses gradient
            // center for both linear and radial. We set it
            // unconditionally so toggling kinds works.)
            let _ = env.call_method(
                drawable,
                "setGradientCenter",
                "(FF)V",
                &[
                    JValue::Float(g.kind_center().0),
                    JValue::Float(g.kind_center().1),
                ],
            );
        }
    }
}

/// Map an arbitrary angle in degrees to the name of the nearest
/// `GradientDrawable.Orientation` enum constant. Caller looks up the
/// constant inline via reflection — keeping the JNI returned-object
/// lifetime local rather than tangled in a return type.
fn nearest_orientation_name(angle_deg: f32) -> &'static str {
    // The 8 supported orientations and their angles (CSS-style:
    // 0° = bottom→top, clockwise).
    const ORIENTATIONS: &[(&str, f32)] = &[
        ("BOTTOM_TOP", 0.0),
        ("BL_TR", 45.0),
        ("LEFT_RIGHT", 90.0),
        ("TL_BR", 135.0),
        ("TOP_BOTTOM", 180.0),
        ("TR_BL", 225.0),
        ("RIGHT_LEFT", 270.0),
        ("BR_TL", 315.0),
    ];
    let mut a = angle_deg % 360.0;
    if a < 0.0 {
        a += 360.0;
    }
    ORIENTATIONS
        .iter()
        .min_by(|(_, x), (_, y)| {
            let dx = cyclic_distance(a, *x);
            let dy = cyclic_distance(a, *y);
            dx.partial_cmp(&dy).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(name, _)| *name)
        .unwrap_or("TOP_BOTTOM")
}

fn cyclic_distance(a: f32, b: f32) -> f32 {
    let d = (a - b).abs() % 360.0;
    if d > 180.0 { 360.0 - d } else { d }
}

/// Local helper on `Gradient` to extract the center coordinate from a
/// `Radial` variant. Returns `(0.5, 0.5)` for linear (the field is
/// unused). Lives here rather than on `runtime_core::Gradient`
/// because the runtime-core type doesn't need to know about Android
/// `GradientDrawable.setGradientCenter`'s shape.
trait GradientKindCenter {
    fn kind_center(&self) -> (f32, f32);
}
impl GradientKindCenter for runtime_core::Gradient {
    fn kind_center(&self) -> (f32, f32) {
        match self.kind {
            runtime_core::GradientKind::Radial { center, .. } => center,
            runtime_core::GradientKind::Linear { .. } => (0.5, 0.5),
        }
    }
}

/// Resolve a `runtime_core::Color` to sRGB `[r, g, b, a]` floats.
/// Used to seed `state.gradient_stops` so the per-frame
/// `GradientStopColor` writer can mutate one entry and re-emit
/// without re-parsing the stylesheet. Falls back to fully
/// transparent on unknown input — matches the legacy behavior of
/// `parse_color(...).unwrap_or(0)` (Android's `0` int is fully
/// transparent).
pub(crate) fn color_to_srgb(c: &runtime_core::Color) -> [f32; 4] {
    runtime_core::color::parse_or(&c.0, runtime_core::color::Rgba::TRANSPARENT).to_srgb_f32()
}

/// Cached `Build.VERSION.SDK_INT` — read once from the JVM on the
/// first gradient apply and reused thereafter. Wrapped in
/// `AtomicI32` rather than `OnceLock<i32>` so it can be read
/// without holding a lock on the hot per-frame writer path.
/// `-1` is the sentinel for "not yet probed".
static ANDROID_SDK_INT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// Read `android.os.Build.VERSION.SDK_INT` once and cache it. Used
/// to gate the API-29+ `GradientDrawable.setColors(int[], float[])`
/// path that honors non-uniform stop offsets.
fn android_sdk_int(env: &mut JNIEnv) -> i32 {
    let cached = ANDROID_SDK_INT.load(std::sync::atomic::Ordering::Relaxed);
    if cached >= 0 {
        return cached;
    }
    let v = env
        .find_class("android/os/Build$VERSION")
        .and_then(|cls| env.get_static_field(&cls, "SDK_INT", "I"))
        .and_then(|jv| jv.i())
        .unwrap_or(0);
    ANDROID_SDK_INT.store(v, std::sync::atomic::Ordering::Relaxed);
    v
}

/// Push a gradient's color stops to an existing `GradientDrawable`,
/// honoring per-stop offsets on API 29+ via the
/// `setColors(int[], float[])` overload and falling back to the
/// legacy uniform-spacing `setColors(int[])` on older devices.
///
/// `colors` and `offsets` are parallel slices of the same length
/// (already sorted by offset). No-op on empty input.
fn push_gradient_colors_to_drawable(
    env: &mut JNIEnv,
    drawable: &JObject,
    colors: &[[f32; 4]],
    offsets: &[f32],
) {
    if colors.is_empty() {
        return;
    }
    let packed: Vec<i32> = colors.iter().map(|c| srgb_to_argb_i32(*c)).collect();
    let color_arr = match env.new_int_array(packed.len() as i32) {
        Ok(a) => a,
        Err(_) => return,
    };
    if env.set_int_array_region(&color_arr, 0, &packed).is_err() {
        return;
    }

    // API 29 (Android 10) added `setColors(int[], float[])` — the
    // first overload that honors arbitrary stop positions. Older
    // devices fall back to the uniform-distribution overload.
    // Min SDK in `crates/build/ios/src/source.rs` is 21 (Lollipop),
    // so the fallback is load-bearing.
    if android_sdk_int(env) >= 29 && offsets.len() == colors.len() {
        let off_arr = match env.new_float_array(offsets.len() as i32) {
            Ok(a) => a,
            Err(_) => return,
        };
        if env.set_float_array_region(&off_arr, 0, offsets).is_err() {
            return;
        }
        let _ = env.call_method(
            drawable,
            "setColors",
            "([I[F)V",
            &[
                JValue::Object(&JObject::from(color_arr)),
                JValue::Object(&JObject::from(off_arr)),
            ],
        );
    } else {
        let _ = env.call_method(
            drawable,
            "setColors",
            "([I)V",
            &[JValue::Object(&JObject::from(color_arr))],
        );
    }
}

/// sRGB float `[r, g, b, a]` → packed ARGB `i32` (Android's color
/// representation). Symmetric with `color_to_srgb`.
pub(crate) fn srgb_to_argb_i32(c: [f32; 4]) -> i32 {
    runtime_core::color::Rgba::from_srgb_f32(c).to_argb_u32() as i32
}

/// Per-frame writer for `AnimProp::GradientStopColor(idx)`. Mutates
/// `state.gradient_stops[idx]` and re-emits the ARGB int array onto
/// the node's `GradientDrawable`. No-op if the node has no
/// drawable / no stops / idx out of range.
pub(crate) fn set_animated_gradient_stop(
    env: &mut JNIEnv,
    state: &mut NodeAnim,
    idx: usize,
    value: [f32; 4],
) {
    if idx >= state.gradient_stops.len() {
        return;
    }
    let Some(drawable) = state.drawable.as_ref() else {
        return;
    };
    state.gradient_stops[idx] = value;
    // `drawable.as_obj()` borrows `state.drawable`; clone the stops
    // + offsets out so we don't double-borrow `state` inside
    // `push_gradient_colors_to_drawable`.
    let colors_copy = state.gradient_stops.clone();
    let offsets_copy = state.gradient_offsets.clone();
    push_gradient_colors_to_drawable(env, drawable.as_obj(), &colors_copy, &offsets_copy);
}
