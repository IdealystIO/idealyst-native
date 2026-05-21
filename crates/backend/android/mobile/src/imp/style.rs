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
use super::NodeAnim;
use framework_core::StyleRules;
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;

pub(crate) fn apply_rules(
    env: &mut JNIEnv,
    node: &GlobalRef,
    state: &mut NodeAnim,
    rules: &StyleRules,
) {
    let view = node.as_obj();

    // --- Padding (per-side; framework stores all four independently).
    //     Each side may animate independently.
    let want_padding = [
        dp_to_px(env, &view, px_or(rules.padding_left.as_ref(), 0.0)),
        dp_to_px(env, &view, px_or(rules.padding_top.as_ref(), 0.0)),
        dp_to_px(env, &view, px_or(rules.padding_right.as_ref(), 0.0)),
        dp_to_px(env, &view, px_or(rules.padding_bottom.as_ref(), 0.0)),
    ];
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
            framework_core::Length::Px(v) => dp_to_px(env, &view, v),
            framework_core::Length::Percent(_) => -1,
            framework_core::Length::Auto => -2,
        });
        let h_lp = rules.height.as_ref().map(|tok| match tok.resolve() {
            framework_core::Length::Px(v) => dp_to_px(env, &view, v),
            framework_core::Length::Percent(_) => -1,
            framework_core::Length::Auto => -2,
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
    let textview_class = env.find_class("android/widget/TextView").unwrap();
    let is_textview = env.is_instance_of(&view, &textview_class).unwrap_or(false);

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
        if let Some(framework_core::Length::Px(size)) =
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
        // text-align → TextView.setGravity. Without this multi-line
        // wrapped text lays out left-justified inside the (often
        // wider-than-the-line) text-box, which makes a `text_align:
        // Center` author intent silently no-op on Android. Gravity
        // constants: LEFT=3, CENTER_HORIZONTAL=1, RIGHT=5. Default
        // gravity is LEFT|TOP — we keep TOP and only override the
        // horizontal axis.
        if let Some(align) = rules.text_align {
            let gravity = match align {
                framework_core::TextAlign::Left => 3 | 48,           // LEFT | TOP
                framework_core::TextAlign::Center => 1 | 48,         // CENTER_HORIZONTAL | TOP
                framework_core::TextAlign::Right => 5 | 48,          // RIGHT | TOP
                framework_core::TextAlign::Justify => 3 | 48,        // No JUSTIFY mode on TextView v1; fall back to LEFT
            };
            let _ = env.call_method(
                &view,
                "setGravity",
                "(I)V",
                &[JValue::Int(gravity)],
            );
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
    apply_transform(env, &view, rules);
}

fn apply_transform(env: &mut JNIEnv, view: &JObject, rules: &StyleRules) {
    use framework_core::{Length, Transform};
    // Default identity values. The loop overwrites them if matching
    // ops appear in `transform`.
    let mut tx_dp: f32 = 0.0;
    let mut ty_dp: f32 = 0.0;
    let mut sx: f32 = 1.0;
    let mut sy: f32 = 1.0;
    let mut rot_deg: f32 = 0.0;
    if let Some(ops) = rules.transform.as_ref() {
        for op in ops {
            match op {
                Transform::TranslateX(Length::Px(v)) => tx_dp = *v,
                Transform::TranslateY(Length::Px(v)) => ty_dp = *v,
                Transform::TranslateX(_) | Transform::TranslateY(_) => {
                    // Percent / Auto don't make sense for transform
                    // translation — silently treat as 0.
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
    // Convert dp → px for translation; scale and rotation are unitless.
    let tx_px = dp_to_px(env, view, tx_dp) as f32;
    let ty_px = dp_to_px(env, view, ty_dp) as f32;
    let _ = env.call_method(view, "setTranslationX", "(F)V", &[JValue::Float(tx_px)]);
    let _ = env.call_method(view, "setTranslationY", "(F)V", &[JValue::Float(ty_px)]);
    let _ = env.call_method(view, "setScaleX", "(F)V", &[JValue::Float(sx)]);
    let _ = env.call_method(view, "setScaleY", "(F)V", &[JValue::Float(sy)]);
    let _ = env.call_method(view, "setRotation", "(F)V", &[JValue::Float(rot_deg)]);
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

    // --- Stroke. GradientDrawable.setStroke(width, color) — single
    //     value. We collapse per-side to the first that's set (same
    //     as before). Width + color may each animate.
    // `border_*_width` is `Option<Tokenized<f32>>` after the
    // tokenization refactor. Resolve to the literal (native has no
    // token system) before passing to `dp_to_px`.
    let want_w = rules
        .border_top_width
        .as_ref()
        .or(rules.border_right_width.as_ref())
        .or(rules.border_bottom_width.as_ref())
        .or(rules.border_left_width.as_ref())
        .map(|tok| dp_to_px(env, &view, tok.resolve()));
    let want_c = rules
        .border_top_color
        .as_ref()
        .or(rules.border_right_color.as_ref())
        .or(rules.border_bottom_color.as_ref())
        .or(rules.border_left_color.as_ref())
        .and_then(|c| parse_color(&c.resolve().0));

    if let (Some(w), Some(c)) = (want_w, want_c) {
        let prev_w = state.last_stroke_w;
        let prev_c = state.last_stroke_color;
        let w_changed = prev_w != Some(w);
        let c_changed = prev_c != Some(c);
        state.last_stroke_w = Some(w);
        state.last_stroke_color = Some(c);
        if w_changed || c_changed {
            // setStroke is a single combined call. We don't have a
            // separate "stroke width" property to animate via
            // ObjectAnimator, so for animated stroke we use a
            // ValueAnimator that re-invokes setStroke on each tick.
            let w_t = rules
                .border_top_width_transition
                .or(rules.border_right_width_transition)
                .or(rules.border_bottom_width_transition)
                .or(rules.border_left_width_transition);
            let c_t = rules
                .border_top_color_transition
                .or(rules.border_right_color_transition)
                .or(rules.border_bottom_color_transition)
                .or(rules.border_left_color_transition);
            match (prev_w, prev_c, w_t.or(c_t)) {
                (Some(fw), Some(fc), Some(t)) if (fw != w || fc != c) => {
                    cancel_animator(env, state.anim_stroke_w.take());
                    state.anim_stroke_w =
                        start_stroke_animator(env, &drawable, fw, w, fc, c, t);
                }
                _ => {
                    let _ = env.call_method(
                        &drawable_obj,
                        "setStroke",
                        "(II)V",
                        &[JValue::Int(w), JValue::Int(c)],
                    );
                }
            }
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

/// Mirror a [`framework_core::Gradient`] onto an existing
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
    g: &framework_core::Gradient,
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
    state.gradient_stops = stops_srgb;

    // Build the packed ARGB int array. `GradientDrawable.setColors(int[])`
    // distributes the colors uniformly along the gradient; to honor
    // explicit `offset` values we'd need to upsample the array (insert
    // duplicates at the right indices). For v1 we pass the colors in
    // order and accept uniform spacing — matches typical sun/glow use
    // cases where stops are usually evenly spaced or so close that
    // the visual difference is minor. Non-uniform stops can be added
    // later by switching to `GradientDrawable.setColors(int[],
    // float[])` (API 29+).
    let colors: Vec<i32> = state
        .gradient_stops
        .iter()
        .map(|c| srgb_to_argb_i32(*c))
        .collect();
    if colors.is_empty() {
        return;
    }
    let arr = env.new_int_array(colors.len() as i32).unwrap();
    env.set_int_array_region(&arr, 0, &colors).unwrap();
    let _ = env.call_method(
        drawable,
        "setColors",
        "([I)V",
        &[JValue::Object(&JObject::from(arr))],
    );

    match g.kind {
        framework_core::GradientKind::Linear { angle_deg } => {
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
        framework_core::GradientKind::Radial { center: _, radius, extent } => {
            // RADIAL_GRADIENT = 1
            let _ = env.call_method(
                drawable,
                "setGradientType",
                "(I)V",
                &[JValue::Int(1)],
            );
            // The radius is a fraction of the smaller side, expressed
            // in dp via the view's bounds. `GradientDrawable.setGradientRadius`
            // takes pixels; convert dp→px using the view's density.
            // We can't read the view's bounds reliably here (the
            // layout pass may not have run yet); approximate with
            // the view's current measured width if available, else
            // fall back to a fixed 200dp — radius then gets re-applied
            // on the next style cycle once layout has produced real
            // dimensions.
            let w_px: i32 = env
                .call_method(view, "getMeasuredWidth", "()I", &[])
                .and_then(|v| v.i())
                .unwrap_or(0);
            let h_px: i32 = env
                .call_method(view, "getMeasuredHeight", "()I", &[])
                .and_then(|v| v.i())
                .unwrap_or(0);
            // Reference distance: half the closer side for
            // `ClosestSide`, the corner distance for `FarthestCorner`.
            // GradientDrawable's radial type is strictly circular —
            // a non-square `FarthestCorner` box produces a circle
            // that reaches the corners (asymmetric edge falloff
            // compared to the elliptical iOS/web shape, but the
            // corners reach the last stop on all platforms which is
            // the goal for vignette use cases).
            let half_w = (w_px as f32 * 0.5).max(0.0);
            let half_h = (h_px as f32 * 0.5).max(0.0);
            let reference_px = match extent {
                framework_core::RadialExtent::ClosestSide => half_w.min(half_h),
                framework_core::RadialExtent::FarthestCorner => (half_w * half_w + half_h * half_h).sqrt(),
            };
            let radius_px = if reference_px > 0.0 {
                reference_px * radius
            } else {
                // Fallback while waiting for layout: convert a
                // sensible default 100dp radius to px via the
                // existing dp→px helper.
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
/// unused). Lives here rather than on `framework_core::Gradient`
/// because the framework-core type doesn't need to know about Android
/// `GradientDrawable.setGradientCenter`'s shape.
trait GradientKindCenter {
    fn kind_center(&self) -> (f32, f32);
}
impl GradientKindCenter for framework_core::Gradient {
    fn kind_center(&self) -> (f32, f32) {
        match self.kind {
            framework_core::GradientKind::Radial { center, .. } => center,
            framework_core::GradientKind::Linear { .. } => (0.5, 0.5),
        }
    }
}

/// Resolve a `framework_core::Color` to sRGB `[r, g, b, a]` floats.
/// Used to seed `state.gradient_stops` so the per-frame
/// `GradientStopColor` writer can mutate one entry and re-emit
/// without re-parsing the stylesheet.
pub(crate) fn color_to_srgb(c: &framework_core::Color) -> [f32; 4] {
    let argb = parse_color(&c.0).unwrap_or(0);
    let a = ((argb >> 24) & 0xff) as f32 / 255.0;
    let r = ((argb >> 16) & 0xff) as f32 / 255.0;
    let g = ((argb >> 8) & 0xff) as f32 / 255.0;
    let b = (argb & 0xff) as f32 / 255.0;
    [r, g, b, a]
}

/// sRGB float `[r, g, b, a]` → packed ARGB `i32` (Android's color
/// representation). Symmetric with `color_to_srgb`.
pub(crate) fn srgb_to_argb_i32(c: [f32; 4]) -> i32 {
    let a = (c[3].clamp(0.0, 1.0) * 255.0).round() as u32;
    let r = (c[0].clamp(0.0, 1.0) * 255.0).round() as u32;
    let g = (c[1].clamp(0.0, 1.0) * 255.0).round() as u32;
    let b = (c[2].clamp(0.0, 1.0) * 255.0).round() as u32;
    (((a & 0xff) << 24) | ((r & 0xff) << 16) | ((g & 0xff) << 8) | (b & 0xff)) as i32
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
    let colors: Vec<i32> = state
        .gradient_stops
        .iter()
        .map(|c| srgb_to_argb_i32(*c))
        .collect();
    let arr = match env.new_int_array(colors.len() as i32) {
        Ok(a) => a,
        Err(_) => return,
    };
    if env.set_int_array_region(&arr, 0, &colors).is_err() {
        return;
    }
    let _ = env.call_method(
        drawable.as_obj(),
        "setColors",
        "([I)V",
        &[JValue::Object(&JObject::from(arr))],
    );
}
