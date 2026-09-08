//! What a plain label writes to UIKit for a given `text-transform`.
//!
//! UIKit has no text-transform property. The only way to render one is
//! to transform the string handed to `setText:`, which makes the
//! displayed text lossy — and `accessibilityLabel` defaults to whatever
//! `text` says, so VoiceOver would announce an uppercased label as a
//! shout. CSS treats `text-transform` as presentational and screen
//! readers get the source text; this backend matches that by pinning
//! the untransformed string as the accessibility label whenever a
//! transform is active.
//!
//! That collides with [`crate::imp::a11y::apply`], which owns
//! `accessibilityLabel` and CLEARS it when the author supplies none.
//! The resolution is the same one `create_link_impl` already uses for
//! the Link's default label: the author's own `a11y.label` always wins,
//! and the derived fallback is (re-)asserted around it.
//!
//! Kept un-gated (no `target_os`) so the decision is host-testable; the
//! `setText:` / `performSelector:` half that consumes it is ios-only.

use runtime_shared::TextTransform;

/// What to do with `accessibilityLabel` after writing a label's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum A11yLabelWrite {
    /// Leave the property alone — `a11y::apply` owns it.
    ///
    /// Both the author-labelled case (their string must survive) and
    /// the un-transformed case (UIKit's own `text` fallback is already
    /// the author's words) land here.
    Leave,
    /// Pin the UNTRANSFORMED text, because the displayed string is not
    /// what the author wrote.
    SetRaw,
    /// A transform was removed. The pin we installed is now stale and
    /// has to go, so UIKit falls back to `text` again.
    Clear,
}

/// Decide the `accessibilityLabel` write for a label whose text was
/// just set.
///
/// `previous` is the transform baked into the string UIKit was
/// displaying, `next` the one now being applied. The transition matters:
/// only a label that HAD a pin needs it cleared, and clearing one we
/// never set would stomp whatever `a11y::apply` last wrote.
pub(crate) fn a11y_label_write(
    previous: TextTransform,
    next: TextTransform,
    author_labelled: bool,
) -> A11yLabelWrite {
    if author_labelled {
        return A11yLabelWrite::Leave;
    }
    match (previous, next) {
        (_, t) if t != TextTransform::None => A11yLabelWrite::SetRaw,
        (TextTransform::None, _) => A11yLabelWrite::Leave,
        // previous was a real transform, next is None.
        _ => A11yLabelWrite::Clear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An author's own `a11y.label` outranks the derived fallback in
    /// every direction — the same precedence `create_link_impl` gives
    /// the Link's default label.
    #[test]
    fn an_author_label_is_never_overwritten() {
        for prev in [TextTransform::None, TextTransform::Uppercase] {
            for next in [
                TextTransform::None,
                TextTransform::Uppercase,
                TextTransform::Lowercase,
                TextTransform::Capitalize,
            ] {
                assert_eq!(
                    a11y_label_write(prev, next, true),
                    A11yLabelWrite::Leave,
                    "{prev:?} -> {next:?}"
                );
            }
        }
    }

    /// With a transform active the displayed string is not the author's,
    /// so the untransformed text has to be announced instead.
    #[test]
    fn an_active_transform_pins_the_untransformed_text() {
        assert_eq!(
            a11y_label_write(TextTransform::None, TextTransform::Uppercase, false),
            A11yLabelWrite::SetRaw
        );
        // Re-applied on a text change under a standing transform, too.
        assert_eq!(
            a11y_label_write(TextTransform::Uppercase, TextTransform::Uppercase, false),
            A11yLabelWrite::SetRaw
        );
    }

    /// Regression: clearing unconditionally when `next` is `None` would
    /// wipe whatever `a11y::apply` had written for a label that never
    /// carried a transform — which is every plain label in the tree, on
    /// every restyle.
    #[test]
    fn regression_an_untransformed_label_is_left_alone() {
        assert_eq!(
            a11y_label_write(TextTransform::None, TextTransform::None, false),
            A11yLabelWrite::Leave
        );
    }

    /// Dropping a transform has to drop the pin with it, or the label
    /// keeps announcing a string that is now identical to its own text
    /// — harmless today, wrong the moment the text changes again.
    #[test]
    fn removing_a_transform_clears_the_pin() {
        assert_eq!(
            a11y_label_write(TextTransform::Uppercase, TextTransform::None, false),
            A11yLabelWrite::Clear
        );
    }
}
