//! Custom font registration & resolution for the Android backend.
//!
//! Flow mirrors the iOS path:
//!
//! 1. `register_asset(id, AssetTag::Font, source)` writes the embedded
//!    bytes to a file under `Context.cacheDir/idealyst-fonts/`, then
//!    builds an Android `Typeface` from the file via
//!    `Typeface.createFromFile(String)`. The resulting `Typeface` is
//!    kept as a `GlobalRef` keyed by `AssetId`.
//! 2. `register_typeface(id, family_name, faces, fallback)` walks the
//!    face list, looks each face's `AssetId` up in the per-asset
//!    Typeface cache, and stores the result keyed by `TypefaceId`.
//! 3. At style-apply time, `resolve_typeface_ref` walks the registry to
//!    pick the best-matching face for the requested `(weight, style)`
//!    and returns a `&GlobalRef` the caller hands to
//!    `TextView.setTypeface`.
//!
//! `Typeface.Builder` accepts a `File`/`FileDescriptor`/asset path
//! but no direct byte-array overload until API 26, and writing the
//! bytes to the app's cache dir is the smallest cross-API path that
//! still uses public APIs.

use std::collections::HashMap;

use runtime_core::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, Typeface, TypefaceFace, TypefaceId,
};
use runtime_core::{FontFamily, FontStyle, FontWeight};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;

/// One registered face: weight + style + the GlobalRef holding the
/// Android `Typeface` we resolved for that face.
#[derive(Clone)]
struct RegisteredFace {
    weight: FontWeight,
    style: FontStyle,
    typeface: GlobalRef,
}

/// One registered typeface: the framework's family + every face we
/// successfully decoded. `_family_name` is recorded for debugging
/// and to allow future `Typeface.create(name, style)` lookups if a
/// custom face fails — the framework's `Typeface.fallback` covers
/// the system fallback path so we don't need to store it here.
#[derive(Clone)]
struct RegisteredTypeface {
    _family_name: String,
    faces: Vec<RegisteredFace>,
}

/// Per-backend font state. Owned by `AndroidBackend`. `register_asset`
/// + `register_typeface` mutate it; the style applier reads it to set
/// `TextView.setTypeface`.
#[derive(Default)]
pub(crate) struct FontRegistry {
    /// Cache of `Typeface` GlobalRefs keyed by font `AssetId`.
    /// Populated by `register_asset` (font branch only) and consulted
    /// by `register_typeface` to materialize each face.
    asset_typefaces: HashMap<AssetId, GlobalRef>,
    /// One entry per framework `Typeface` we've seen at least once.
    typefaces: HashMap<TypefaceId, RegisteredTypeface>,
}

