//! Static asset surface: fonts, images, audio, video, blobs.
//!
//! Assets are declared once at compile time via [`asset!`] (or, for
//! fonts, [`typeface!`] + [`face!`]). The macros mint a stable
//! [`AssetId`] from the asset's logical path, hashed under the calling
//! crate's name so paths can collide across crates without colliding
//! ids.
//!
//! For images / audio / video / blobs, [`asset!`] records a
//! `Bundled` path the build tool / per-backend resolver is
//! responsible for getting in front of the platform.
//! Fonts carry a `Bundled` path too, so a backend can fetch the font
//! file by URL (the web `@font-face { src: url(...) }` path). Whether
//! [`face!`] *also* embeds the file's bytes via `include_bytes!` is
//! decided by the `embed-font-bytes` cargo feature on this crate:
//!
//! - **feature off** (e.g. a pure web/DOM build): `face!` emits
//!   [`AssetSource::Bundled`] — path only, no bytes in the binary. The
//!   web backend links the font as a separately-fetched file.
//! - **feature on** (any build that links a byte-consuming backend —
//!   cosmic-text/wgpu, CoreText, Android): `face!` emits
//!   [`AssetSource::BundledEmbedded`], which carries the bytes *and*
//!   the path. Native/wgpu use the bytes; web still prefers the path.
//!
//! The feature is flipped by the dep graph, not the author: each
//! byte-consuming backend enables `runtime-core/embed-font-bytes`
//! through its own dep. URL-loaded fonts (arbitrary remote `Remote`
//! sources) are still intentionally unsupported for fonts — only
//! project-shipped font files are valid.
//!
//! At render time the framework calls [`Backend::register_asset`] /
//! [`Backend::register_typeface`] the first time an asset id is
//! observed. Each backend decides what registration means for it:
//!
//! - **Web**: a font becomes a `@font-face` rule and an `@import`-able
//!   URL; an image becomes a URL the resolver hands back to `<img>`.
//! - **iOS**: a font registers via `CTFontManagerRegisterFontsForURL`;
//!   an image becomes a `UIImage(named:)` lookup against the .app bundle.
//! - **Android**: assets land in `AssetManager` and become `Typeface` /
//!   `Bitmap` lookups.
//! - **wgpu / native renderer**: bytes go to cosmic-text /
//!   texture-upload paths.
//! - **runtime-server / wire**: bytes ship over the wire on first reference and
//!   live in the [`SceneModel`](crate::backend) snapshot.
//!
//! The `Asset<K>` handle is generic over a marker type so the type
//! system rejects passing an image where a font is expected. Marker
//! types live in the [`kinds`] submodule.
//!
//! [`Backend::register_asset`]: crate::Backend::register_asset
//! [`Backend::register_typeface`]: crate::Backend::register_typeface

use std::marker::PhantomData;

use crate::style::{FontStyle, FontWeight};

// ---------------------------------------------------------------------------
// IDs
// ---------------------------------------------------------------------------

/// Stable identity for a registered asset. Minted by the [`asset!`]
/// macro (or `face!` for typeface faces) from a const-hash of the
/// crate name + logical path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssetId(pub u64);

/// Stable identity for a registered typeface (font family with a set
/// of weight/style faces). Minted by [`typeface!`] from a const-hash
/// of the crate name + family name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypefaceId(pub u64);

// ---------------------------------------------------------------------------
// Asset kinds — type-level markers + a runtime tag
// ---------------------------------------------------------------------------

/// Runtime tag matching one of the [`kinds`] marker types. Passed to
/// the backend alongside [`AssetSource`] so a single
/// [`Backend::register_asset`](crate::Backend::register_asset) entry
/// point can route by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetTag {
    Font,
    Image,
    Audio,
    Video,
    Blob,
}

/// Sealed-by-convention trait that ties an [`AssetTag`] to a
/// compile-time marker type. Implementors live in [`kinds`].
pub trait AssetKind: 'static {
    const TAG: AssetTag;
}

