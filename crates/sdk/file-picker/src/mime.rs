//! Pure MIME → platform-filter mappings.
//!
//! These are the host-testable core of the picker's type filtering: each
//! backend takes the author's MIME list (or [`MediaKind`]) and turns it into
//! whatever shape its native dialog wants — Apple Uniform Type Identifiers,
//! Windows `*.ext` filter specs, or a media-kind predicate. Keeping the
//! mappings here (no platform deps) means they run in the host unit tests
//! regardless of which backend compiles.
//!
//! The tables are deliberately small and conservative: common document and
//! media types. Unknowns fall back to "any item", so the picker still opens —
//! just unfiltered for that entry — rather than failing.

// Which of these helpers is actually *used* depends on which backend compiles
// (`apple_uttype` on Apple, `win_filter` on Windows, …), so some are dead on
// any single target. They're all exercised by the host tests below.
#![allow(dead_code)]

use crate::MediaKind;

/// Map a MIME type to an Apple Uniform Type Identifier (UTI), used to populate
/// `NSOpenPanel.allowedContentTypes` (macOS) and
/// `UIDocumentPickerViewController`'s content types (iOS).
///
/// Falls back to `"public.item"` (the root "any file" type) for anything not in
/// the table, which leaves the picker open to all files for that entry.
pub(crate) fn apple_uttype(mime: &str) -> &'static str {
    match normalize(mime).as_str() {
        "" | "*/*" | "application/octet-stream" => "public.item",
        "image/*" => "public.image",
        "video/*" => "public.movie",
        "audio/*" => "public.audio",
        "text/*" => "public.text",
        "application/pdf" => "com.adobe.pdf",
        "image/png" => "public.png",
        "image/jpeg" => "public.jpeg",
        "image/gif" => "com.compuserve.gif",
        "image/heic" => "public.heic",
        "image/webp" => "org.webmproject.webp",
        "video/mp4" => "public.mpeg-4",
        "video/quicktime" => "com.apple.quicktime-movie",
        "audio/mpeg" => "public.mp3",
        "text/plain" => "public.plain-text",
        "text/csv" => "public.comma-separated-values-text",
        "text/html" => "public.html",
        "text/markdown" => "net.daringfireball.markdown",
        "application/json" => "public.json",
        "application/zip" => "public.zip-archive",
        _ => "public.item",
    }
}

/// Map a MIME type to a Windows file-dialog filter: a human label plus a
/// semicolon-separated glob pattern (e.g. `("PNG image", "*.png")`,
/// `("Image", "*.png;*.jpg;*.jpeg;*.gif;*.bmp;*.webp")`).
///
/// Returns `None` for the "any file" cases so the caller can emit a single
/// `("All files", "*.*")` spec instead.
pub(crate) fn win_filter(mime: &str) -> Option<(&'static str, &'static str)> {
    Some(match normalize(mime).as_str() {
        "" | "*/*" | "application/octet-stream" => return None,
        "image/*" => ("Images", "*.png;*.jpg;*.jpeg;*.gif;*.bmp;*.webp;*.heic"),
        "video/*" => ("Videos", "*.mp4;*.mov;*.m4v;*.webm;*.avi;*.mkv"),
        "audio/*" => ("Audio", "*.mp3;*.m4a;*.wav;*.aac;*.flac;*.ogg"),
        "text/*" => ("Text", "*.txt;*.csv;*.md;*.json;*.html"),
        "application/pdf" => ("PDF", "*.pdf"),
        "image/png" => ("PNG image", "*.png"),
        "image/jpeg" => ("JPEG image", "*.jpg;*.jpeg"),
        "image/gif" => ("GIF image", "*.gif"),
        "image/heic" => ("HEIC image", "*.heic"),
        "image/webp" => ("WebP image", "*.webp"),
        "video/mp4" => ("MP4 video", "*.mp4;*.m4v"),
        "video/quicktime" => ("QuickTime video", "*.mov"),
        "audio/mpeg" => ("MP3 audio", "*.mp3"),
        "text/plain" => ("Text", "*.txt"),
        "text/csv" => ("CSV", "*.csv"),
        "text/html" => ("HTML", "*.html;*.htm"),
        "text/markdown" => ("Markdown", "*.md"),
        "application/json" => ("JSON", "*.json"),
        "application/zip" => ("ZIP archive", "*.zip"),
        _ => return None,
    })
}

/// The wildcard MIME types for a media kind, used by the desktop/web backends
/// that have no dedicated media picker and fall back to the document picker
/// with image/video filters. (iOS/Android route [`MediaKind`] straight to their
/// native photo pickers instead and don't call this.)
pub(crate) fn media_mimes(kind: MediaKind) -> &'static [&'static str] {
    match kind {
        MediaKind::Images => &["image/*"],
        MediaKind::Videos => &["video/*"],
        MediaKind::ImagesAndVideos => &["image/*", "video/*"],
    }
}

/// Best-effort MIME type from a file name's extension, for the backends whose
/// native dialog reports only a path (macOS/Windows/Linux). Falls back to
/// `"application/octet-stream"` for unknown or missing extensions.
pub(crate) fn guess_mime(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let mime = match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "heic" => "image/heic",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    };
    mime.to_string()
}

/// Lowercase + trim, so `"Image/PNG "` and `"image/png"` map alike.
fn normalize(mime: &str) -> String {
    mime.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_uttype_known_and_wildcards() {
        assert_eq!(apple_uttype("application/pdf"), "com.adobe.pdf");
        assert_eq!(apple_uttype("image/*"), "public.image");
        assert_eq!(apple_uttype("video/*"), "public.movie");
        // Case/whitespace-insensitive.
        assert_eq!(apple_uttype("  Image/PNG "), "public.png");
    }

    #[test]
    fn apple_uttype_unknown_falls_back_to_any() {
        assert_eq!(apple_uttype("application/x-made-up"), "public.item");
        assert_eq!(apple_uttype(""), "public.item");
        assert_eq!(apple_uttype("*/*"), "public.item");
    }

    #[test]
    fn win_filter_specific_and_wildcard() {
        assert_eq!(win_filter("application/pdf"), Some(("PDF", "*.pdf")));
        assert_eq!(
            win_filter("image/*"),
            Some(("Images", "*.png;*.jpg;*.jpeg;*.gif;*.bmp;*.webp;*.heic"))
        );
    }

    #[test]
    fn win_filter_any_is_none() {
        assert_eq!(win_filter(""), None);
        assert_eq!(win_filter("*/*"), None);
        assert_eq!(win_filter("application/octet-stream"), None);
    }

    #[test]
    fn guess_mime_from_extension() {
        assert_eq!(guess_mime("report.PDF"), "application/pdf");
        assert_eq!(guess_mime("clip.mp4"), "video/mp4");
        assert_eq!(guess_mime("photo.jpeg"), "image/jpeg");
        assert_eq!(guess_mime("noext"), "application/octet-stream");
        assert_eq!(guess_mime("weird.xyz"), "application/octet-stream");
    }

    #[test]
    fn media_mimes_mapping() {
        assert_eq!(media_mimes(MediaKind::Images), &["image/*"]);
        assert_eq!(media_mimes(MediaKind::Videos), &["video/*"]);
        assert_eq!(
            media_mimes(MediaKind::ImagesAndVideos),
            &["image/*", "video/*"]
        );
    }
}