impl FontRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// `Backend::register_asset` entry point for the font branch.
    /// Returns `true` if `kind == AssetTag::Font` (handled here, even
    /// on failure); the caller uses that as a signal to skip any
    /// image-cache branch.
    pub(crate) fn register_asset(
        &mut self,
        env: &mut JNIEnv,
        context: &GlobalRef,
        id: AssetId,
        kind: AssetTag,
        source: &AssetSource,
    ) -> bool {
        if kind != AssetTag::Font {
            return false;
        }
        if self.asset_typefaces.contains_key(&id) {
            return true;
        }
        // With `embed-font-bytes` on (this backend enables it), `face!`
        // emits `BundledEmbedded` (path + bytes); `Embedded` covers a
        // hand-rolled `embed_asset!` font. Both carry the bytes (+
        // extension) we write to the cache dir. Bytes-free `Bundled` /
        // `Remote` are intentionally unreachable for fonts — URL-loaded
        // fonts are not supported and only project-shipped files are
        // permitted.
        let (AssetSource::Embedded { bytes, extension }
        | AssetSource::BundledEmbedded { bytes, extension, .. }) = source
        else {
            log::info!(
                "[font] register_asset id={:?} skipped — bytes-free source (URL fonts are unsupported)",
                id
            );
            return true;
        };
        log::info!(
            "[font] register_asset id={:?} bytes={} ext={}",
            id,
            bytes.len(),
            extension
        );
        if let Some(tf) = write_font_to_cache_and_load(env, context, id, bytes, extension) {
            log::info!("[font] register_asset id={:?} → Typeface OK", id);
            self.asset_typefaces.insert(id, tf);
        } else {
            log::warn!(
                "[font] register_asset id={:?} FAILED — Typeface.createFromFile returned null \
                 (cacheDir unavailable, write_font_to_cache_and_load IO error, or font \
                 format unsupported)",
                id
            );
        }
        true
    }

    pub(crate) fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        _fallback: SystemFallback,
    ) {
        let mut registered = Vec::with_capacity(faces.len());
        let mut missing = 0usize;
        for f in faces {
            if let Some(tf) = self.asset_typefaces.get(&f.asset).cloned() {
                registered.push(RegisteredFace {
                    weight: f.weight,
                    style: f.style,
                    typeface: tf,
                });
            } else {
                missing += 1;
                log::warn!(
                    "[font] register_typeface family={:?}: face {:?}/{:?} asset_id={:?} \
                     not found in asset_typefaces — register_asset failed for this face",
                    family_name,
                    f.weight,
                    f.style,
                    f.asset
                );
            }
        }
        log::info!(
            "[font] register_typeface family={:?} id={:?} faces_resolved={}/{}{}",
            family_name,
            id,
            registered.len(),
            faces.len(),
            if missing > 0 { " (some faces missing — see warnings above)" } else { "" }
        );
        self.typefaces.insert(
            id,
            RegisteredTypeface {
                _family_name: family_name.to_string(),
                faces: registered,
            },
        );
    }

    pub(crate) fn unregister_typeface(&mut self, id: TypefaceId) {
        self.typefaces.remove(&id);
    }

    pub(crate) fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        if kind == AssetTag::Font {
            self.asset_typefaces.remove(&id);
        }
    }

    /// Look up the Android `Typeface` GlobalRef to set on a TextView
    /// for the given style request. Returns the resolved typeface
    /// **and the face's actual (weight, style)** — the caller needs
    /// the actual metadata to compute the `setTypeface(tf, int style)`
    /// synthesis flags correctly (see [`apply_resolved_font_to_textview`]
    /// for the math). Returns `None` when no custom typeface applies —
    /// the caller is then expected to fall through to the platform
    /// default (or `Typeface.create(name, style)` for
    /// `FontFamily::System`, which doesn't need the registry).
    pub(crate) fn resolve_typeface_ref(
        &self,
        family: Option<&FontFamily>,
        weight: FontWeight,
        style: FontStyle,
    ) -> Option<(&GlobalRef, FontWeight, FontStyle)> {
        let FontFamily::Typeface(t) = family? else {
            return None;
        };
        self.resolve_typeface_inner(t, weight, style)
    }

    fn resolve_typeface_inner(
        &self,
        t: &Typeface,
        weight: FontWeight,
        style: FontStyle,
    ) -> Option<(&GlobalRef, FontWeight, FontStyle)> {
        let entry = self.typefaces.get(&t.id)?;
        let face = pick_face(&entry.faces, weight, style)?;
        Some((&face.typeface, face.weight, face.style))
    }
}

// ---------------------------------------------------------------------------
// File-backed Typeface loader
// ---------------------------------------------------------------------------

/// Materialize a registered font: cache `bytes` to disk under the
/// app's cache dir and call `Typeface.createFromFile(String)`.
/// Returns `None` on any JNI / IO failure — the caller falls back to
/// the platform default.
fn write_font_to_cache_and_load(
    env: &mut JNIEnv,
    context: &GlobalRef,
    id: AssetId,
    bytes: &[u8],
    extension: &str,
) -> Option<GlobalRef> {
    let cache_dir_path = cache_dir_path(env, context)?;
    let dir = format!("{}/idealyst-fonts", cache_dir_path);
    std::fs::create_dir_all(&dir).ok()?;
    // Path is keyed by `AssetId` so re-registering the same asset is
    // idempotent: the second write hits the same path, and any
    // already-loaded Typeface in the registry short-circuits before we
    // reach here. Extension matters for `Typeface.createFromFile` —
    // the parser sniffs the file but a wrong extension is a footgun
    // for users grep'ing the cache.
    let file_path = format!("{}/{}.{}", dir, id.0, extension);
    std::fs::write(&file_path, bytes).ok()?;

    let typeface_cls = env.find_class("android/graphics/Typeface").ok()?;
    let path_jstr = env.new_string(&file_path).ok()?;
    let typeface_obj = env
        .call_static_method(
            &typeface_cls,
            "createFromFile",
            "(Ljava/lang/String;)Landroid/graphics/Typeface;",
            &[JValue::Object(&path_jstr.into())],
        )
        .and_then(|v| v.l())
        .ok()?;
    if typeface_obj.is_null() {
        return None;
    }
    env.new_global_ref(&typeface_obj).ok()
}

