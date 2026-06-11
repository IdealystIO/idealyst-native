//! Hand-curated registration table for [`SdkEntry`] — the opt-in crates
//! under `crates/sdk/*`, `crates/api/*`, and `crates/ui/*` that ship
//! outside `runtime-core`.
//!
//! Same lock pattern as `primitives.rs` / `macros.rs`: `SdkEntry` carries
//! a private `_seal: ()` so only this crate constructs one. Every entry
//! names a crate that actually exists in the workspace; the prose home
//! for the roster is the `sdks` guide (`guides/sdks.md`), and the
//! `#[server]` flow has its own [[server-functions]] guide. The drift
//! audit (`.claude/audits/mcp-catalog-drift.md`) checks this table
//! against the `crates/{sdk,api,ui}/*` directory listing, so adding or
//! renaming a crate means updating this file in the same change.
//!
//! `dep_line` is a copy-pasteable `Cargo.toml` line. We use the
//! `{ workspace = true }` form (correct inside the workspace and for
//! examples); an external project mirrors its `runtime-core` git/rev/path
//! source — the `sdks` guide spells that out.

use crate::{SdkCategory, SdkEntry, SdkKind};

macro_rules! sdk {
    ($name:literal, $cat:expr, $kind:expr, $summary:literal) => {
        inventory::submit! {
            SdkEntry {
                name: $name,
                summary: $summary,
                dep_line: concat!($name, " = { workspace = true }"),
                category: $cat,
                kind: $kind,
                guide: "sdks",
                _seal: (),
            }
        }
    };
}

// ---------------------------------------------------------------------
// Data — networking, persistence, server relay, i18n
// ---------------------------------------------------------------------

sdk!(
    "net",
    SdkCategory::Data,
    SdkKind::Api,
    "Cross-platform async networking — HTTP, WebSocket, and Server-Sent Events. `net::Client` is the HTTP entry point; the transport the server-functions layer composes."
);
sdk!(
    "server",
    SdkCategory::Data,
    SdkKind::Api,
    "Full-stack server functions: `#[server] async fn`, `server::configure`, `server::router`, request extractors, auth guards. See the [[server-functions]] guide."
);
sdk!(
    "storage",
    SdkCategory::Data,
    SdkKind::Api,
    "Cross-platform INSECURE key-value storage for non-sensitive app data. `storage::platform_storage()` returns the platform store. No security claims — use `credentials` for secrets."
);
sdk!(
    "credentials",
    SdkCategory::Data,
    SdkKind::Api,
    "Cross-platform SECURE storage for secrets (auth tokens, API keys) — Keychain / Keystore on device. Web errors rather than faking security."
);
sdk!(
    "files",
    SdkCategory::Data,
    SdkKind::Api,
    "Cross-platform blob/file storage for binary data — recordings, downloads."
);
sdk!(
    "file-export",
    SdkCategory::Data,
    SdkKind::Api,
    "Save a file to a user-chosen location through the platform's native save UI (no permission prompt)."
);
sdk!(
    "file-picker",
    SdkCategory::Data,
    SdkKind::Api,
    "Inverse of file-export: let the user pick local file(s) via the native picker. Yields a lazily-streamed `PickedFile` (path / open-chunk / copy_to) — never reads the whole file into RAM. Documents vs Media (dedicated mobile photo picker)."
);
sdk!(
    "i18n",
    SdkCategory::Data,
    SdkKind::Api,
    "Lightweight, Rust-native internationalization — runtime half."
);

// ---------------------------------------------------------------------
// Media — capture, playback, drawing
// ---------------------------------------------------------------------

