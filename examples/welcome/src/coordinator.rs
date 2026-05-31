//! Animation orchestration for the welcome scene, exposed as a
//! hook-style call.
//!
//! [`use_welcome`] creates every primitive ref the tree binds to,
//! wires up every `AnimatedValue` against those refs, schedules the
//! act timeline, and starts the unified raf-driven pulse — then
//! returns the refs for [`crate::app`] to use in the `ui!` tree.
//! Animation plumbing doesn't leak into the tree-building code.
//!
//! All scheduling here is scope-anchored: the framework's
//! [`runtime_core::after_ms_scoped`] / [`runtime_core::raf_loop_scoped`]
//! and the auto-cleanup baked into [`runtime_core::timeline!`] mean
//! the welcome's timers die naturally with the mounted `Owner` —
//! no `std::mem::forget` boilerplate, no `on_cleanup(move || drop(...))`.

use std::time::Duration;

use runtime_core::animation::{AnimProp, SpringTo, TweenTo};
use runtime_core::{
    effect, node_ref, raf_loop_scoped, timeline, Ref, TextHandle, ViewHandle,
};

use crate::color::{lerp_color, srgb_tuple, srgba_tuple};
use crate::components::page::{COLOR_DARK_BG, COLOR_LIGHT_BG};
use crate::components::planet::{
    PLANETS, PLANET_FADE_IN_MS, PLANET_SCALE_BACK, PLANET_SCALE_FRONT,
};
use crate::components::subtitle::SUBTITLE_ENTER_Y;
use crate::components::sun_glare::{
    COLOR_SUN_CORE, GLARE_INITIAL_SCALE, SUN_CORE_BRIGHT, SUN_CORE_DIM, SUN_CORONA_BRIGHT,
    SUN_CORONA_DIM, SUN_PULSE_AMPLITUDE, SUN_PULSE_PERIOD_MS,
};
use crate::components::vignette::{VIGNETTE_CORNER_BRIGHT, VIGNETTE_CORNER_DIM};
use crate::components::welcome_phrase::{
    COLOR_HEADLINE_DARK, COLOR_HEADLINE_LIGHT, PHRASE_ENTER_SCALE, PHRASE_ENTER_Y,
    WELCOME_SHUFFLE_Y,
};
use crate::constants::{
    ACT_1_HOLD_MS, CONTENT_OFFSET_AFTER_GLARE_MS, DARK_FADE_MS, GLARE_LAG_AFTER_DARK_MS,
    INTRO_PAUSE_MS, PHRASE_ENTER_BUDGET_MS,
};

/// Every primitive ref the welcome tree exposes. `welcome_text` is
/// split out from `welcome` because UILabel's `textColor` doesn't
/// cascade from its wrapper's transform — the color AV writes
/// through a `TextHandle` of its own. `Copy` so components can pass
/// it through props by value without lifetime threading.
#[derive(Clone, Copy, Default)]
pub struct WelcomeRefs {
    pub welcome: Ref<ViewHandle>,
    pub welcome_text: Ref<TextHandle>,
    pub vignette: Ref<ViewHandle>,
    pub vignette_top: Ref<ViewHandle>,
    pub vignette_bottom: Ref<ViewHandle>,
    pub vignette_left: Ref<ViewHandle>,
    pub vignette_right: Ref<ViewHandle>,
    pub glare: Ref<ViewHandle>,
    pub subtitle: Ref<ViewHandle>,
    pub page: Ref<ViewHandle>,
    pub planets: [Ref<ViewHandle>; 3],
}

