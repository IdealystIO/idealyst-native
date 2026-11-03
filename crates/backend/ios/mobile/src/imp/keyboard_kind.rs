//! `text_input`'s soft-keyboard hint ([`KeyboardKind`]) on `UITextField`.
//!
//! One kind sets four `UITextInputTraits` together — `keyboardType` (the
//! layout), `textContentType` (the AutoFill hint: the QuickType bar offers
//! the user's own email / phone / URL), and auto-capitalization /
//! auto-correction, which an email or URL must not get (a capitalized or
//! "corrected" address is a wrong address). They are set as a unit so a
//! kind flipping back to `Default` restores every trait, not only the
//! layout.
//!
//! `secureTextEntry` is a separate trait, so `secure` composes with any
//! kind in either call order (UIKit keeps the chosen layout under the mask
//! and disables auto-correction itself while secure).

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::NSString;
use runtime_shared::primitives::text_input::KeyboardKind;

// `UITextContentType` constants. Linked from UIKit (which the crate
// already links) rather than spelled as string literals: the constants'
// values are not API, the symbols are.
extern "C" {
    static UITextContentTypeEmailAddress: &'static NSString;
    static UITextContentTypeTelephoneNumber: &'static NSString;
    static UITextContentTypeURL: &'static NSString;
}

/// Which `UITextContentType` constant a kind sets (`None` = nil).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentType {
    EmailAddress,
    TelephoneNumber,
    Url,
}

/// The `UITextInputTraits` values for one kind. Raw enum values, so the
/// mapping is a pure function the tests pin without UIKit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Traits {
    /// `UIKeyboardType`.
    pub keyboard_type: isize,
    pub content_type: Option<ContentType>,
    /// `UITextAutocapitalizationType`.
    pub autocapitalization: isize,
    /// `UITextAutocorrectionType`.
    pub autocorrection: isize,
    /// `UITextSpellCheckingType`.
    pub spell_checking: isize,
}

// UIKeyboardType.
const KEYBOARD_DEFAULT: isize = 0;
const KEYBOARD_URL: isize = 3;
const KEYBOARD_NUMBER_PAD: isize = 4;
const KEYBOARD_PHONE_PAD: isize = 5;
const KEYBOARD_EMAIL_ADDRESS: isize = 7;
const KEYBOARD_DECIMAL_PAD: isize = 8;
// UITextAutocapitalizationType: `.none` / `.sentences` (UITextField's
// default, restored for `Default`).
const AUTOCAP_NONE: isize = 0;
const AUTOCAP_SENTENCES: isize = 2;
// UITextAutocorrectionType / UITextSpellCheckingType: `.default` / `.no`.
const TRAIT_DEFAULT: isize = 0;
const TRAIT_NO: isize = 1;

pub(crate) fn traits(kind: KeyboardKind) -> Traits {
    let (keyboard_type, content_type, address) = match kind {
        KeyboardKind::Default => (KEYBOARD_DEFAULT, None, false),
        KeyboardKind::Email => (KEYBOARD_EMAIL_ADDRESS, Some(ContentType::EmailAddress), true),
        KeyboardKind::Phone => (KEYBOARD_PHONE_PAD, Some(ContentType::TelephoneNumber), false),
        KeyboardKind::Number => (KEYBOARD_NUMBER_PAD, None, false),
        KeyboardKind::Decimal => (KEYBOARD_DECIMAL_PAD, None, false),
        KeyboardKind::Url => (KEYBOARD_URL, Some(ContentType::Url), true),
    };
    Traits {
        keyboard_type,
        content_type,
        autocapitalization: if address { AUTOCAP_NONE } else { AUTOCAP_SENTENCES },
        autocorrection: if address { TRAIT_NO } else { TRAIT_DEFAULT },
        spell_checking: if address { TRAIT_NO } else { TRAIT_DEFAULT },
    }
}

/// The object that implements text-input-trait selector `sel` for
/// `field`: the field, when its class implements it, else the object the
/// field forwards it to — the receiver the Objective-C runtime itself
/// picks.
///
/// INVARIANT (iOS 26): `UITextField` / `UITextView` implement almost none
/// of the `UITextInputTraits` accessors themselves (`setKeyboardType:`,
/// `setTextContentType:`, `setAutocapitalizationType:`, … have no method
/// on the class) and hand them to an internal `UITextInputTraits` through
/// `forwardingTargetForSelector:`. The message works, but objc2's
/// debug-build check (`class_getInstanceMethod` on the receiver's class)
/// finds no method and panics — "invalid message send to
/// -[IdealystTextField setKeyboardType:]: method not found", an abort on
/// mount for every field with a typed keyboard. Not "always send to the
/// forwarding target": a few setters (`setSecureTextEntry:`) ARE the
/// field's own, overriding the traits object's, and the field must get
/// those. Checked on the iOS 26.5 simulator.
fn trait_receiver(field: &AnyObject, sel: Sel) -> Retained<AnyObject> {
    let own = || {
        // SAFETY: `field` is a live object; retaining it is always sound.
        unsafe { Retained::retain(field as *const AnyObject as *mut AnyObject) }.expect("non-null")
    };
    if field.class().instance_method(sel).is_some() {
        return own();
    }
    // SAFETY: `forwardingTargetForSelector:` is NSObject API on every
    // object, returning the object the runtime would forward `sel` to,
    // or nil.
    let target: *mut AnyObject = unsafe { msg_send![field, forwardingTargetForSelector: sel] };
    // SAFETY: nil or a live object the field owns; retained for the send.
    unsafe { Retained::retain(target) }.unwrap_or_else(own)
}

