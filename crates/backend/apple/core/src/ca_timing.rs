//! The `CAMediaTimingFunction` name for a stylesheet [`Easing`], shared by
//! every explicit `CABasicAnimation` the Apple backends attach (the macOS
//! transform transition, the box-shadow transition). Cross-target — no
//! CoreAnimation calls — so the mapping is host-tested.

use runtime_shared::Easing;

/// The documented value behind the matching `kCAMediaTimingFunction*`
/// constant. `Ease` maps to `"default"` (Core Animation's ease, the closest
/// to CSS `ease`); `CubicBezier` approximates to ease-in-ease-out, because
/// the exact `functionWithControlPoints::::` path needs a four-anonymous-arg
/// selector `msg_send!` can't express cleanly.
pub fn ca_timing_name(easing: Easing) -> &'static str {
    match easing {
        Easing::Linear => "linear",
        Easing::EaseIn => "easeIn",
        Easing::EaseOut => "easeOut",
        Easing::EaseInOut | Easing::CubicBezier(..) => "easeInEaseOut",
        Easing::Ease => "default",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_easing_maps_to_a_core_animation_name() {
        assert_eq!(ca_timing_name(Easing::Linear), "linear");
        assert_eq!(ca_timing_name(Easing::EaseIn), "easeIn");
        assert_eq!(ca_timing_name(Easing::EaseOut), "easeOut");
        assert_eq!(ca_timing_name(Easing::EaseInOut), "easeInEaseOut");
        assert_eq!(ca_timing_name(Easing::Ease), "default");
        assert_eq!(ca_timing_name(Easing::CubicBezier(0.1, 0.2, 0.3, 0.4)), "easeInEaseOut");
    }
}
