//! Compile-checked usage **recipes** for the storage SDK.
//!
//! Motivated by the arena's run-2 feedback report: `describe_sdk("storage")`
//! gave only a summary, so the agent read this crate's source to learn
//! `platform_storage` + async `get`/`set` and how to call them from UI code.
//! This recipe is that missing example, compiled against the live API so it
//! can't rot.
//!
//! `recipe!` self-gates on the `catalog` feature: production builds expand to
//! nothing, and `runtime-core` isn't even a dependency then.

use runtime_core::recipe;

recipe!(
    platform_storage,
    /// A text note that persists across app restarts. `platform_storage(ns)`
    /// returns the platform's plaintext key-value store (localStorage /
    /// NSUserDefaults / SharedPreferences / a JSON file). Its `get`/`set` are
    /// async — call them from UI code via `runtime_core::driver::spawn_async`
    /// (no `Send` bound; the generated app wrappers pre-install the executor,
    /// so no setup is needed). Hydrate on build, persist on change.
    pub fn persisted_note() -> ::runtime_core::Element {
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::{signal, ui};

        let note = signal(String::new());

        // Hydrate once at build: read the saved value into the signal.
        spawn_async(async move {
            let store = crate::platform_storage("recipes-demo");
            if let Ok(Some(saved)) = store.get("note").await {
                note.set(saved);
            }
        });

        // Persist on every change.
        let persist = move |text: String| {
            note.set(text.clone());
            spawn_async(async move {
                let store = crate::platform_storage("recipes-demo");
                let _ = store.set("note", &text).await;
            });
        };

        ui! {
            view {
                text_input(
                    value = note,
                    on_change = persist,
                    placeholder = "Saved as you type",
                )
                text { "Current: {note}" }
            }
        }
    }
);
