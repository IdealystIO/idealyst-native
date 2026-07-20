//! Minimal JSONC → JSON comment stripping.
//!
//! VS Code's `settings.json` / `extensions.json` and Dev Containers'
//! `devcontainer.json` are JSONC (comments allowed), which `serde_json` won't
//! parse. This strips `//` line comments and `/* */` block comments while
//! leaving string contents intact — enough to re-parse with `serde_json`.
//! Rewriting a file this way loses its comments, so callers only rewrite when
//! they actually need to change something.

/// Strip `//` and `/* */` comments from JSONC, preserving strings and
/// newlines (so byte offsets in error messages stay roughly aligned).
pub fn strip(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                // Line comment — consume to end of line.
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                // Block comment — consume to the closing `*/`.
                chars.next(); // consume '*'
                let mut prev = '\0';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}