/// Mount the welcome animation system: allocate refs, bind every
/// `AnimatedValue` to its target, schedule the act timeline, start
/// the raf-driven pulse, and return the refs.
///
/// Call once from inside the mounted reactive scope (i.e. from
/// `app()` running under `runtime_core::mount(...)`). Every
/// scheduling primitive used here is scope-anchored, so the
/// returned refs and all their animations die with the surrounding
/// `Owner`.
pub fn use_welcome() -> WelcomeRefs {
    let refs = WelcomeRefs {
        welcome: node_ref!(ViewHandle),
        welcome_text: node_ref!(TextHandle),
        vignette: node_ref!(ViewHandle),
        vignette_top: node_ref!(ViewHandle),
        vignette_bottom: node_ref!(ViewHandle),
        vignette_left: node_ref!(ViewHandle),
        vignette_right: node_ref!(ViewHandle),
        glare: node_ref!(ViewHandle),
        subtitle: node_ref!(ViewHandle),
        page: node_ref!(ViewHandle),
        planets: [
            node_ref!(ViewHandle),
            node_ref!(ViewHandle),
            node_ref!(ViewHandle),
        ],
    };

    // ---- Animated values, one per (element, property) pair ----
    //
    // Declared via `runtime_core::session::animated(key, initial)`
    // so each AV's current value survives `SessionMsg::Rerender`
    // (hot-patch landing). Combined with the session-relative
    // [`timeline!`] macro (which fires already-elapsed acts
    // immediately), this is what stops the welcome scene from
    // visibly re-running Acts 1/2/3 on every save: the AVs are
    // already at their post-act values, the re-fired timeline
    // tweens compute `current == target`, and the scene picks up
    // continuously from where the previous run left off.
    //
    // Keys must be unique within this session — collisions are
    // last-write-wins. The `welcome_` prefix scopes them so a
    // larger app composing welcome alongside other components
    // doesn't accidentally collide.
    use runtime_core::session::animated as keyed;
    let welcome_opacity = keyed("welcome_opacity", 0.0_f32);
    let welcome_scale = keyed("welcome_scale", PHRASE_ENTER_SCALE);
    let welcome_y = keyed("welcome_y", PHRASE_ENTER_Y);
    let welcome_color = keyed("welcome_color", srgb_tuple(COLOR_HEADLINE_DARK));

    let page_background = keyed("welcome_page_background", srgb_tuple(COLOR_LIGHT_BG));

    let glare_opacity = keyed("welcome_glare_opacity", 0.0_f32);
    let glare_scale = keyed("welcome_glare_scale", GLARE_INITIAL_SCALE);
    let sun_core_color = keyed("welcome_sun_core_color", srgb_tuple(COLOR_SUN_CORE));
    let sun_corona_color = keyed(
        "welcome_sun_corona_color",
        srgba_tuple(255.0, 210.0, 110.0, 0.85),
    );

    let vignette_opacity = keyed("welcome_vignette_opacity", 0.0_f32);
    let vignette_corner_color = keyed(
        "welcome_vignette_corner_color",
        srgba_tuple(255.0, 168.0, 60.0, 0.0),
    );

    let subtitle_opacity = keyed("welcome_subtitle_opacity", 0.0_f32);
    let subtitle_y = keyed("welcome_subtitle_y", SUBTITLE_ENTER_Y);

    // Planet AVs — one keyed entry per (planet_index, property).
    let planet_x = [
        keyed("welcome_planet_x_0", 0.0_f32),
        keyed("welcome_planet_x_1", 0.0_f32),
        keyed("welcome_planet_x_2", 0.0_f32),
    ];
    let planet_y = [
        keyed("welcome_planet_y_0", 0.0_f32),
        keyed("welcome_planet_y_1", 0.0_f32),
        keyed("welcome_planet_y_2", 0.0_f32),
    ];
    let planet_alpha = [
        keyed("welcome_planet_alpha_0", 0.0_f32),
        keyed("welcome_planet_alpha_1", 0.0_f32),
        keyed("welcome_planet_alpha_2", 0.0_f32),
    ];
    let planet_scale = [
        keyed("welcome_planet_scale_0", 1.0_f32),
        keyed("welcome_planet_scale_1", 1.0_f32),
        keyed("welcome_planet_scale_2", 1.0_f32),
    ];
    let planet_z = [
        keyed("welcome_planet_z_0", 0.0_f32),
        keyed("welcome_planet_z_1", 0.0_f32),
        keyed("welcome_planet_z_2", 0.0_f32),
    ];

    // ---- Bind each AV to its target ref + property ----
    welcome_opacity.bind(refs.welcome, AnimProp::Opacity);
    welcome_scale.bind(refs.welcome, AnimProp::Scale);
    welcome_y.bind(refs.welcome, AnimProp::TranslateY);
    welcome_color.bind_text_color(refs.welcome_text, AnimProp::ForegroundColor);
    page_background.bind_color(refs.page, AnimProp::BackgroundColor);
    vignette_opacity.bind(refs.vignette, AnimProp::Opacity);
    glare_opacity.bind(refs.glare, AnimProp::Opacity);
    glare_scale.bind(refs.glare, AnimProp::Scale);
    sun_core_color.bind_gradient_stop(refs.glare, 0);
    sun_corona_color.bind_gradient_stop(refs.glare, 1);
    vignette_corner_color.bind_gradient_stop(refs.vignette_top, 1);
    vignette_corner_color.bind_gradient_stop(refs.vignette_bottom, 1);
    vignette_corner_color.bind_gradient_stop(refs.vignette_left, 1);
    vignette_corner_color.bind_gradient_stop(refs.vignette_right, 1);
    subtitle_opacity.bind(refs.subtitle, AnimProp::Opacity);
    subtitle_y.bind(refs.subtitle, AnimProp::TranslateY);

    for i in 0..3 {
        planet_x[i].bind(refs.planets[i], AnimProp::TranslateX);
        planet_y[i].bind(refs.planets[i], AnimProp::TranslateY);
        planet_alpha[i].bind(refs.planets[i], AnimProp::Opacity);
        planet_scale[i].bind(refs.planets[i], AnimProp::Scale);
        planet_z[i].bind(refs.planets[i], AnimProp::ZIndex);
    }

    // ---- Act schedule ----
    let act_1_start = INTRO_PAUSE_MS;
    let act_2_start = act_1_start + PHRASE_ENTER_BUDGET_MS + ACT_1_HOLD_MS;
    let glare_start = act_2_start + GLARE_LAG_AFTER_DARK_MS;
    let act_3_start = glare_start + CONTENT_OFFSET_AFTER_GLARE_MS;

    let page_ref = refs.page;

    effect!({
        timeline! {
            act_1_start => {
                welcome_opacity: TweenTo::new(1.0, Duration::from_millis(700)).ease_out(),
                welcome_scale: SpringTo::new(1.0).stiffness(170.0).damping(22.0),
                welcome_y: SpringTo::new(0.0).stiffness(170.0).damping(22.0),
            },
            act_2_start => {
                welcome_color: TweenTo::new(
                    srgb_tuple(COLOR_HEADLINE_LIGHT),
                    Duration::from_millis(DARK_FADE_MS),
                ).ease_in_out(),
                page_background: TweenTo::new(
                    srgb_tuple(COLOR_DARK_BG),
                    Duration::from_millis(DARK_FADE_MS),
                ).ease_in_out(),
                vignette_opacity: TweenTo::new(1.0, Duration::from_millis(DARK_FADE_MS)).ease_in_out(),
            },
            // Glare lags the dark wash by a beat so it reads as
            // arriving INTO the dark scene, not painted with it.
            glare_start => {
                glare_opacity: TweenTo::new(1.0, Duration::from_millis(1700)).ease_out(),
                glare_scale: SpringTo::new(1.0).stiffness(55.0).damping(18.0),
            },
            act_3_start => {
                welcome_y: SpringTo::new(WELCOME_SHUFFLE_Y).stiffness(110.0).damping(20.0),
                subtitle_opacity: TweenTo::new(1.0, Duration::from_millis(800)).ease_out(),
                subtitle_y: SpringTo::new(0.0).stiffness(140.0).damping(20.0),
            },
        };

        // Unified raf-driven pulse: one sine wave drives the sun's
        // core/corona/scale, the vignette's corner color, AND every
        // planet's position/scale/alpha/z. Sharing one phase keeps
        // the scene's "breath" coherent.
        let core_av = sun_core_color.clone();
        let corona_av = sun_corona_color.clone();
        let vignette_av = vignette_corner_color.clone();
        let scale_av = glare_scale.clone();
        let planet_x_clones = planet_x.clone();
        let planet_y_clones = planet_y.clone();
        let planet_alpha_clones = planet_alpha.clone();
        let planet_scale_clones = planet_scale.clone();
        let planet_z_clones = planet_z.clone();

        // `session::after_ms` schedules relative to the session epoch,
        // not "now". On the first mount it behaves like
        // `after_ms_scoped(glare_start, ...)`; after a hot-patch
        // rerender, if we're already past `glare_start` ms into the
        // session, this fires immediately — so the raf-driven sun
        // pulse / planet orbit resumes within one frame of the
        // rerender instead of waiting another 3.2 seconds for the
        // act timeline to replay.
        runtime_core::session::after_ms(glare_start as u64, move || {
            let period_ms = SUN_PULSE_PERIOD_MS;
            // Wait half a period before the scale pulse takes over
            // the entrance spring — joins on `sin(0) = 0` so the
            // handoff lands at scale 1.0 with no visible jump.
            let scale_gate_ms = period_ms / 2.0;
            // `session::epoch_micros` returns the *first-call* time
            // for this session thread and survives hot-patch rerenders
            // (see [runtime_core::session]). With plain
            // `time::now_micros()`, every rerender recaptured a fresh
            // epoch and the sun pulse / planet orbits visibly snapped
            // back to their initial phase on every save. Anchoring to
            // the session epoch keeps the animation phase continuous
            // across edits.
            let epoch = runtime_core::session::epoch_micros();
            // Track total paused duration so the clock effectively
            // halts while the embedding host reports
            // `is_frame_active() == false` (e.g. the wgpu host's
            // surface is hidden behind a navigator's persistent
            // screen). Without this, the welcome animation would
            // keep advancing in real-time while invisible and snap
            // forward when the surface re-appears. With it, the
            // planet positions stay frozen and resume right where
            // they left off.
            let mut pause_start_us: Option<u64> = None;
            let mut paused_duration_us: u64 = 0;
            raf_loop_scoped(move || {
                let now = runtime_core::time::now_micros();
                if !runtime_core::is_frame_active() {
                    if pause_start_us.is_none() {
                        pause_start_us = Some(now);
                    }
                    return;
                }
                if let Some(start) = pause_start_us.take() {
                    paused_duration_us =
                        paused_duration_us.saturating_add(now.saturating_sub(start));
                }
                let elapsed_us = now
                    .saturating_sub(epoch)
                    .saturating_sub(paused_duration_us);
                let elapsed_ms = (elapsed_us as f64) / 1000.0;
                let phase = (elapsed_ms / period_ms) * std::f64::consts::TAU;
                let sin = phase.sin();
                let t = ((sin + 1.0) * 0.5) as f32;
                core_av.set(lerp_color(SUN_CORE_DIM, SUN_CORE_BRIGHT, t));
                corona_av.set(lerp_color(SUN_CORONA_DIM, SUN_CORONA_BRIGHT, t));
                vignette_av.set(lerp_color(VIGNETTE_CORNER_DIM, VIGNETTE_CORNER_BRIGHT, t));
                if elapsed_ms >= scale_gate_ms {
                    scale_av.set(1.0_f32 + SUN_PULSE_AMPLITUDE * sin as f32);
                }

                // Planets orbit a 45° diagonal ellipse around
                // screen centre. Depth = sin θ → scale + binary z.
                let viewport = page_ref.with(|h| h.frame()).flatten();
                let (vw, vh) = viewport
                    .map(|r| (r.width, r.height))
                    .filter(|(w, h)| *w > 0.0 && *h > 0.0)
                    .unwrap_or((393.0, 800.0));
                let fade_in = ((elapsed_ms / PLANET_FADE_IN_MS).min(1.0)) as f32;
                let cx = vw * 0.50;
                let cy = vh * 0.50;
                let major_x: f32 = -std::f32::consts::FRAC_1_SQRT_2;
                let major_y: f32 = std::f32::consts::FRAC_1_SQRT_2;
                let minor_x: f32 = major_y;
                let minor_y: f32 = -major_x;
                for (i, cfg) in PLANETS.iter().enumerate() {
                    let theta = (elapsed_ms / cfg.period_ms) * std::f64::consts::TAU
                        + cfg.phase_offset as f64;
                    let cos_t = theta.cos() as f32;
                    let sin_t = theta.sin() as f32;
                    let r_major = vh * cfg.ry_frac;
                    let r_minor = vh * cfg.rx_frac;
                    let offset_x = r_major * cos_t * major_x + r_minor * sin_t * minor_x;
                    let offset_y = r_major * cos_t * major_y + r_minor * sin_t * minor_y;
                    let center_x = cx + offset_x;
                    let center_y = cy + offset_y;
                    // Planet's natural top-left sits at (vw - size, 0)
                    // because the sheet pins it to `top:0, right:0`.
                    let tx = center_x - vw + cfg.size_dp * 0.5;
                    let ty = center_y - cfg.size_dp * 0.5;
                    planet_x_clones[i].set(tx);
                    planet_y_clones[i].set(ty);
                    let depth_t = (sin_t + 1.0) * 0.5;
                    let scale = PLANET_SCALE_BACK
                        + (PLANET_SCALE_FRONT - PLANET_SCALE_BACK) * depth_t;
                    planet_scale_clones[i].set(scale);
                    planet_alpha_clones[i].set(fade_in);
                    // Binary z flips at `sin θ = 0`:
                    //   front half (sin θ > 0) → z = 1, above the
                    //     content layer's implicit z = 0.
                    //   back half  (sin θ ≤ 0) → z = 0, tied with
                    //     content; sibling document order resolves
                    //     the tie (dark/vignette/glare before
                    //     planets paint below; content_layer after
                    //     paints above) — exactly the "planet behind
                    //     text, above the dark wash" stack.
                    //
                    // The scale animation already carries the depth
                    // cue; z only needs a sign flip.
                    planet_z_clones[i].set(if sin_t > 0.0 { 1.0 } else { 0.0 });
                }
            });
        });
    });

    refs
}
