/// A `signal(false)`-driven toast that animates in and out with
/// `presence` — the primitive that owns mount/unmount *timing* so an
/// enter animation can play on appearance and an exit animation can
/// finish before the subtree is torn down (a plain `if`/`when` would
/// tear it down instantly, with no window for the exit to play).
///
/// `presence(child)` returns a builder; chain `.present(...)` (the
/// open/close predicate), `.enter(...)` and `.exit(...)`. On enter,
/// `enter.state` is applied *before* the first paint then interpolated
/// back to rest over `enter.duration_ms`; on exit, `exit.state` is
/// interpolated *toward* and the subtree is held mounted for
/// `exit.duration_ms` before it drops. `PresenceState` carries only the
/// four universally-interpolatable properties (opacity + 2D translate +
/// uniform scale) — here a fade-and-slide, mirrored both ways. Finish
/// with `.into_element()` and splat the result into the tree.
pub fn animated_toast() -> ::runtime_core::Element {
    use ::runtime_core::{
        presence, signal, ui, Easing, IntoElement, PresenceAnim, PresenceState,
    };

    let open = signal(false);

    // The presence-controlled subtree. `child` is a closure so presence
    // can (re)build it per mount; it captures nothing here.
    let toast = presence(|| {
        ui! {
            view {
                text { "Saved" }
            }
        }
    })
    .present(move || open.get())
    // Enter: start faded + nudged down, settle to rest over 200ms.
    .enter(PresenceAnim::new(
        PresenceState::rest().opacity(0.0).translate_y(8.0),
        200,
        Easing::EaseOut,
    ))
    // Exit: fade + slide back out over 150ms, THEN unmount (the mirror
    // of enter — sharing the `PresenceState` shape reads symmetrically).
    .exit(PresenceAnim::new(
        PresenceState::rest().opacity(0.0).translate_y(8.0),
        150,
        Easing::EaseIn,
    ))
    .into_element();

    ui! {
        view {
            button(label = "Toggle toast", on_click = move || open.set(!open.get()))
            // Splat the built presence element as a sibling child (bare
            // identifier — no braces — is the pre-built-Element splat).
            toast
        }
    }
}
