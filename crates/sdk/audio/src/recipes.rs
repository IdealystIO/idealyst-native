//! Compile-checked usage **recipes** for the audio playback SDK.
//!
//! Each `recipe!(Target, fn ...)` is a real, type-checked example of how to
//! use the SDK. Because the fn compiles against the live API, a signature
//! change that isn't reflected here is a compile error (whenever the catalog
//! is built), so these examples can't silently rot — and the MCP / docs
//! surface them as trustworthy "how do I use this?" context.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every
//! production build) these expand to nothing — the recipes, and the imports
//! inside them, don't compile at all. So there's no `#[cfg]` here and no
//! cost in shipped apps. Recipes are self-contained (imports live inside
//! each fn) so the captured source reads as a complete, copy-pasteable
//! example.

use runtime_core::recipe;

recipe!(
    Sound,
    /// A button that plays a short sound effect on press. The sound is
    /// decoded once up front (the async [`load`](crate::load) is spawned via
    /// `spawn_async` and stashed in a shared slot); each press starts a
    /// fresh [`Playback`](crate::Playback). The handles live in `RefCell`
    /// slots — RAII, no `mem::forget` — so the sound isn't cut off the
    /// instant the press closure returns, and the previous voice is stopped
    /// (dropped) when the next press replaces it.
    pub fn play_sound_on_press() -> ::runtime_core::Element {
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::ui;
        use ::std::cell::RefCell;
        use ::std::rc::Rc;

        // Decode once on mount. In real code the bytes come from
        // `include_bytes!` (a bundled asset) or a file path. `load` is
        // async, so spawn it and stash the ready `Sound` in a shared slot.
        let sound: Rc<RefCell<Option<crate::Sound>>> = Rc::new(RefCell::new(None));
        let prepared = sound.clone();
        spawn_async(async move {
            if let Ok(s) = crate::load(crate::AudioSource::path("assets/ding.wav")).await {
                *prepared.borrow_mut() = Some(s);
            }
        });

        // Hold the active `Playback` so the next press's `play()` drops it
        // (stopping the prior voice) — RAII, no `mem::forget`.
        let active: Rc<RefCell<Option<crate::Playback>>> = Rc::new(RefCell::new(None));

        let on_click = move || {
            if let Some(s) = sound.borrow().as_ref() {
                *active.borrow_mut() = Some(s.play());
            }
        };

        ui! {
            button(label = "Play sound", on_click = on_click)
        }
    }
);