/// Marker types for [`Asset<K>`]. Use in turbofish position or rely on
/// inference at the call site:
///
/// ```ignore
/// use runtime_core::assets::{kinds::Image, Asset};
/// static LOGO: Asset<Image> = asset!("images/logo.png");
/// ```
pub mod kinds {
    use super::{AssetKind, AssetTag};

    pub struct Font;
    impl AssetKind for Font {
        const TAG: AssetTag = AssetTag::Font;
    }

    pub struct Image;
    impl AssetKind for Image {
        const TAG: AssetTag = AssetTag::Image;
    }

    pub struct Audio;
    impl AssetKind for Audio {
        const TAG: AssetTag = AssetTag::Audio;
    }

    pub struct Video;
    impl AssetKind for Video {
        const TAG: AssetTag = AssetTag::Video;
    }

    pub struct Blob;
    impl AssetKind for Blob {
        const TAG: AssetTag = AssetTag::Blob;
    }
}

// ---------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------

/// Where the bytes come from. The backend's
/// [`register_asset`](crate::Backend::register_asset) implementation
/// decides what to do with each variant — fetch over the network,
/// register with the OS, hand to a GPU uploader, etc.
#[derive(Debug, Clone, Copy)]
pub enum AssetSource {
    /// Bytes are baked into the binary via `include_bytes!`. The
    /// backend can use them directly. Appropriate for small assets
    /// (icon SVGs, fallback fonts).
    Embedded {
        bytes: &'static [u8],
        /// Lowercase file extension without leading dot (`"ttf"`,
        /// `"png"`, `"woff2"`). Backends use this to pick the right
        /// content-type / `@font-face` `format()` / decoder.
        extension: &'static str,
    },
    /// Logical path the build tool resolves to a per-backend location
    /// before the binary runs. The framework does not read the file —
    /// it just forwards the path to the backend.
    Bundled { path: &'static str },
    /// Both a bundle-relative `path` (for backends that fetch the file
    /// by URL — the web backend's `@font-face { src: url(...) }`) and
    /// the file's `bytes` embedded at compile time (for backends that
    /// consume bytes directly — cosmic-text/wgpu, CoreText, Android).
    ///
    /// Emitted by [`face!`] when the `embed-font-bytes` feature is on.
    /// Each backend picks the half it needs: web ignores `bytes` and
    /// links the `path`; native/wgpu ignore `path` and load `bytes`.
    BundledEmbedded {
        path: &'static str,
        bytes: &'static [u8],
        /// Lowercase extension without leading dot — same role as on
        /// [`AssetSource::Embedded`].
        extension: &'static str,
    },
    /// Raw URL. Backend fetches at runtime. Escape hatch for
    /// CDN-hosted or user-supplied assets.
    Remote { url: &'static str },
}

// ---------------------------------------------------------------------------
// Asset<K>
// ---------------------------------------------------------------------------

/// A type-safe handle to a single registered asset. Construct via
/// [`asset!`]; the macro infers the kind from the surrounding context.
///
/// The handle is `Copy` and zero-runtime-cost — the framework reads
/// `id` to dedupe registration and the backend reads `source` once on
/// first observation.
pub struct Asset<K: AssetKind> {
    pub id: AssetId,
    pub source: AssetSource,
    pub tag: AssetTag,
    _kind: PhantomData<fn() -> K>,
}

impl<K: AssetKind> Asset<K> {
    /// Internal constructor — call sites should use the [`asset!`]
    /// macro, which mints a stable id from the call's path literal.
    #[doc(hidden)]
    pub const fn new(id: AssetId, source: AssetSource) -> Self {
        Self {
            id,
            source,
            tag: K::TAG,
            _kind: PhantomData,
        }
    }
}

impl<K: AssetKind> Clone for Asset<K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K: AssetKind> Copy for Asset<K> {}

impl<K: AssetKind> std::fmt::Debug for Asset<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Asset")
            .field("id", &self.id)
            .field("tag", &self.tag)
            .field("source", &self.source)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Typeface — a font family with a set of (weight, style) faces
// ---------------------------------------------------------------------------

/// A font family declaration. Built via [`typeface!`] and stored in a
/// `static`/`const` — the backend reads `family_name` + `faces` once
/// at registration and never again.
#[derive(Debug, Clone, Copy)]
pub struct Typeface {
    pub id: TypefaceId,
    /// Authoritative family name. Backends with a native font registry
    /// (web's `font-family`, iOS `UIFont(name:)`) use this as the
    /// post-registration lookup key.
    pub family_name: &'static str,
    pub faces: &'static [TypefaceFace],
    /// What the system should fall back to when none of the [`faces`]
    /// can be resolved (load failure, asset missing). Sized fallback
    /// keyed by generic role, not a specific family.
    ///
    /// [`faces`]: Self::faces
    pub fallback: SystemFallback,
}

/// One face within a [`Typeface`]: a specific weight/style combination
/// backed by a single font asset.
#[derive(Debug, Clone, Copy)]
pub struct TypefaceFace {
    pub weight: FontWeight,
    pub style: FontStyle,
    pub asset: AssetId,
    pub source: AssetSource,
}

/// Generic font role used when a typeface fails to resolve. Maps to
/// the OS's default for that role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemFallback {
    Serif,
    SansSerif,
    Monospace,
    /// No fallback — render as the platform-default UI font.
    None,
}

// ---------------------------------------------------------------------------
// Const hashing — FNV-1a 64
// ---------------------------------------------------------------------------

/// Const-evaluable FNV-1a 64 over arbitrary bytes. Used by the asset
/// macros to mint stable ids from `crate_name + path`. The choice of
/// hash is unimportant — we only need determinism and a low collision
/// rate within a single binary.
#[doc(hidden)]
pub const fn const_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

/// Extract the lowercase extension from a path literal (the bytes
/// after the last `.`). Returns `""` if no `.` is present.
#[doc(hidden)]
pub const fn extension_from_path(path: &'static str) -> &'static str {
    let bytes = path.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        if bytes[i] == b'.' {
            // SAFETY: we found a '.' at byte `i`; the suffix from
            // `i+1` onwards is a valid utf-8 slice because `path` is.
            let (_head, tail) = path.split_at(i + 1);
            return tail;
        }
        if bytes[i] == b'/' {
            break;
        }
    }
    ""
}

/// Normalize a `face!`/`include_bytes!` source path (relative to the
/// calling `.rs` file) into a bundle-root-relative path suitable for a
/// served-file URL. Strips leading `./` and `../` segments so a path
/// like `"../fonts/Inter-Regular.ttf"` (written from `src/typeface.rs`)
/// becomes `"fonts/Inter-Regular.ttf"`.
///
/// The convention this assumes: font files live in a top-level project
/// directory (`fonts/`, `assets/`, …) that the web build stages into
/// the deployed bundle, and the `src:` literal walks up out of `src/`
/// to reach it. The web backend turns the result into a root-absolute
/// URL (`/fonts/Inter-Regular.ttf`).
#[doc(hidden)]
pub const fn bundle_path_from_src(path: &'static str) -> &'static str {
    let bytes = path.as_bytes();
    let mut i = 0;
    loop {
        // Strip a leading "./"
        if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1] == b'/' {
            i += 2;
            continue;
        }
        // Strip a leading "../"
        if i + 2 < bytes.len() && bytes[i] == b'.' && bytes[i + 1] == b'.' && bytes[i + 2] == b'/' {
            i += 3;
            continue;
        }
        break;
    }
    // SAFETY-equivalent: `i` only ever advances past whole ASCII
    // `./` / `../` prefixes, which are char boundaries, so the tail is
    // a valid utf-8 slice of `path`.
    let (_head, tail) = path.split_at(i);
    tail
}

// ---------------------------------------------------------------------------
// Macros
// ---------------------------------------------------------------------------

/// Declare a bundled asset. Returns an [`Asset<K>`] where `K` is
/// inferred from the surrounding context.
///
/// ```ignore
/// use runtime_core::assets::{kinds::Image, Asset};
///
/// static LOGO: Asset<Image> = asset!("images/logo.png");
/// ```
///
/// The id is `const_hash(crate_name + "::" + path)`, so two crates
/// referencing the same path get distinct ids. The build tool is
/// responsible for copying the file into a per-backend location at
/// build time.
#[macro_export]
macro_rules! asset {
    ($path:literal) => {{
        const SOURCE: $crate::assets::AssetSource = $crate::assets::AssetSource::Bundled {
            path: $path,
        };
        const ID: $crate::assets::AssetId = $crate::assets::AssetId(
            $crate::assets::const_hash(
                concat!(env!("CARGO_PKG_NAME"), "::", $path).as_bytes(),
            ),
        );
        $crate::assets::Asset::new(ID, SOURCE)
    }};
}

/// Declare an asset and embed its bytes into the binary at compile
/// time via `include_bytes!`. Unlike [`asset!`] (which only carries
/// the *path* and leaves resolution to the build tool / backend),
/// `embed_asset!` reads the file at compile time so the asset is
/// fully self-contained.
///
/// Path resolution follows `include_bytes!` rules — it's relative to
/// the calling source file. The extension is derived from the path
/// and used by backends to set a content-type / `format()` hint.
///
/// ```ignore
/// use runtime_core::embed_asset;
/// use runtime_core::assets::{kinds::Image, Asset};
///
/// // Path is relative to *this* file. `../assets/logo.svg` resolves
/// // against the calling source file's directory.
/// static LOGO: Asset<Image> = embed_asset!("../assets/logo.svg");
/// ```
///
/// Trade-offs vs [`asset!`]: embedding bloats the binary but avoids
/// the build-tool step that otherwise has to copy the file into each
/// platform's bundle. Right for small images, icon packs, fallback
/// fonts; wrong for anything big enough to matter on download.
#[macro_export]
macro_rules! embed_asset {
    ($path:literal) => {{
        const BYTES: &[u8] = include_bytes!($path);
        const EXTENSION: &str = $crate::assets::extension_from_path($path);
        const SOURCE: $crate::assets::AssetSource = $crate::assets::AssetSource::Embedded {
            bytes: BYTES,
            extension: EXTENSION,
        };
        const ID: $crate::assets::AssetId = $crate::assets::AssetId(
            $crate::assets::const_hash(
                concat!(env!("CARGO_PKG_NAME"), "::embed::", $path).as_bytes(),
            ),
        );
        $crate::assets::Asset::new(ID, SOURCE)
    }};
}

/// Declare a typeface (font family) with one or more weight/style
/// faces. Each face is declared with [`face!`].
///
/// ```ignore
/// use runtime_core::assets::SystemFallback;
/// use runtime_core::style::{FontStyle, FontWeight};
///
/// static INTER: runtime_core::assets::Typeface = typeface! {
///     name: "Inter",
///     faces: [
///         face!(weight: FontWeight::Normal, style: FontStyle::Normal,
///               src: "fonts/Inter-Regular.ttf"),
///         face!(weight: FontWeight::Bold, style: FontStyle::Normal,
///               src: "fonts/Inter-Bold.ttf"),
///     ],
///     fallback: SystemFallback::SansSerif,
/// };
/// ```
#[macro_export]
macro_rules! typeface {
    (
        name: $name:literal,
        faces: [ $($face:expr),* $(,)? ],
        fallback: $fallback:expr $(,)?
    ) => {{
        const FACES: &[$crate::assets::TypefaceFace] = &[ $($face),* ];
        const ID: $crate::assets::TypefaceId = $crate::assets::TypefaceId(
            $crate::assets::const_hash(
                concat!(env!("CARGO_PKG_NAME"), "::typeface::", $name).as_bytes(),
            ),
        );
        $crate::assets::Typeface {
            id: ID,
            family_name: $name,
            faces: FACES,
            fallback: $fallback,
        }
    }};
}

/// Declare one face within a [`typeface!`] block. Takes a weight, a
/// style, and a path literal (relative to the calling source file, per
/// `include_bytes!` rules).
///
/// Whether the file's bytes are embedded depends on the
/// `embed-font-bytes` feature on `runtime-core` (see the module docs):
///
/// - **feature on** → [`AssetSource::BundledEmbedded`]: bytes are
///   `include_bytes!`-baked into the binary *and* a bundle path is
///   recorded. Native/wgpu backends load the bytes; web links the path.
/// - **feature off** → [`AssetSource::Bundled`]: only the bundle path
///   is recorded, no bytes. The web backend links the font as a
///   separately-fetched file. Nothing reads the file at compile time.
///
/// The feature is flipped by which backends are in the build, not by
/// the author — see [`crate::__face_source`]. URL-loaded fonts are
/// intentionally unsupported; only project-shipped files are valid.
///
/// The asset id is `const_hash(crate_name + "::" + path)`, the same
/// scheme [`asset!`] uses, so two crates referencing the same path
/// produce distinct ids.
#[macro_export]
macro_rules! face {
    (weight: $w:expr, style: $s:expr, src: $path:literal $(,)?) => {{
        $crate::assets::TypefaceFace {
            weight: $w,
            style: $s,
            asset: $crate::assets::AssetId(
                $crate::assets::const_hash(
                    concat!(env!("CARGO_PKG_NAME"), "::", $path).as_bytes(),
                ),
            ),
            source: $crate::__face_source!($path),
        }
    }};
}

/// Internal: build the [`AssetSource`] for a [`face!`]. Two
/// `#[cfg]`-split definitions live here in `runtime-core` so the
/// embed/no-embed decision is made against *this crate's* unified
/// feature set (the dep graph), not the author crate's features —
/// `#[cfg(feature = ...)]` inside the `face!` expansion would resolve
/// against the author crate, which never declares the feature.
///
/// The `include_bytes!($path)` still resolves relative to the author's
/// invocation site (macro_rules paths are call-site-relative), so the
/// `src:` literal in `face!` keeps its `include_bytes!` semantics.
#[cfg(feature = "embed-font-bytes")]
#[doc(hidden)]
#[macro_export]
macro_rules! __face_source {
    ($path:literal) => {
        $crate::assets::AssetSource::BundledEmbedded {
            path: $crate::assets::bundle_path_from_src($path),
            bytes: include_bytes!($path),
            extension: $crate::assets::extension_from_path($path),
        }
    };
}

#[cfg(not(feature = "embed-font-bytes"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __face_source {
    ($path:literal) => {
        $crate::assets::AssetSource::Bundled {
            path: $crate::assets::bundle_path_from_src($path),
        }
    };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_from_path_basic() {
        assert_eq!(extension_from_path("fonts/Inter-Regular.ttf"), "ttf");
        assert_eq!(extension_from_path("logo.png"), "png");
        assert_eq!(extension_from_path("nested/dir.with.dot/file.woff2"), "woff2");
        assert_eq!(extension_from_path("no-extension"), "");
        assert_eq!(extension_from_path("dir.with.dot/no-ext"), "");
    }

