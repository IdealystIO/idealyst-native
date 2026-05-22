//! Case-conversion helper shared by `ui!`, `jsx!`, and `mcp_emit`.
//!
//! The framework's convention is:
//! - Function definitions use idiomatic Rust `snake_case`.
//! - `ui!` / `jsx!` call sites use React-style `PascalCase`.
//!
//! The macros bridge the two by lowering `PascalCase` to `snake_case`
//! at dispatch time: `PrimaryButton()` in `ui!` → `primary_button!()` →
//! `primary_button()`. Authors can also write the call site directly in
//! snake_case (`primary_button()`) — the conversion is idempotent on
//! already-lowercase input.
//!
//! This helper is the single source of truth for that conversion;
//! changing it ripples through `ui!`'s primitive match arms,
//! `jsx!`'s element-name lowering, and the MCP resolver's edge
//! matching.

/// Convert a `PascalCase` (or `camelCase`, or already-snake) string
/// to `snake_case`. Underscores already in the input survive
/// unchanged; we never insert a double underscore.
///
/// Acronym handling: a run of uppercase letters followed by a
/// lowercase letter is treated as `<acronym>_<word>`. So:
/// - `HTMLParser` → `html_parser`
/// - `IODevice`  → `io_device`
/// - `ABCDef`    → `abc_def`
/// - `iOS`       → `i_os` (degenerate; avoid in component names)
///
/// Digits are treated as lowercase-like for boundary detection, so
/// `Card2D` → `card2_d` (acceptable; consumers should avoid mixing
/// digits with trailing single-letter acronyms).
pub(crate) fn pascal_to_snake(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            let prev = chars[i - 1];
            // Boundary when:
            //   1. previous char is a lowercase letter or digit — i.e.
            //      a word just ended (FooBar / Card2D).
            //   2. previous char is uppercase but the next is
            //      lowercase — i.e. an acronym just ended and a new
            //      word starts (HTMLParser → ...L|Parser).
            let prev_lowerish = prev.is_ascii_lowercase() || prev.is_ascii_digit();
            let acronym_to_word = prev.is_ascii_uppercase()
                && chars
                    .get(i + 1)
                    .map(|n| n.is_ascii_lowercase())
                    .unwrap_or(false);
            if (prev_lowerish || acronym_to_word) && !out.ends_with('_') {
                out.push('_');
            }
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_to_snake_basics() {
        assert_eq!(pascal_to_snake("PrimaryButton"), "primary_button");
        assert_eq!(pascal_to_snake("Card"), "card");
        assert_eq!(pascal_to_snake("primary_button"), "primary_button");
        assert_eq!(pascal_to_snake(""), "");
    }

    #[test]
    fn pascal_to_snake_acronyms() {
        assert_eq!(pascal_to_snake("HTMLParser"), "html_parser");
        assert_eq!(pascal_to_snake("IODevice"), "io_device");
        assert_eq!(pascal_to_snake("ABCDef"), "abc_def");
    }

    #[test]
    fn pascal_to_snake_idempotent_on_snake() {
        let v = "icon_label";
        assert_eq!(pascal_to_snake(v), v);
        // No double underscores even if input already has them.
        assert_eq!(pascal_to_snake("icon__label"), "icon__label");
    }

    #[test]
    fn pascal_to_snake_digits() {
        assert_eq!(pascal_to_snake("Card2"), "card2");
        assert_eq!(pascal_to_snake("Card2D"), "card2_d");
    }
}