sdk!(
    "media-stream",
    SdkCategory::Media,
    SdkKind::Api,
    "A platform-agnostic handle to a live video source — the common abstraction camera / screen-recorder yield."
);
sdk!(
    "camera",
    SdkCategory::Media,
    SdkKind::Api,
    "Cross-platform camera capture → a `MediaStream`."
);
sdk!(
    "microphone",
    SdkCategory::Media,
    SdkKind::Api,
    "Cross-platform microphone capture → an audio stream."
);
sdk!(
    "screen-recorder",
    SdkCategory::Media,
    SdkKind::Api,
    "Cross-platform screen / window recording → a `MediaStream`."
);
sdk!(
    "media-writer",
    SdkCategory::Media,
    SdkKind::Api,
    "Record live media streams to a file (mp4)."
);
sdk!(
    "canvas",
    SdkCategory::Media,
    SdkKind::Api,
    "Author-facing facade for the 2D-drawing SDK (GPU canvas + self-capture compositor)."
);
sdk!(
    "video",
    SdkCategory::Media,
    SdkKind::External,
    "Third-party `Video` playback primitive (`Element::External`)."
);
sdk!(
    "video-decode",
    SdkCategory::Media,
    SdkKind::Api,
    "Decode a video file into frames — the file-decoder peer of `camera` / `screen-recorder`."
);

// ---------------------------------------------------------------------
// UI — component library + Element::External primitives
// ---------------------------------------------------------------------

sdk!(
    "idea-ui",
    SdkCategory::Ui,
    SdkKind::External,
    "The cross-platform component library — `Button`, `Card`, `Field`, `Select`, etc. Its `#[component]`s surface in `list_components` once linked."
);
sdk!(
    "idea-theme",
    SdkCategory::Ui,
    SdkKind::Api,
    "Theming abstraction + extensibility for the idealyst design system."
);
sdk!(
    "icons-lucide",
    SdkCategory::Ui,
    SdkKind::Api,
    "Lucide icon pack — only icons you reference end up in the binary."
);
sdk!(
    "webview",
    SdkCategory::Ui,
    SdkKind::External,
    "Third-party `WebView` primitive. The canonical single-crate cfg-gated `Element::External` pattern."
);
sdk!(
    "maps",
    SdkCategory::Ui,
    SdkKind::External,
    "Third-party `MapView` primitive."
);
sdk!(
    "svg",
    SdkCategory::Ui,
    SdkKind::External,
    "Third-party SVG renderer."
);
sdk!(
    "markdown",
    SdkCategory::Ui,
    SdkKind::External,
    "CommonMark/GFM document primitive."
);
sdk!(
    "codeblock",
    SdkCategory::Ui,
    SdkKind::External,
    "Read-only colored-text (code) panel primitive."
);
sdk!(
    "table",
    SdkCategory::Ui,
    SdkKind::External,
    "Cross-platform table — a real `<table>` on web."
);
sdk!(
    "form",
    SdkCategory::Ui,
    SdkKind::Api,
    "Third-party `Form` SDK."
);
sdk!(
    "toolbar",
    SdkCategory::Ui,
    SdkKind::External,
    "Third-party `Toolbar` SDK."
);
sdk!(
    "menu",
    SdkCategory::Ui,
    SdkKind::Api,
    "OS-level menu-bar SDK (desktop)."
);

// ---------------------------------------------------------------------
// Navigation — render navigators (`Element::Navigator`)
// ---------------------------------------------------------------------

sdk!(
    "drawer-navigator",
    SdkCategory::Ui,
    SdkKind::Api,
    "Side-drawer navigator — a responsive sidebar/modal drawer over screens. Renders `Element::Navigator`."
);
sdk!(
    "stack-navigator",
    SdkCategory::Ui,
    SdkKind::Api,
    "Push/pop stack navigator with native screen transitions and a back stack."
);
sdk!(
    "tab-navigator",
    SdkCategory::Ui,
    SdkKind::Api,
    "Tab-bar navigator — top-level sibling screens selected by a tab bar."
);

// ---------------------------------------------------------------------
// Device — input gestures + device capabilities
// ---------------------------------------------------------------------

sdk!(
    "pan",
    SdkCategory::Device,
    SdkKind::Api,
    "Pan-gesture SDK — a reactive value handle tracking drag offset for author-level pan interactions."
);
sdk!(
    "zoom",
    SdkCategory::Device,
    SdkKind::Api,
    "Zoom-gesture SDK — reactive scale from a pinch recognizer (touch) plus a wheel/magnify channel (web `wheel`+ctrlKey / macOS `magnify:`)."
);
sdk!(
    "biometrics",
    SdkCategory::Device,
    SdkKind::Api,
    "Cross-platform biometric authentication — prove the device owner is present."
);
