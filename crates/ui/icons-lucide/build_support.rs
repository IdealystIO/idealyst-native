// Pure SVG → path-data conversion helpers shared by `build.rs` and the
// crate's tests (`tests/svg_convert.rs`).
//
// Lucide ships icons as a mix of `<path>`, `<circle>`, `<ellipse>`,
// `<line>`, `<polyline>`, `<polygon>` and `<rect>` elements. Every native
// backend (iOS/macOS/Android) renders icons by parsing a single SVG path
// `d` string — they have no `<circle>`/`<rect>` primitives. So this module
// normalizes *all* drawable elements down to equivalent path `d` strings at
// build time, preserving document order. Downstream, every backend only
// ever sees `paths: &[&str]`.
//
// This is `include!`d (not a normal module) so build.rs can use it with no
// crate/dependency wiring, and `tests/svg_convert.rs` can exercise the same
// code via `include!("../build_support.rs")`.

/// Format a number for embedding in path data: round to kill float noise,
/// then use Rust's shortest round-trip `Display` (e.g. `3.0 → "3"`,
/// `0.5 → "0.5"`). `-0.0` normalizes to `"0"`.
#[allow(dead_code)]
fn fmt(n: f64) -> String {
    let r = (n * 1000.0).round() / 1000.0;
    let r = if r == 0.0 { 0.0 } else { r }; // collapse -0.0
    format!("{}", r)
}

/// Read attribute `name` from a single element tag string. Requires the match
/// to be at an attribute boundary (preceded by whitespace or the tag's `<`),
/// so a query for `x` does not match inside `rx="..."`. Supports both quote
/// styles.
#[allow(dead_code)]
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    for quote in ['"', '\''] {
        let pat = format!("{}={}", name, quote);
        let mut from = 0;
        while let Some(p) = tag[from..].find(&pat) {
            let abs = from + p;
            let prev = tag[..abs].chars().last();
            let boundary = abs == 0 || prev.map_or(false, |c| c.is_whitespace() || c == '<');
            if boundary {
                let vstart = abs + pat.len();
                if let Some(e) = tag[vstart..].find(quote) {
                    return Some(&tag[vstart..vstart + e]);
                }
            }
            from = abs + pat.len();
        }
    }
    None
}

/// Parse a numeric attribute, defaulting to `0.0` when absent/unparseable.
#[allow(dead_code)]
fn num(tag: &str, name: &str) -> f64 {
    attr(tag, name)
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0.0)
}

/// Parse an optional numeric attribute (`None` when absent).
#[allow(dead_code)]
fn num_opt(tag: &str, name: &str) -> Option<f64> {
    attr(tag, name).and_then(|s| s.trim().parse().ok())
}

/// A circle as two semicircular relative arcs. Matches Lucide's own
/// path-equivalent form (`M (cx-r) cy a r r 0 1 0 2r 0 a r r 0 1 0 -2r 0 Z`).
#[allow(dead_code)]
fn circle_path(cx: f64, cy: f64, r: f64) -> String {
    ellipse_path(cx, cy, r, r)
}

/// An ellipse as two semielliptical relative arcs.
#[allow(dead_code)]
fn ellipse_path(cx: f64, cy: f64, rx: f64, ry: f64) -> String {
    format!(
        "M{} {}a{} {} 0 1 0 {} 0a{} {} 0 1 0 {} 0Z",
        fmt(cx - rx),
        fmt(cy),
        fmt(rx),
        fmt(ry),
        fmt(2.0 * rx),
        fmt(rx),
        fmt(ry),
        fmt(-2.0 * rx),
    )
}

/// A straight line segment.
#[allow(dead_code)]
fn line_path(x1: f64, y1: f64, x2: f64, y2: f64) -> String {
    format!("M{} {}L{} {}", fmt(x1), fmt(y1), fmt(x2), fmt(y2))
}

/// `<polyline>`/`<polygon>` points → `M … L … [Z]`. Returns `None` if the
/// point list is malformed (fewer than 2 points or an odd coordinate count).
#[allow(dead_code)]
fn points_path(tag: &str, close: bool) -> Option<String> {
    let pts = attr(tag, "points")?;
    let nums: Vec<f64> = pts
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if nums.len() < 4 || nums.len() % 2 != 0 {
        return None;
    }
    let mut s = format!("M{} {}", fmt(nums[0]), fmt(nums[1]));
    let mut i = 2;
    while i + 1 < nums.len() {
        s.push_str(&format!("L{} {}", fmt(nums[i]), fmt(nums[i + 1])));
        i += 2;
    }
    if close {
        s.push('Z');
    }
    Some(s)
}

