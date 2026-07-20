//! Shared attributed-string assembly for styled text runs
//! (`runtime_core::styled_text`).
//!
//! Both Apple leaf backends realize a styled text node as ONE
//! `NSAttributedString` on the platform label (UILabel's
//! `attributedText`, NSTextField's `attributedStringValue`) — the
//! platform text engine then wraps the mixed-style paragraph as a
//! single unit, which is the whole point of the runs model.
//!
//! This module owns the toolkit-agnostic half: appending fragments
//! and building attribute dictionaries from already-constructed
//! platform objects. `UIFont`/`NSFont` and `UIColor`/`NSColor` differ
//! per toolkit, but they're all `NSObject`s to the attributed string,
//! and the attribute KEYS share raw values across UIKit and AppKit
//! ("NSFont", "NSColor", "NSBackgroundColor") — the same hard-coded
//! strings the codeblock SDK uses to avoid a linker dependency on the
//! imported constants. The leaf backends own font/color construction
//! and hand finished objects in via [`RunAttrs`].
//!
//! Only run DELTAS are attributed. Unattributed ranges take the
//! label's own font/text-color properties (both toolkits merge label
//! properties in for attributes the string doesn't specify), so the
//! paragraph style keeps flowing through the regular
//! `apply_text_style` path. The one ordering rule — set label font
//! properties BEFORE the attributed string, never after — is enforced
//! by the callers: every property change re-realizes the attributed
//! string afterwards (setting `.font` after `attributedText` stomps
//! per-range fonts on UIKit).

use objc2::rc::{Allocated, Retained};
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{
    NSAttributedString, NSMutableAttributedString, NSMutableDictionary, NSObject, NSString,
};

/// Raw attribute-key strings, identical on UIKit and AppKit.
const KEY_FONT: &str = "NSFont";
const KEY_FOREGROUND: &str = "NSColor";
const KEY_BACKGROUND: &str = "NSBackgroundColor";

/// Platform attribute objects for one run — `None` fields inherit the
/// label's own properties. Built by the leaf backend (it owns
/// `UIFont`/`NSFont` construction), consumed here.
pub struct RunAttrs {
    pub font: Option<Retained<NSObject>>,
    pub foreground: Option<Retained<NSObject>>,
    pub background: Option<Retained<NSObject>>,
}

impl RunAttrs {
    pub fn is_empty(&self) -> bool {
        self.font.is_none() && self.foreground.is_none() && self.background.is_none()
    }
}

/// Assemble the full attributed string: one fragment per `(text,
/// attrs)` pair, in order. Plain fragments (empty attrs) are appended
/// without an attribute dictionary so they inherit label properties.
pub fn build_attributed(runs: &[(&str, RunAttrs)]) -> Retained<NSMutableAttributedString> {
    let attributed: Retained<NSMutableAttributedString> = unsafe {
        let cls = objc2::class!(NSMutableAttributedString);
        msg_send_id![cls, new]
    };
    for (text, attrs) in runs {
        let ns_text = NSString::from_str(text);
        let fragment: Retained<NSAttributedString> = if attrs.is_empty() {
            unsafe {
                let cls = objc2::class!(NSAttributedString);
                let alloc: Allocated<NSAttributedString> = msg_send_id![cls, alloc];
                msg_send_id![alloc, initWithString: &*ns_text]
            }
        } else {
            let dict = attrs_dict(attrs);
            unsafe {
                let cls = objc2::class!(NSAttributedString);
                let alloc: Allocated<NSAttributedString> = msg_send_id![cls, alloc];
                msg_send_id![
                    alloc,
                    initWithString: &*ns_text,
                    attributes: &*dict,
                ]
            }
        };
        let _: () = unsafe { msg_send![&*attributed, appendAttributedString: &*fragment] };
    }
    attributed
}

fn attrs_dict(attrs: &RunAttrs) -> Retained<NSMutableDictionary<NSString, NSObject>> {
    let dict: Retained<NSMutableDictionary<NSString, NSObject>> = unsafe {
        let cls = objc2::class!(NSMutableDictionary);
        msg_send_id![cls, new]
    };
    let mut put = |key: &str, value: &NSObject| {
        let k = NSString::from_str(key);
        let _: () = unsafe { msg_send![&*dict, setObject: value, forKey: &*k] };
    };
    if let Some(f) = &attrs.font {
        put(KEY_FONT, f);
    }
    if let Some(c) = &attrs.foreground {
        put(KEY_FOREGROUND, c);
    }
    if let Some(b) = &attrs.background {
        put(KEY_BACKGROUND, b);
    }
    dict
}

