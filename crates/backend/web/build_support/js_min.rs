// Conservative JS "minifier" for the embedded runtime shims.
//
// Shared by `build.rs` (which runs it over `runtime/js/*.js` into
// `OUT_DIR`) and `tests/minify_shims.rs` (which pins its semantics) via
// `include!` — build scripts can't be unit-tested directly.
//
// Deliberately NOT a real minifier: it only removes what provably cannot
// change program behavior —
//   * `//` line comments (the trailing newline is kept),
//   * `/* .. */` block comments (replaced by a newline when the comment
//     spans lines, else a space — ECMA-262 treats a multi-line comment as
//     a LineTerminator for automatic-semicolon-insertion, so collapsing
//     one to nothing could silently change ASI),
//   * leading/trailing horizontal whitespace per line,
//   * blank lines.
// Every newline in actual code survives, so ASI behavior is untouched.
// No identifier renaming, no line joining.
//
// String (`'`/`"`) and template literals (`` ` `` with `${}` nesting) are
// tracked so comment-looking bytes inside them are preserved. The one
// construct this scanner cannot handle is a REGEX LITERAL containing `//`
// or `/*` (e.g. `/[/*]/`) — the shims contain no regex literals at all,
// and `tests/minify_shims.rs::shims_contain_no_regex_literals` fails the
// build if one ever appears, pointing here.

/// Strip comments + dead whitespace from `src`. See module docs for the
/// exact (and deliberately small) set of transforms.
pub fn minify_js(src: &str) -> String {
    #[derive(PartialEq)]
    enum S {
        Code,
        Single,
        Double,
        Template,
        LineComment,
        BlockComment { has_newline: bool },
    }
    let mut out = String::with_capacity(src.len());
    let mut state = S::Code;
    // `${ .. }` inside a template drops back into Code; this tracks the
    // brace depth per nested template expression so `}` knows whether it
    // returns to the template or is ordinary code.
    let mut template_expr_depth: Vec<u32> = Vec::new();
    let mut in_template_expr = false;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match state {
            S::Code => match c {
                '/' if chars.peek() == Some(&'/') => {
                    chars.next();
                    state = S::LineComment;
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    state = S::BlockComment { has_newline: false };
                }
                '\'' => {
                    out.push(c);
                    state = S::Single;
                }
                '"' => {
                    out.push(c);
                    state = S::Double;
                }
                '`' => {
                    out.push(c);
                    state = S::Template;
                }
                '{' if in_template_expr => {
                    *template_expr_depth.last_mut().unwrap() += 1;
                    out.push(c);
                }
                '}' if in_template_expr => {
                    let d = template_expr_depth.last_mut().unwrap();
                    if *d == 0 {
                        template_expr_depth.pop();
                        in_template_expr = !template_expr_depth.is_empty();
                        out.push(c);
                        state = S::Template;
                    } else {
                        *d -= 1;
                        out.push(c);
                    }
                }
                _ => out.push(c),
            },
            S::Single => {
                out.push(c);
                match c {
                    '\\' => {
                        if let Some(n) = chars.next() {
                            out.push(n);
                        }
                    }
                    '\'' => state = S::Code,
                    _ => {}
                }
            }
            S::Double => {
                out.push(c);
                match c {
                    '\\' => {
                        if let Some(n) = chars.next() {
                            out.push(n);
                        }
                    }
                    '"' => state = S::Code,
                    _ => {}
                }
            }
            S::Template => {
                out.push(c);
                match c {
                    '\\' => {
                        if let Some(n) = chars.next() {
                            out.push(n);
                        }
                    }
                    '`' => state = S::Code,
                    '$' if chars.peek() == Some(&'{') => {
                        out.push(chars.next().unwrap());
                        template_expr_depth.push(0);
                        in_template_expr = true;
                        state = S::Code;
                    }
                    _ => {}
                }
            }
            S::LineComment => {
                if c == '\n' {
                    out.push('\n');
                    state = S::Code;
                }
            }
            S::BlockComment { has_newline } => match c {
                '*' if chars.peek() == Some(&'/') => {
                    chars.next();
                    // Preserve the comment's LineTerminator status for ASI.
                    out.push(if has_newline { '\n' } else { ' ' });
                    state = S::Code;
                }
                '\n' => state = S::BlockComment { has_newline: true },
                _ => {}
            },
        }
    }
    // Second pass: trim per-line horizontal whitespace, drop blank lines.
    let mut compact = String::with_capacity(out.len());
    for line in out.lines() {
        let t = line.trim();
        if !t.is_empty() {
            compact.push_str(t);
            compact.push('\n');
        }
    }
    compact
}