/// `<rect>` → rectangle path. Honors `rx`/`ry` rounded corners (with the SVG
/// rule that a missing radius mirrors the present one, and each is clamped to
/// half the corresponding side). A square-cornered rect emits the simple
/// `M x y h w v h h -w Z` form.
#[allow(dead_code)]
fn rect_path(tag: &str) -> String {
    let x = num(tag, "x");
    let y = num(tag, "y");
    let w = num(tag, "width");
    let h = num(tag, "height");
    let rx0 = num_opt(tag, "rx");
    let ry0 = num_opt(tag, "ry");
    let mut rx = rx0.or(ry0).unwrap_or(0.0).max(0.0);
    let mut ry = ry0.or(rx0).unwrap_or(0.0).max(0.0);
    rx = rx.min(w / 2.0);
    ry = ry.min(h / 2.0);

    if rx <= 0.0 || ry <= 0.0 {
        return format!(
            "M{} {}h{}v{}h{}Z",
            fmt(x),
            fmt(y),
            fmt(w),
            fmt(h),
            fmt(-w)
        );
    }
    format!(
        "M{} {}h{}a{} {} 0 0 1 {} {}v{}a{} {} 0 0 1 {} {}h{}a{} {} 0 0 1 {} {}v{}a{} {} 0 0 1 {} {}Z",
        fmt(x + rx),
        fmt(y),
        fmt(w - 2.0 * rx),
        fmt(rx), fmt(ry), fmt(rx), fmt(ry),
        fmt(h - 2.0 * ry),
        fmt(rx), fmt(ry), fmt(-rx), fmt(ry),
        fmt(-(w - 2.0 * rx)),
        fmt(rx), fmt(ry), fmt(-rx), fmt(-ry),
        fmt(-(h - 2.0 * ry)),
        fmt(rx), fmt(ry), fmt(rx), fmt(-ry),
    )
}

/// Convert one element (`name` = tag name, `tag` = full `<… >` text) into a
/// path `d` string, or `None` for non-drawable elements (`svg`, etc.).
#[allow(dead_code)]
fn convert_element(name: &str, tag: &str) -> Option<String> {
    match name {
        "path" => attr(tag, "d").map(|s| s.to_string()),
        "circle" => Some(circle_path(num(tag, "cx"), num(tag, "cy"), num(tag, "r"))),
        "ellipse" => Some(ellipse_path(
            num(tag, "cx"),
            num(tag, "cy"),
            num(tag, "rx"),
            num(tag, "ry"),
        )),
        "line" => Some(line_path(
            num(tag, "x1"),
            num(tag, "y1"),
            num(tag, "x2"),
            num(tag, "y2"),
        )),
        "polyline" => points_path(tag, false),
        "polygon" => points_path(tag, true),
        "rect" => Some(rect_path(tag)),
        _ => None,
    }
}

/// Walk an SVG document in order and return one path `d` string per drawable
/// element (skipping comments and the `<svg>` wrapper). Order is preserved so
/// multi-part icons composite identically to the source.
#[allow(dead_code)]
fn svg_to_paths(svg: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = svg.as_bytes();
    let mut i = 0;
    while i < svg.len() {
        if bytes[i] == b'<' {
            if svg[i..].starts_with("<!--") {
                match svg[i..].find("-->") {
                    Some(end) => {
                        i += end + 3;
                        continue;
                    }
                    None => break,
                }
            }
            let name: String = svg[i + 1..]
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            match svg[i..].find('>') {
                Some(gt) => {
                    let tag = &svg[i..i + gt + 1];
                    if let Some(d) = convert_element(&name, tag) {
                        out.push(d);
                    }
                    i += gt + 1;
                    continue;
                }
                None => break,
            }
        }
        i += 1;
    }
    out
}

/// Extract `viewBox="x y w h"` → `(w, h)`. Defaults handled by the caller.
#[allow(dead_code)]
fn extract_view_box(svg: &str) -> Option<(u16, u16)> {
    let vb = attr(svg, "viewBox")?;
    let parts: Vec<&str> = vb.split_whitespace().collect();
    if parts.len() == 4 {
        Some((parts[2].parse().ok()?, parts[3].parse().ok()?))
    } else {
        None
    }
}

/// Convert a kebab-case / snake_case icon name to a valid SCREAMING_SNAKE_CASE
/// Rust identifier (prefixing `_` when it would otherwise start with a digit).
#[allow(dead_code)]
fn to_screaming_snake(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch == '-' || ch == ' ' {
            result.push('_');
        } else {
            result.push(ch.to_ascii_uppercase());
        }
    }
    if result.starts_with(|c: char| c.is_ascii_digit()) {
        result.insert(0, '_');
    }
    result
}