/// Pick a concrete font size for a run: the run's own `font_size`
/// (resolved) when present, else the paragraph's. Shared so iOS and
/// macOS can't drift on the precedence.
pub fn run_font_size(style: &runtime_core::TextRunStyle, base_size: f64) -> f64 {
    match style.font_size.as_ref().map(|t| t.resolve()) {
        Some(runtime_core::Length::Px(px)) if px > 0.0 => px as f64,
        _ => base_size,
    }
}

/// Does this run style need a font object at all? (Any of family /
/// weight / size set — a color-only run inherits the label font.)
pub fn run_needs_font(style: &runtime_core::TextRunStyle) -> bool {
    style.font_family.is_some() || style.font_weight.is_some() || style.font_size.is_some()
}

/// Classify one entry of a CSS-ish font-family stack for system-font
/// dispatch. The leaf backend walks the stack in order: `Named` gets
/// looked up by PostScript/display name, the generics map to the
/// toolkit's system font constructors.
pub enum StackEntry<'a> {
    Monospace,
    SansSerif,
    Named(&'a str),
}

/// Split a `System("ui-monospace, SFMono-Regular, Menlo, monospace")`
/// stack into dispatchable entries, trimming whitespace and quotes.
pub fn parse_font_stack(stack: &str) -> Vec<StackEntry<'_>> {
    stack
        .split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\''))
        .filter(|s| !s.is_empty())
        .map(|s| match s.to_ascii_lowercase().as_str() {
            "monospace" | "ui-monospace" => StackEntry::Monospace,
            "sans-serif" | "system-ui" | "ui-sans-serif" => StackEntry::SansSerif,
            _ => StackEntry::Named(s),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Foundation-only assembly check (no AppKit/UIKit, runs headless):
    /// fragments append in order to the full concatenated string, and a
    /// styled run's attributes land on exactly its own range. Attribute
    /// VALUES are opaque `NSObject`s to the attributed string, so
    /// placeholder `NSString`s stand in for the platform color/font
    /// objects — the leaf backends' object construction is exercised by
    /// the live robot pass, not here (needs a UI toolkit + main thread).
    #[test]
    fn build_attributed_places_attrs_on_the_styled_range_only() {
        let marker: Retained<NSObject> = unsafe {
            let s = NSString::from_str("#bg-marker");
            Retained::cast(s)
        };
        let runs = [
            (
                "the ",
                RunAttrs { font: None, foreground: None, background: None },
            ),
            (
                "ui!",
                RunAttrs { font: None, foreground: None, background: Some(marker) },
            ),
            (
                " macro",
                RunAttrs { font: None, foreground: None, background: None },
            ),
        ];
        let attributed = build_attributed(&runs);

        let s: Retained<NSString> = unsafe { msg_send_id![&*attributed, string] };
        assert_eq!(s.to_string(), "the ui! macro");

        let key = NSString::from_str(KEY_BACKGROUND);
        // Index 5 is inside "ui!" (bytes 4..7) — attribute present.
        let inside: *mut NSObject = unsafe {
            msg_send![
                &*attributed,
                attribute: &*key,
                atIndex: 5usize,
                effectiveRange: std::ptr::null_mut::<objc2_foundation::NSRange>(),
            ]
        };
        assert!(!inside.is_null(), "styled range carries the background attr");
        // Index 1 is inside "the " — no attribute.
        let outside: *mut NSObject = unsafe {
            msg_send![
                &*attributed,
                attribute: &*key,
                atIndex: 1usize,
                effectiveRange: std::ptr::null_mut::<objc2_foundation::NSRange>(),
            ]
        };
        assert!(outside.is_null(), "plain range carries no background attr");
    }

    #[test]
    fn parse_font_stack_classifies_generics_and_names() {
        let entries = parse_font_stack("ui-monospace, \"SF Mono\", Menlo, monospace");
        assert!(matches!(entries[0], StackEntry::Monospace));
        assert!(matches!(entries[1], StackEntry::Named("SF Mono")));
        assert!(matches!(entries[2], StackEntry::Named("Menlo")));
        assert!(matches!(entries[3], StackEntry::Monospace));
    }

    #[test]
    fn run_font_size_prefers_run_size_over_base() {
        use runtime_core::{Length, Tokenized, TextRunStyle};
        let mut s = TextRunStyle::default();
        assert_eq!(run_font_size(&s, 17.0), 17.0);
        s.font_size = Some(Tokenized::Literal(Length::Px(13.0)));
        assert_eq!(run_font_size(&s, 17.0), 13.0);
    }
}
