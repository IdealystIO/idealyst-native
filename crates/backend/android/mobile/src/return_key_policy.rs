//! `text_input`'s return-key label as Android `EditorInfo` IME options.
//! Un-gated so the mapping is pinned on the host; the JNI half
//! (`setImeOptions`, then `InputMethodManager.restartInput` for a field the
//! keyboard is already up for) lives in `imp/primitives/text_input.rs`.
//!
//! A label only: the keyboard's action key still arrives as an editor
//! action, which `RustKeyListener.onEditorAction` reports to `on_key_down`
//! as Enter whichever action the key is labelled with.

use runtime_shared::primitives::text_input::ReturnKey;

/// `EditorInfo.IME_ACTION_*` raw values.
pub(crate) fn ime_action(key: ReturnKey) -> i32 {
    match key {
        ReturnKey::Default => 0, // IME_ACTION_UNSPECIFIED
        ReturnKey::Go => 2,
        ReturnKey::Search => 3,
        ReturnKey::Send => 4,
        ReturnKey::Next => 5,
        ReturnKey::Done => 6,
    }
}

/// `EditorInfo.IME_MASK_ACTION`: the bits [`ime_action`] owns.
pub(crate) const IME_MASK_ACTION: i32 = 0xff;

/// `current` IME options with the action bits replaced by `key`'s, the
/// flags (`IME_FLAG_*`) kept.
pub(crate) fn with_action(current: i32, key: ReturnKey) -> i32 {
    (current & !IME_MASK_ACTION) | ime_action(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ime_actions_per_key() {
        let got: Vec<i32> = ReturnKey::ALL.iter().map(|k| ime_action(*k)).collect();
        assert_eq!(got, [0, 5, 2, 6, 4, 3]);
    }

    #[test]
    fn the_action_replaces_only_the_action_bits() {
        const IME_FLAG_NO_EXTRACT_UI: i32 = 0x1000_0000;
        let opts = with_action(IME_FLAG_NO_EXTRACT_UI | 6, ReturnKey::Next);
        assert_eq!(opts, IME_FLAG_NO_EXTRACT_UI | 5);
        assert_eq!(with_action(opts, ReturnKey::Default), IME_FLAG_NO_EXTRACT_UI);
    }
}
