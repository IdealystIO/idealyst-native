//! The open and close animation shared by anchored surfaces (`Popover`,
//! and the `Menu` / `SubMenu` / `Select` panel): a quick fade with a slight
//! scale, so a surface arrives and leaves instead of blinking.
//!
//! These surfaces are host-gated — `if open.get() { Menu(…) { … } }` — and
//! stay that way. Closing swaps the `if` branch out, and the surface asks
//! that branch to keep it mounted for the length of the exit
//! (`runtime_core::hold_on_exit`); the fade out plays in that window and
//! the subtree is removed when it ends. A surface that is not inside a
//! conditional has nothing to hold it and simply disappears with its
//! parent.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use runtime_core::animation::{AnimProp, AnimatedValue, TweenTo};
use runtime_core::{hold_on_exit, reduce_motion, Ref, StyleRules, Tokenized, ViewHandle};

/// How long the surface takes to arrive. Short enough that a menu still
/// feels immediate under a press; long enough to read as motion.
pub(crate) const POP_IN_MS: u64 = 120;

/// How long the surface takes to leave. Shorter than the arrival: a
/// dismissal should get out of the way.
pub(crate) const POP_OUT_MS: u64 = 90;

/// The scale the surface starts from, and leaves toward. Close to 1 on purpose: the surface
/// should settle, not zoom.
pub(crate) const POP_IN_FROM_SCALE: f32 = 0.96;

/// The static rules an animated surface must carry: it starts fully
/// transparent. The animator's bind runs before the view mounts, so its
/// first write is skipped; without this the surface paints one
/// full-opacity frame before the fade starts (an open flicker).
///
/// Put this on a STATIC style only. A reactive style closure re-applies its
/// rules when it re-runs, which would zero the alpha again mid-life.
pub(crate) fn pop_in_start_rules() -> StyleRules {
    StyleRules { opacity: Some(Tokenized::Literal(0.0)), ..Default::default() }
}

/// The animators behind one surface's arrival. Owned by the scope that
/// called [`pop_in`]; returned so a test can reach them.
pub(crate) struct PopIn {
    pub opacity: AnimatedValue<f32>,
    /// `None` under reduced motion.
    pub scale: Option<AnimatedValue<f32>>,
}

/// Fade (and, unless the user asked for reduced motion, slightly scale) the
/// view bound to `target` in now, and back out when the conditional that
/// shows it switches it off. Call it while building the surface, with the
/// same `Ref` the surface view is bound to and [`pop_in_start_rules`] on
/// its style. The animators are owned by the calling scope.
pub(crate) fn pop_in(target: Ref<ViewHandle>) -> PopIn {
    let arrive = Duration::from_millis(POP_IN_MS);

    let opacity = AnimatedValue::new(0.0_f32);
    opacity.bind(target, AnimProp::Opacity);
    opacity.animate(TweenTo::new(1.0, arrive).ease_out());

    // Reduced motion keeps the fade (a state change still needs to read)
    // and drops the movement. Read once: the surface is opening now.
    let scale = (!runtime_core::untrack(|| reduce_motion().get())).then(|| {
        let scale = AnimatedValue::new(POP_IN_FROM_SCALE);
        scale.bind(target, AnimProp::Scale);
        scale.animate(TweenTo::new(1.0, arrive).ease_out());
        scale
    });

    // The way out: held mounted for the exit, then removed.
    {
        let (opacity, scale) = (opacity.clone(), scale.clone());
        hold_on_exit(POP_OUT_MS as u32, move || {
            let leave = Duration::from_millis(POP_OUT_MS);
            opacity.animate(TweenTo::new(0.0, leave).ease_out());
            if let Some(scale) = scale {
                scale.animate(TweenTo::new(POP_IN_FROM_SCALE, leave).ease_out());
            }
        });
    }
    PopIn { opacity, scale }
}

/// A flag that turns `true` the moment the conditional showing the calling
/// component switches it off. A surface held mounted for its exit is still
/// live — its rows still take presses — so anything that must not fire
/// twice (a menu row a second click lands on while it fades) checks this.
/// Call it from a component body built in the same `if` as the surface.
pub(crate) fn leaving() -> Rc<Cell<bool>> {
    let flag = Rc::new(Cell::new(false));
    let set = flag.clone();
    hold_on_exit(0, move || set.set(true));
    flag
}

#[cfg(test)]
mod tests {
    use super::*;
    use host_mock::{take_animated_log, Harness};
    use runtime_core::IntoElement as _;
    use runtime_shared::app_state::{self, AppStateChange};

    /// Mount a view bound to a ref `pop_in` animates; hand back its
    /// animators. The tween itself cannot be watched here: host-mock's
    /// frame loop never ticks, so no animation advances in a test. What is
    /// reachable is that each animator is bound to the MOUNTED view, which
    /// is the part a component can get wrong (a ref that is never bound, a
    /// scale left in under reduced motion).
    fn mounted(reduce: bool) -> (Harness, runtime_scene::Realized<host_mock::Node>, PopIn) {
        app_state::reset_for_tests();
        app_state::report(AppStateChange::ReduceMotion(reduce));
        let h = Harness::new();
        let (tree, anim) = h.world.enter(|| {
            let target: Ref<ViewHandle> = Ref::new();
            let anim = pop_in(target);
            (runtime_core::view(Vec::new()).bind(target).into_element(), anim)
        });
        let realized = h.mount(tree);
        h.flush();
        take_animated_log();
        (h, realized, anim)
    }

    fn props_written() -> Vec<String> {
        take_animated_log().into_iter().filter_map(|l| l.split(' ').nth(2).map(str::to_string)).collect()
    }

    #[test]
    fn opacity_and_scale_drive_the_mounted_surface() {
        let (_h, realized, anim) = mounted(false);
        anim.opacity.set(0.5);
        assert_eq!(props_written(), ["Opacity"]);
        anim.scale.as_ref().expect("scale animates unless motion is reduced").set(0.98);
        assert_eq!(props_written(), ["Scale"]);
        drop(realized);
    }

    #[test]
    fn reduced_motion_keeps_the_fade_and_drops_the_scale() {
        let (_h, realized, anim) = mounted(true);
        assert!(anim.scale.is_none());
        anim.opacity.set(0.5);
        assert_eq!(props_written(), ["Opacity"]);
        drop(realized);
    }

    #[test]
    fn a_surface_starts_transparent() {
        assert_eq!(pop_in_start_rules().opacity, Some(Tokenized::Literal(0.0)));
    }
}
