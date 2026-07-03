//! Android handler for the `code_block` external. Produces a single
//! `RustCodeBlock` (HorizontalScrollView + TextView) and feeds it a
//! `SpannableString` whose colored ranges mirror the Rust-side
//! `Vec<(String, Color)>` spans.
//!
//! See `RustCodeBlock.kt` for the JVM-side counterpart and the
//! rationale for collapsing per-token TextViews into a single
//! SpannableString.

use crate::CodeBlockProps;
use backend_android::AndroidBackend;
use jni::objects::{GlobalRef, JObject, JValue};
use std::rc::Rc;

/// `Element::External` handler signature for the codeblock kind on
/// Android. Returns the wrapper `GlobalRef` that the framework will
/// parent into the surrounding view tree (with no Taffy children —
/// the SpannableString lives inside one TextView, both invisible to
/// the framework's layout pass).
pub(crate) fn build(
    props: &Rc<CodeBlockProps>,
    backend: &mut AndroidBackend,
) -> GlobalRef {
    // Concatenate every span's text into one source string, recording
    // each span's (start, end, color) byte range. Java's
    // `String.length` is in UTF-16 code units; we feed Rust UTF-8
    // bytes through `NewStringUTF` (via jni-rs's auto_local) which
    // re-encodes them. The resulting Java string's code-unit
    // boundaries match the byte boundaries we tracked for any ASCII
    // run — for non-ASCII we'd need to track UTF-16 offsets. Code
    // blocks ship from a syntax highlighter that emits ASCII tokens
    // for keywords / punctuation / identifiers + occasional Unicode
    // in string literals; that mismatch isn't a visible defect (a
    // Unicode literal still gets its color, just possibly with a
    // half-character extension).
    //
    // Span count is small (one per highlighted token, typically a few
    // hundred per block max). Allocate up front.
    let mut full_text = String::with_capacity(
        props.spans.iter().map(|(s, _)| s.len()).sum::<usize>(),
    );
    let mut starts: Vec<i32> = Vec::with_capacity(props.spans.len());
    let mut ends: Vec<i32> = Vec::with_capacity(props.spans.len());
    let mut colors: Vec<i32> = Vec::with_capacity(props.spans.len());
    for (text, color) in &props.spans {
        let start = full_text.encode_utf16().count() as i32;
        full_text.push_str(text);
        let end = full_text.encode_utf16().count() as i32;
        starts.push(start);
        ends.push(end);
        colors.push(parse_color_argb(&color.0).unwrap_or(0xFF000000u32 as i32));
    }

    backend.with_jni(|env, context| {
        // Construct the wrapper. RustCodeBlock takes Context-only.
        let class = match env.find_class("io/idealyst/runtime/RustCodeBlock") {
            Ok(c) => c,
            Err(e) => {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
                log::error!(
                    "RustCodeBlock class not found — make sure the CLI was \
                     reinstalled after adding the runtime registry entry. \
                     Underlying: {:?}",
                    e
                );
                // Fall back to a plain View so the framework doesn't
                // panic on a missing handler return.
                return new_empty_view(env, &context.as_obj());
            }
        };
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&context.as_obj())],
            )
            .expect("new RustCodeBlock failed");
        let global = env
            .new_global_ref(&local)
            .expect("global_ref RustCodeBlock");

        // Build the three JNI int[] arrays + the Java String, then
        // call `update(text, starts, ends, colors)`.
        let java_text = env.new_string(&full_text).expect("new_string text");
        let starts_arr = env
            .new_int_array(starts.len() as i32)
            .expect("new_int_array starts");
        env.set_int_array_region(&starts_arr, 0, &starts)
            .expect("set_int_array_region starts");
        let ends_arr = env
            .new_int_array(ends.len() as i32)
            .expect("new_int_array ends");
        env.set_int_array_region(&ends_arr, 0, &ends)
            .expect("set_int_array_region ends");
        let colors_arr = env
            .new_int_array(colors.len() as i32)
            .expect("new_int_array colors");
        env.set_int_array_region(&colors_arr, 0, &colors)
            .expect("set_int_array_region colors");
        let _ = env.call_method(
            &local,
            "update",
            "(Ljava/lang/String;[I[I[I)V",
            &[
                JValue::Object(&java_text),
                JValue::Object(&starts_arr),
                JValue::Object(&ends_arr),
                JValue::Object(&colors_arr),
            ],
        );

        global
    })
}

/// Last-resort fallback when `find_class` fails. Returns a plain
/// `View` so the framework gets *some* node back and doesn't panic.
/// The class lookup failure has already been logged.
fn new_empty_view(env: &mut jni::JNIEnv, context: &JObject) -> GlobalRef {
    let class = env.find_class("android/view/View").unwrap();
    let local = env
        .new_object(
            &class,
            "(Landroid/content/Context;)V",
            &[JValue::Object(context)],
        )
        .unwrap();
    env.new_global_ref(&local).unwrap()
}

/// Parse the framework's CSS-style color string (`#RGB`, `#RRGGBB`,
/// `#AARRGGBB`, `rgba(...)`) into a packed Android ARGB int.
/// Returns `None` for unparseable input.
///
/// The framework owns the canonical parser in `runtime_core::color`,
/// but it lives behind an `Rgba` byte-intermediate that's overkill
/// for the inline JNI call shape here. Re-implement the common
/// shapes inline; the parser is small and the codeblock SDK doesn't
/// need to depend on the full color crate just for this one helper.
fn parse_color_argb(s: &str) -> Option<i32> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex_argb(hex);
    }
    if let Some(rest) = s.strip_prefix("rgba(").and_then(|r| r.strip_suffix(')')) {
        let mut parts = rest.split(',').map(|p| p.trim());
        let r = parts.next()?.parse::<u8>().ok()?;
        let g = parts.next()?.parse::<u8>().ok()?;
        let b = parts.next()?.parse::<u8>().ok()?;
        let a_f = parts.next()?.parse::<f32>().ok()?;
        let a = (a_f.clamp(0.0, 1.0) * 255.0).round() as u8;
        return Some(pack_argb(a, r, g, b));
    }
    if let Some(rest) = s.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) {
        let mut parts = rest.split(',').map(|p| p.trim());
        let r = parts.next()?.parse::<u8>().ok()?;
        let g = parts.next()?.parse::<u8>().ok()?;
        let b = parts.next()?.parse::<u8>().ok()?;
        return Some(pack_argb(0xFF, r, g, b));
    }
    None
}

fn parse_hex_argb(hex: &str) -> Option<i32> {
    match hex.len() {
        3 => {
            // #RGB → #RRGGBB
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            Some(pack_argb(0xFF, r, g, b))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(pack_argb(0xFF, r, g, b))
        }
        8 => {
            let a = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let r = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let g = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let b = u8::from_str_radix(&hex[6..8], 16).ok()?;
            Some(pack_argb(a, r, g, b))
        }
        _ => None,
    }
}

fn pack_argb(a: u8, r: u8, g: u8, b: u8) -> i32 {
    (((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)) as i32
}