/// The kind `field`'s `keyboardType` shows — the robot's
/// `native_keyboard` (`IntrospectionOps::keyboard_kind`) — read from the
/// receiver [`trait_receiver`] picks (the same forwarding invariant as the
/// setters). A type no kind writes reads as the nearest kind's keyboard:
/// none does, so it reads `None`.
pub(crate) fn read(field: &AnyObject) -> Option<KeyboardKind> {
    let from = trait_receiver(field, sel!(keyboardType));
    let ty: isize = unsafe { msg_send![&*from, keyboardType] };
    kind_of_keyboard_type(ty)
}

/// The kind whose [`traits`] write `keyboard_type`.
pub(crate) fn kind_of_keyboard_type(keyboard_type: isize) -> Option<KeyboardKind> {
    KeyboardKind::ALL.into_iter().find(|k| traits(*k).keyboard_type == keyboard_type)
}

/// Mask `field`'s text (`secureTextEntry`), sent to the receiver
/// [`trait_receiver`] picks.
pub(crate) fn set_secure(field: &AnyObject, secure: bool) {
    let to = trait_receiver(field, sel!(setSecureTextEntry:));
    unsafe {
        let _: () = msg_send![&*to, setSecureTextEntry: Bool::new(secure)];
    }
}

/// Apply `kind` to a `UITextField` (any `UITextInputTraits` adopter),
/// each trait sent to the receiver [`trait_receiver`] picks.
///
/// INVARIANT: UIKit reads the traits when the keyboard is brought up, so a
/// change on a field that is already first responder only shows after
/// `reloadInputViews`. The mount handler sets the kind before anything can
/// focus the field; the reload covers a live kind changing mid-edit.
pub(crate) fn apply(field: &AnyObject, kind: KeyboardKind) {
    let t = traits(kind);
    let to = |sel: Sel| trait_receiver(field, sel);
    unsafe {
        let _: () = msg_send![&*to(sel!(setKeyboardType:)), setKeyboardType: t.keyboard_type];
        let content: Option<&NSString> = t.content_type.map(|c| match c {
            ContentType::EmailAddress => UITextContentTypeEmailAddress,
            ContentType::TelephoneNumber => UITextContentTypeTelephoneNumber,
            ContentType::Url => UITextContentTypeURL,
        });
        let _: () = msg_send![&*to(sel!(setTextContentType:)), setTextContentType: content];
        let _: () =
            msg_send![&*to(sel!(setAutocapitalizationType:)), setAutocapitalizationType: t.autocapitalization];
        let _: () = msg_send![&*to(sel!(setAutocorrectionType:)), setAutocorrectionType: t.autocorrection];
        let _: () = msg_send![&*to(sel!(setSpellCheckingType:)), setSpellCheckingType: t.spell_checking];
        let first: bool = msg_send![field, isFirstResponder];
        if first {
            let _: () = msg_send![field, reloadInputViews];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the `UIKeyboardType` raw values (UIKit's `UIKeyboardType` enum)
    /// and that Email/Url — and only they — turn off auto-capitalization,
    /// auto-correction and spell-checking.
    #[test]
    fn traits_per_kind() {
        let t = traits(KeyboardKind::Email);
        assert_eq!((t.keyboard_type, t.content_type), (7, Some(ContentType::EmailAddress)));
        assert_eq!((t.autocapitalization, t.autocorrection, t.spell_checking), (0, 1, 1));
        let t = traits(KeyboardKind::Url);
        assert_eq!((t.keyboard_type, t.content_type), (3, Some(ContentType::Url)));
        assert_eq!((t.autocapitalization, t.autocorrection, t.spell_checking), (0, 1, 1));
        let t = traits(KeyboardKind::Phone);
        assert_eq!((t.keyboard_type, t.content_type), (5, Some(ContentType::TelephoneNumber)));
        assert_eq!(traits(KeyboardKind::Number).keyboard_type, 4);
        assert_eq!(traits(KeyboardKind::Decimal).keyboard_type, 8);
        // `Default` restores UITextField's own defaults, so a live kind
        // flipping back leaves no trait behind.
        assert_eq!(
            traits(KeyboardKind::Default),
            Traits {
                keyboard_type: 0,
                content_type: None,
                autocapitalization: 2,
                autocorrection: 0,
                spell_checking: 0,
            }
        );
        for k in [KeyboardKind::Phone, KeyboardKind::Number, KeyboardKind::Decimal] {
            let t = traits(k);
            assert_eq!((t.autocapitalization, t.autocorrection), (2, 0), "{k:?}");
        }
    }

    /// Every kind's `UIKeyboardType` reads back as that kind (the robot's
    /// `native_keyboard`); a type no kind writes reads as none.
    #[test]
    fn every_keyboard_type_reads_back_as_its_kind() {
        for k in KeyboardKind::ALL {
            assert_eq!(kind_of_keyboard_type(traits(k).keyboard_type), Some(k), "{k:?}");
        }
        assert_eq!(kind_of_keyboard_type(1), None, "ASCII-capable is no kind's");
    }
}