    #[test]
    fn const_hash_is_deterministic_and_path_sensitive() {
        let a = const_hash(b"my-crate::fonts/Inter-Regular.ttf");
        let b = const_hash(b"my-crate::fonts/Inter-Regular.ttf");
        let c = const_hash(b"my-crate::fonts/Inter-Bold.ttf");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn asset_macro_mints_stable_id() {
        let a: Asset<kinds::Image> = asset!("images/logo.png");
        let b: Asset<kinds::Image> = asset!("images/logo.png");
        let c: Asset<kinds::Image> = asset!("images/other.png");
        assert_eq!(a.id, b.id);
        assert_ne!(a.id, c.id);
        assert_eq!(a.tag, AssetTag::Image);
    }

    #[test]
    fn typeface_macro_mints_face_ids_from_paths() {
        // Uses this file (`assets.rs`) and `lib.rs` as the `src:`
        // targets: under the `embed-font-bytes` feature `face!` calls
        // `include_bytes!`, which needs a real file at the literal
        // path; without the feature only the path string is recorded.
        // Either way the two distinct paths produce distinct
        // const-hash-derived asset ids.
        let tf: Typeface = typeface! {
            name: "Inter",
            faces: [
                face!(weight: FontWeight::Normal, style: FontStyle::Normal,
                      src: "assets.rs"),
                face!(weight: FontWeight::Bold, style: FontStyle::Normal,
                      src: "lib.rs"),
            ],
            fallback: SystemFallback::SansSerif,
        };
        assert_eq!(tf.family_name, "Inter");
        assert_eq!(tf.faces.len(), 2);
        assert_eq!(tf.faces[0].weight, FontWeight::Normal);
        assert_eq!(tf.faces[1].weight, FontWeight::Bold);
        assert_ne!(tf.faces[0].asset, tf.faces[1].asset);
    }

    #[test]
    fn asset_kind_tag_matches_marker_type() {
        let f: Asset<kinds::Font> = asset!("fonts/X-Regular.ttf");
        let i: Asset<kinds::Image> = asset!("images/x.png");
        let a: Asset<kinds::Audio> = asset!("audio/x.mp3");
        let v: Asset<kinds::Video> = asset!("video/x.mp4");
        let b: Asset<kinds::Blob> = asset!("data/x.bin");
        assert_eq!(f.tag, AssetTag::Font);
        assert_eq!(i.tag, AssetTag::Image);
        assert_eq!(a.tag, AssetTag::Audio);
        assert_eq!(v.tag, AssetTag::Video);
        assert_eq!(b.tag, AssetTag::Blob);
    }

    #[test]
    fn asset_macro_emits_bundled_source_with_literal_path() {
        let a: Asset<kinds::Image> = asset!("dir/sub/logo.png");
        match a.source {
            AssetSource::Bundled { path } => assert_eq!(path, "dir/sub/logo.png"),
            other => panic!("expected Bundled, got {:?}", other),
        }
    }

    #[test]
    fn bundle_path_from_src_strips_relative_prefixes() {
        // The web backend turns these into root-absolute served-file
        // URLs, so the leading `./` / `../` that `include_bytes!` needs
        // (to climb out of `src/`) must be stripped to a bundle-root
        // path.
        assert_eq!(bundle_path_from_src("../fonts/Inter-Regular.ttf"), "fonts/Inter-Regular.ttf");
        assert_eq!(bundle_path_from_src("../../fonts/Inter-Bold.ttf"), "fonts/Inter-Bold.ttf");
        assert_eq!(bundle_path_from_src("./fonts/x.ttf"), "fonts/x.ttf");
        assert_eq!(bundle_path_from_src("../assets/fonts/x.woff2"), "assets/fonts/x.woff2");
        // Already bundle-relative — unchanged.
        assert_eq!(bundle_path_from_src("fonts/x.ttf"), "fonts/x.ttf");
        // A `..` that isn't a leading path segment is left alone.
        assert_eq!(bundle_path_from_src("fonts/a..b.ttf"), "fonts/a..b.ttf");
    }

    #[test]
    fn typeface_face_source_records_bundle_path() {
        // `face!` always records the bundle path (for the web backend
        // to link the file). The `src:` literal climbs out of the
        // (hypothetical) `src/` dir, so the recorded path is
        // normalized to bundle-root.
        let tf: Typeface = typeface! {
            name: "Mono",
            faces: [
                face!(weight: FontWeight::Normal, style: FontStyle::Italic,
                      src: "assets.rs"),
            ],
            fallback: SystemFallback::Monospace,
        };
        let face = &tf.faces[0];
        assert_eq!(face.weight, FontWeight::Normal);
        assert_eq!(face.style, FontStyle::Italic);
        match face.source {
            // `embed-font-bytes` ON: carries both the path and the
            // embedded bytes (this file, used as the embed target).
            AssetSource::BundledEmbedded { path, bytes, extension } => {
                assert_eq!(path, "assets.rs");
                assert!(!bytes.is_empty());
                assert_eq!(extension, "rs");
            }
            // `embed-font-bytes` OFF (runtime-core's default test
            // build): path only, no bytes baked into the binary.
            AssetSource::Bundled { path } => {
                assert_eq!(path, "assets.rs");
            }
            other => panic!("expected Bundled/BundledEmbedded, got {:?}", other),
        }
        assert_eq!(tf.fallback, SystemFallback::Monospace);
    }

    #[test]
    #[cfg(not(feature = "embed-font-bytes"))]
    fn face_emits_bytes_free_bundled_when_feature_off() {
        // The pure-web path: no font bytes in the binary, just a path
        // the web backend links via `@font-face { src: url(...) }`.
        let tf: Typeface = typeface! {
            name: "Inter",
            faces: [
                face!(weight: FontWeight::Normal, style: FontStyle::Normal,
                      src: "../fonts/Inter-Regular.ttf"),
            ],
            fallback: SystemFallback::SansSerif,
        };
        match tf.faces[0].source {
            AssetSource::Bundled { path } => assert_eq!(path, "fonts/Inter-Regular.ttf"),
            other => panic!("expected Bundled (no bytes), got {:?}", other),
        }
    }

    #[test]
    #[cfg(feature = "embed-font-bytes")]
    fn face_embeds_bytes_and_path_when_feature_on() {
        // Byte-consuming-backend path: bytes baked in for
        // cosmic-text/CoreText/Android, path kept for web linking.
        let tf: Typeface = typeface! {
            name: "Mono",
            faces: [
                face!(weight: FontWeight::Normal, style: FontStyle::Normal,
                      src: "assets.rs"),
            ],
            fallback: SystemFallback::Monospace,
        };
        match tf.faces[0].source {
            AssetSource::BundledEmbedded { path, bytes, extension } => {
                assert_eq!(path, "assets.rs");
                assert!(!bytes.is_empty());
                assert_eq!(extension, "rs");
            }
            other => panic!("expected BundledEmbedded, got {:?}", other),
        }
    }

    #[test]
    fn same_path_different_kinds_collide_on_id() {
        // Two asset! calls with the same path produce the same id
        // regardless of K — the id is content-derived, not type-
        // discriminated. Backends use `tag` to route, not the id, so
        // this is fine in practice.
        let a: Asset<kinds::Image> = asset!("collide/same.bin");
        let b: Asset<kinds::Blob> = asset!("collide/same.bin");
        assert_eq!(a.id, b.id);
        assert_ne!(a.tag, b.tag);
    }

    #[test]
    fn embed_asset_emits_embedded_source_with_bytes() {
        // Embed this very file — guaranteed to exist at build time
        // and contain bytes, regardless of where the workspace lives.
        let a: Asset<kinds::Image> = embed_asset!("assets.rs");
        match a.source {
            AssetSource::Embedded { bytes, extension } => {
                assert!(!bytes.is_empty());
                assert_eq!(extension, "rs");
            }
            other => panic!("expected Embedded, got {:?}", other),
        }
        assert_eq!(a.tag, AssetTag::Image);
    }

    #[test]
    fn embed_asset_id_namespaced_separately_from_bundled() {
        // Same path string, different macros — different ids so an
        // `asset!("x")` and `embed_asset!("x")` registration are
        // tracked as distinct backend entries.
        let bundled: Asset<kinds::Blob> = asset!("assets.rs");
        let embedded: Asset<kinds::Blob> = embed_asset!("assets.rs");
        assert_ne!(bundled.id, embedded.id);
    }

    #[test]
    fn typeface_id_uses_family_name_not_face_paths() {
        let a: Typeface = typeface! {
            name: "FamilyA",
            faces: [
                face!(weight: FontWeight::Normal, style: FontStyle::Normal,
                      src: "assets.rs"),
            ],
            fallback: SystemFallback::None,
        };
        let b: Typeface = typeface! {
            name: "FamilyA",
            faces: [
                // Different embed path, same family name — same id.
                face!(weight: FontWeight::Bold, style: FontStyle::Normal,
                      src: "lib.rs"),
            ],
            fallback: SystemFallback::None,
        };
        let c: Typeface = typeface! {
            name: "FamilyB",
            faces: [
                face!(weight: FontWeight::Normal, style: FontStyle::Normal,
                      src: "assets.rs"),
            ],
            fallback: SystemFallback::None,
        };
        assert_eq!(a.id, b.id, "same family name → same id");
        assert_ne!(a.id, c.id, "different family name → different id");
    }
}