fn cache_dir_path(env: &mut JNIEnv, context: &GlobalRef) -> Option<String> {
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(|v| v.l())
        .ok()?;
    if cache_dir.is_null() {
        return None;
    }
    let path_jobj: JObject = env
        .call_method(
            &cache_dir,
            "getAbsolutePath",
            "()Ljava/lang/String;",
            &[],
        )
        .and_then(|v| v.l())
        .ok()?;
    if path_jobj.is_null() {
        return None;
    }
    let jstr = jni::objects::JString::from(path_jobj);
    let path: String = env.get_string(&jstr).ok()?.into();
    Some(path)
}

// ---------------------------------------------------------------------------
// Face matching
// ---------------------------------------------------------------------------

/// Pick the registered face that best matches `(weight, style)`. Same
/// scoring as the iOS resolver: exact-style first, closest weight as
/// the tiebreak.
fn pick_face<'a>(
    faces: &'a [RegisteredFace],
    weight: FontWeight,
    style: FontStyle,
) -> Option<&'a RegisteredFace> {
    if faces.is_empty() {
        return None;
    }
    let want_weight = weight_to_axis(weight);
    faces.iter().min_by(|a, b| {
        let key_a = (
            if a.style == style { 0 } else { 1 },
            (weight_to_axis(a.weight) - want_weight).abs(),
        );
        let key_b = (
            if b.style == style { 0 } else { 1 },
            (weight_to_axis(b.weight) - want_weight).abs(),
        );
        key_a.partial_cmp(&key_b).unwrap_or(std::cmp::Ordering::Equal)
    })
}

fn weight_to_axis(w: FontWeight) -> f32 {
    match w {
        FontWeight::Thin => 100.0,
        FontWeight::ExtraLight => 200.0,
        FontWeight::Light => 300.0,
        FontWeight::Normal => 400.0,
        FontWeight::Medium => 500.0,
        FontWeight::SemiBold => 600.0,
        FontWeight::Bold => 700.0,
        FontWeight::ExtraBold => 800.0,
        FontWeight::Black => 900.0,
    }
}

// ---------------------------------------------------------------------------
// Style-applier helpers
// ---------------------------------------------------------------------------

/// `Typeface.Style` constants. Combined into a single int when calling
/// `TextView.setTypeface(Typeface, int)`.
const TYPEFACE_STYLE_NORMAL: i32 = 0;
const TYPEFACE_STYLE_BOLD: i32 = 1;
const TYPEFACE_STYLE_ITALIC: i32 = 2;
const TYPEFACE_STYLE_BOLD_ITALIC: i32 = 3;

fn is_bold_weight(w: FontWeight) -> bool {
    matches!(
        w,
        FontWeight::SemiBold
            | FontWeight::Bold
            | FontWeight::ExtraBold
            | FontWeight::Black
    )
}

fn typeface_style(weight: FontWeight, style: FontStyle) -> i32 {
    match (is_bold_weight(weight), style == FontStyle::Italic) {
        (true, true) => TYPEFACE_STYLE_BOLD_ITALIC,
        (true, false) => TYPEFACE_STYLE_BOLD,
        (false, true) => TYPEFACE_STYLE_ITALIC,
        (false, false) => TYPEFACE_STYLE_NORMAL,
    }
}

/// Bits the picked face's *own* weight/style don't already satisfy.
/// Hand this to `setTypeface(Typeface, int)` so Android only fake-
/// bolds / fake-italicizes the axes that the file genuinely doesn't
/// cover — keeps a real `Inter-Bold` from rendering as
/// fake-bold-on-top-of-bold.
///
/// "Bold" here means semantic-bold (SemiBold+), matching
/// [`typeface_style`]. So requesting `Bold` against a `SemiBold`
/// face produces no synthesis (close enough, leave it alone);
/// requesting `Bold` against a `Regular`-only family does synthesize.
fn synthesis_style(
    req_weight: FontWeight,
    req_style: FontStyle,
    face_weight: FontWeight,
    face_style: FontStyle,
) -> i32 {
    let need_bold = is_bold_weight(req_weight) && !is_bold_weight(face_weight);
    let need_italic = req_style == FontStyle::Italic && face_style != FontStyle::Italic;
    match (need_bold, need_italic) {
        (true, true) => TYPEFACE_STYLE_BOLD_ITALIC,
        (true, false) => TYPEFACE_STYLE_BOLD,
        (false, true) => TYPEFACE_STYLE_ITALIC,
        (false, false) => TYPEFACE_STYLE_NORMAL,
    }
}

