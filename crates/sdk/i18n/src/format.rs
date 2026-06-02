//! Message formatting: lightweight `{name}` interpolation, with a
//! pluggable-formatter seam for richer (plural / gender / ICU) needs.

use std::cell::RefCell;
use std::fmt::Display;

thread_local! {
    /// Optional app-installed formatter. When set, [`format`] delegates to
    /// it instead of the built-in substitutor. Thread-local because the
    /// reactive runtime is single-threaded (UI thread).
    static FORMATTER: RefCell<Option<Box<dyn Fn(&str, &[(&str, String)]) -> String>>> =
        const { RefCell::new(None) };
}

/// Install a custom message formatter. It receives the message template
/// and the interpolation arguments (already stringified, in declaration
/// order) and returns the final string. This is the swap-in point for a
/// Fluent / ICU / plural layer built on top of `i18n` — the default is
/// plain `{name}` substitution.
///
/// Installing replaces any previously installed formatter.
pub fn install_formatter<F>(f: F)
where
    F: Fn(&str, &[(&str, String)]) -> String + 'static,
{
    FORMATTER.with(|c| *c.borrow_mut() = Some(Box::new(f)));
}

/// Remove any custom formatter, restoring the built-in substitutor.
pub fn clear_formatter() {
    FORMATTER.with(|c| *c.borrow_mut() = None);
}

/// Render `template` with `args`, substituting each `{name}` placeholder
/// with the matching argument's `Display` output. If a custom formatter is
/// installed it is used instead.
///
/// Generated message functions call this; you rarely call it directly.
pub fn format(template: &str, args: &[(&str, &dyn Display)]) -> String {
    // Stringify once so the installed formatter sees a simple, owned shape
    // (`&[(&str, String)]`) and doesn't have to deal with `dyn Display`
    // lifetimes.
    let owned: Vec<(&str, String)> = args.iter().map(|(k, v)| (*k, v.to_string())).collect();
    FORMATTER.with(|c| match &*c.borrow() {
        Some(custom) => custom(template, &owned),
        None => substitute(template, &owned),
    })
}

/// Single-pass `{name}` substitution. `{{` and `}}` are literal braces.
/// An unmatched/unterminated placeholder is emitted verbatim (so a stray
/// brace in a translation is visible, not silently eaten).
fn substitute(template: &str, args: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    out.push('{');
                    continue;
                }
                let mut name = String::new();
                let mut closed = false;
                for nc in chars.by_ref() {
                    if nc == '}' {
                        closed = true;
                        break;
                    }
                    name.push(nc);
                }
                if !closed {
                    out.push('{');
                    out.push_str(&name);
                    continue;
                }
                let key = name.trim();
                match args.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => out.push_str(v),
                    // Unknown placeholder: leave it literally so the gap is
                    // visible during development rather than vanishing.
                    None => {
                        out.push('{');
                        out.push_str(&name);
                        out.push('}');
                    }
                }
            }
            '}' => {
                if chars.peek() == Some(&'}') {
                    chars.next();
                }
                out.push('}');
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_named_placeholders() {
        let s = format("Hello, {name}!", &[("name", &"Ada")]);
        assert_eq!(s, "Hello, Ada!");
    }

    #[test]
    fn substitutes_multiple_and_repeated() {
        let s = format("{a}+{b}={a}{b}", &[("a", &1), ("b", &2)]);
        assert_eq!(s, "1+2=12");
    }

    #[test]
    fn escaped_braces_are_literal() {
        let s = format("{{not a placeholder}} {x}", &[("x", &"y")]);
        assert_eq!(s, "{not a placeholder} y");
    }

    #[test]
    fn unknown_placeholder_left_verbatim() {
        let s = format("{known} {missing}", &[("known", &"ok")]);
        assert_eq!(s, "ok {missing}");
    }

    #[test]
    fn whitespace_inside_braces_is_trimmed() {
        let s = format("{ name }", &[("name", &"z")]);
        assert_eq!(s, "z");
    }

    #[test]
    fn unterminated_brace_emitted_verbatim() {
        let s = format("a {oops", &[("oops", &"!")]);
        assert_eq!(s, "a {oops");
    }

    #[test]
    fn custom_formatter_takes_over_then_clears() {
        install_formatter(|tmpl, args| format!("[{tmpl}|{}]", args.len()));
        assert_eq!(format("x {a}", &[("a", &1)]), "[x {a}|1]");
        clear_formatter();
        assert_eq!(format("x {a}", &[("a", &1)]), "x 1");
    }
}
