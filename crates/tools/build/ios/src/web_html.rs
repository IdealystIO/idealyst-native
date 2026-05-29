//! Tiny HTML-augmentation helpers shared by `build-web` (staging-time
//! rewrite of the deployed `index.html`) and `dev-http` (request-time
//! injection while serving the project's `index.html`).
//!
//! These two paths MUST emit the same preload tags from the same input
//! list — otherwise the dev loop's "what does the browser see" diverges
//! from the deployed bundle's, and a font-flash bug "only" reproduces
//! in one mode. One source of truth for both call sites.
//!
//! Lives here (not in a `dev-bundle-prep` crate) because the input —
//! `manifest.app.web.preload_fonts` — comes from the same TOML schema
//! `build-ios` parses; co-locating the producer and the rendering
//! helpers keeps the seam tight.

/// Render `<link rel="preload">` tags for each `path` in the user's
/// `[package.metadata.idealyst.app.web].preload_fonts` list. Each path
/// is a project-relative file path (e.g. `"fonts/Inter-Regular.ttf"`);
/// the rendered tag carries the URL form (`"/fonts/Inter-Regular.ttf"`),
/// the matching MIME type derived from the extension, and
/// `crossorigin` (required for the preload to dedupe with the
/// framework's `@font-face` fetch — without it the browser opens two
/// requests for the same font).
///
/// Returns the empty string when `paths` is empty so callers can pass
/// it through unconditionally.
pub fn font_preload_tags(paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(paths.len() * 90);
    for path in paths {
        let url = ensure_root_absolute(path);
        let mime_attr = font_mime(&url)
            .map(|m| format!(r#" type="{m}""#))
            .unwrap_or_default();
        out.push_str(&format!(
            "<link rel=\"preload\" as=\"font\" crossorigin{mime_attr} href=\"{url}\">\n"
        ));
    }
    out
}

/// Splice `snippet` into `html` immediately before `</head>` (or
/// prepend if the document has no `</head>` — the snippet still
/// executes first). Identity when `snippet` is empty.
///
/// Used by both `build-web`'s staging step and `dev-http`'s HTML
/// response path — same insertion mechanics as the existing
/// `inject_aas_url` / `inject_reload_script` in `dev-http`.
pub fn inject_into_head(html: String, snippet: &str) -> String {
    if snippet.is_empty() {
        return html;
    }
    if let Some(idx) = html.find("</head>") {
        let (head, tail) = html.split_at(idx);
        let mut out = String::with_capacity(html.len() + snippet.len());
        out.push_str(head);
        out.push_str(snippet);
        out.push_str(tail);
        out
    } else {
        format!("{snippet}{html}")
    }
}

fn ensure_root_absolute(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn font_mime(url: &str) -> Option<&'static str> {
    let ext = url.rsplit('.').next()?;
    Some(match ext.to_ascii_lowercase().as_str() {
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REGRESSION GUARD: each path → one preload tag, with `crossorigin`
    /// + matching MIME, root-absolute URL. The `crossorigin` attribute
    /// is the load-bearing detail: without it the browser refuses to
    /// dedupe the preload against the framework's `@font-face` fetch
    /// and the font downloads twice (the bug we hit earlier this
    /// session before adding it).
    #[test]
    fn font_preload_tags_emits_one_tag_per_path() {
        let paths = vec![
            "fonts/Inter-Regular.ttf".to_string(),
            "fonts/Inter-Bold.woff2".to_string(),
        ];
        let html = font_preload_tags(&paths);
        assert!(
            html.contains(
                r#"<link rel="preload" as="font" crossorigin type="font/ttf" href="/fonts/Inter-Regular.ttf">"#
            ),
            "got: {html}"
        );
        assert!(
            html.contains(
                r#"<link rel="preload" as="font" crossorigin type="font/woff2" href="/fonts/Inter-Bold.woff2">"#
            ),
            "got: {html}"
        );
    }

    /// Empty list → empty output. Callers can pass the result through
    /// `inject_into_head` unconditionally; that's also a no-op on empty.
    #[test]
    fn empty_list_emits_nothing() {
        assert!(font_preload_tags(&[]).is_empty());
    }

    /// Paths already root-absolute pass through unchanged — supports
    /// users who write `"/fonts/X.ttf"` (matching how the URL appears
    /// in the served HTML) as well as `"fonts/X.ttf"` (matching the
    /// disk layout).
    #[test]
    fn root_absolute_paths_pass_through() {
        let html = font_preload_tags(&["/fonts/A.ttf".to_string()]);
        assert!(html.contains(r#"href="/fonts/A.ttf""#));
    }

    /// `inject_into_head` splices before `</head>`.
    #[test]
    fn inject_into_head_splices_before_head_close() {
        let html = "<!doctype html><html><head><title>x</title></head><body></body></html>"
            .to_string();
        let out = inject_into_head(html, "<link rel=\"preload\" href=\"/x.ttf\">\n");
        let head_close = out.find("</head>").unwrap();
        let link = out.find("/x.ttf").unwrap();
        assert!(link < head_close);
    }

    /// Empty snippet leaves html untouched — callers can pass empty
    /// strings safely.
    #[test]
    fn inject_into_head_empty_snippet_is_identity() {
        let html = "<!doctype html><html><head></head><body></body></html>".to_string();
        assert_eq!(inject_into_head(html.clone(), ""), html);
    }
}