/// Apply the resolved typeface to a `TextView`. Returns `true` when a
/// font was set so the caller can skip the system-font fallback.
///
/// Order of precedence:
/// 1. `FontFamily::Typeface` resolved from the registry → exact face.
/// 2. `FontFamily::System(name)` → `Typeface.create(name, style)`.
/// 3. Just `font_weight`/`font_style` → `Typeface.defaultFromStyle(...)`.
///
/// ## Synthesis flag math for path 1
///
/// `Typeface.createFromFile` returns a typeface whose `getStyle()` is
/// always `NORMAL` regardless of what weight is actually inside the
/// file — Android doesn't read the OS/2 metadata on the load path.
/// So if we hand `setTypeface(inter_bold_typeface, BOLD)` to a
/// TextView, Android sees "typeface is NORMAL, request is BOLD" and
/// turns on fake-bold via `Paint.setFakeBoldText(true)` — fake-bold
/// applied **on top of** real bold, which renders too thick (or
/// substitutes a system font entirely on some Android versions).
///
/// The fix: only request synthesis for bits the picked face doesn't
/// already satisfy. Pass `Typeface.NORMAL` when the picked face's
/// own weight/style already matches the request; pass `BOLD` or
/// `ITALIC` only for the axes that need faking. The face picker has
/// already selected the closest-matching registered face, so this
/// only kicks in when the registered family genuinely lacks the
/// requested weight/style (e.g. asking for italic when only upright
/// is bundled).
pub(crate) fn apply_resolved_font_to_textview(
    env: &mut JNIEnv,
    text_view: &JObject,
    registry: &FontRegistry,
    family: Option<&FontFamily>,
    weight: FontWeight,
    style: FontStyle,
) -> bool {
    // 1. Registry-backed typeface.
    if let Some((tf, face_weight, face_style)) =
        registry.resolve_typeface_ref(family, weight, style)
    {
        let synthesis = synthesis_style(weight, style, face_weight, face_style);
        let _ = env.call_method(
            text_view,
            "setTypeface",
            "(Landroid/graphics/Typeface;I)V",
            &[JValue::Object(tf.as_obj()), JValue::Int(synthesis)],
        );
        return true;
    }

    let combined_style = typeface_style(weight, style);

    // 2. `FontFamily::System(name)` — hand the name to
    //    `Typeface.create(String, int)`. Build + call in the same
    //    scope so the local-ref JObject's lifetime stays inside the
    //    JNIEnv frame.
    if let Some(FontFamily::System(name)) = family {
        if create_and_set_named_typeface(env, text_view, name, combined_style) {
            return true;
        }
    }

    // 3. No family — `Typeface.defaultFromStyle(int)` lets a pure
    //    `font_weight: Bold` set the default-font bold without an
    //    explicit family.
    if matches!(family, None) && combined_style != TYPEFACE_STYLE_NORMAL {
        if create_and_set_default_typeface(env, text_view, combined_style) {
            return true;
        }
    }

    false
}

/// `Typeface.create(name, style)` then `TextView.setTypeface(tf, style)`
/// in a single scope. Returns `true` on success. Wrapping the local
/// `JObject` lookup + use in one function avoids leaking its lifetime
/// out of the JNI frame.
fn create_and_set_named_typeface(
    env: &mut JNIEnv,
    text_view: &JObject,
    name: &str,
    style: i32,
) -> bool {
    let Ok(typeface_cls) = env.find_class("android/graphics/Typeface") else {
        return false;
    };
    let Ok(name_jstr) = env.new_string(name) else {
        return false;
    };
    let result = env
        .call_static_method(
            &typeface_cls,
            "create",
            "(Ljava/lang/String;I)Landroid/graphics/Typeface;",
            &[JValue::Object(&name_jstr.into()), JValue::Int(style)],
        )
        .and_then(|v| v.l());
    let Ok(tf_obj) = result else { return false };
    if tf_obj.is_null() {
        return false;
    }
    env.call_method(
        text_view,
        "setTypeface",
        "(Landroid/graphics/Typeface;I)V",
        &[JValue::Object(&tf_obj), JValue::Int(style)],
    )
    .is_ok()
}

fn create_and_set_default_typeface(
    env: &mut JNIEnv,
    text_view: &JObject,
    style: i32,
) -> bool {
    let Ok(typeface_cls) = env.find_class("android/graphics/Typeface") else {
        return false;
    };
    let result = env
        .call_static_method(
            &typeface_cls,
            "defaultFromStyle",
            "(I)Landroid/graphics/Typeface;",
            &[JValue::Int(style)],
        )
        .and_then(|v| v.l());
    let Ok(tf_obj) = result else { return false };
    if tf_obj.is_null() {
        return false;
    }
    env.call_method(
        text_view,
        "setTypeface",
        "(Landroid/graphics/Typeface;I)V",
        &[JValue::Object(&tf_obj), JValue::Int(style)],
    )
    .is_ok()
}
