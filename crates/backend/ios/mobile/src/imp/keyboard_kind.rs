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

use objc2::msg_send;
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

/// Apply `kind` to a `UITextField` (any `UITextInputTraits` adopter).
///
/// INVARIANT: UIKit reads the traits when the keyboard is brought up, so a
/// change on a field that is already first responder only shows after
/// `reloadInputViews`. The mount handler sets the kind before anything can
/// focus the field; the reload covers a live kind changing mid-edit.
pub(crate) fn apply<F: objc2::Message>(field: &F, kind: KeyboardKind) {
    let t = traits(kind);
    unsafe {
        let _: () = msg_send![field, setKeyboardType: t.keyboard_type];
        let content: Option<&NSString> = t.content_type.map(|c| match c {
            ContentType::EmailAddress => UITextContentTypeEmailAddress,
            ContentType::TelephoneNumber => UITextContentTypeTelephoneNumber,
            ContentType::Url => UITextContentTypeURL,
        });
        let _: () = msg_send![field, setTextContentType: content];
        let _: () = msg_send![field, setAutocapitalizationType: t.autocapitalization];
        let _: () = msg_send![field, setAutocorrectionType: t.autocorrection];
        let _: () = msg_send![field, setSpellCheckingType: t.spell_checking];
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
}
