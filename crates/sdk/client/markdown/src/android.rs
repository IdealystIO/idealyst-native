//! Android handler for the `markdown` external.
//!
//! Renders the WHOLE document as ONE `android.widget.TextView` fed a
//! `SpannableStringBuilder` built from the shared [`segments::lower`]
//! flattening. Each segment contributes one foreground-color span + one
//! absolute-size span, plus optional style (bold/italic), monospace
//! typeface, background, underline, and strikethrough spans over its
//! UTF-16 range. One TextView regardless of document size — the
//! per-token-widget explosion is avoided entirely (see [`crate`] docs).
//!
//! No custom Kotlin class: a plain `TextView` returned from an external
//! handler gets width-aware wrapping measurement automatically (the
//! backend installs a `View.measure(AT_MOST width, …)` measure_fn), so
//! the document wraps to the column like normal text.

use crate::ir::MarkdownDoc;
use crate::segments::{self, Seg};
use backend_android::AndroidBackend;
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;
use std::rc::Rc;

/// `Spanned.SPAN_INCLUSIVE_EXCLUSIVE` — the standard flag for character
/// styling that doesn't expand when text is inserted at the boundary.
const SPAN_INCLUSIVE_EXCLUSIVE: i32 = 0x21;
/// `Typeface.BOLD` / `ITALIC` / `BOLD_ITALIC`.
const STYLE_BOLD: i32 = 1;
const STYLE_ITALIC: i32 = 2;
const STYLE_BOLD_ITALIC: i32 = 3;

pub(crate) fn build(doc: &Rc<MarkdownDoc>, backend: &mut AndroidBackend) -> GlobalRef {
    let segs = segments::lower(doc);

    // Concatenate all segment text + record each segment's UTF-16
    // (start, end) — Java string offsets are UTF-16 code units.
    let total: usize = segs.iter().map(|s| s.text.len()).sum();
    let mut full = String::with_capacity(total);
    let mut ranges: Vec<(i32, i32)> = Vec::with_capacity(segs.len());
    for seg in &segs {
        let start = full.encode_utf16().count() as i32;
        full.push_str(&seg.text);
        let end = full.encode_utf16().count() as i32;
        ranges.push((start, end));
    }

    backend.with_jni(|env, context| {
        build_textview(env, &context.as_obj(), &segs, &ranges, &full)
            .unwrap_or_else(|| fallback_view(env, &context.as_obj(), &full))
    })
}

fn build_textview(
    env: &mut JNIEnv,
    context: &JObject,
    segs: &[Seg],
    ranges: &[(i32, i32)],
    full: &str,
) -> Option<GlobalRef> {
    let tv_class = env.find_class("android/widget/TextView").ok()?;
    let tv = env
        .new_object(
            &tv_class,
            "(Landroid/content/Context;)V",
            &[JValue::Object(context)],
        )
        .ok()?;
    // Match iOS UILabel's tighter metrics.
    let _ = env.call_method(&tv, "setIncludeFontPadding", "(Z)V", &[JValue::Bool(0)]);

    // SpannableStringBuilder(full)
    let java_full = env.new_string(full).ok()?;
    let ssb_class = env.find_class("android/text/SpannableStringBuilder").ok()?;
    let ssb = env
        .new_object(
            &ssb_class,
            "(Ljava/lang/CharSequence;)V",
            &[JValue::Object(&java_full)],
        )
        .ok()?;

    for (seg, &(start, end)) in segs.iter().zip(ranges) {
        if start >= end {
            continue;
        }
        // Bound the per-segment local refs so a large doc doesn't
        // overflow the JNI local reference table.
        let _ = env.with_local_frame(16, |env| -> Result<(), jni::errors::Error> {
            apply_segment_spans(env, &ssb, seg, start, end);
            Ok(())
        });
    }

    // setText(CharSequence)
    env.call_method(
        &tv,
        "setText",
        "(Ljava/lang/CharSequence;)V",
        &[JValue::Object(&ssb)],
    )
    .ok()?;

    env.new_global_ref(&tv).ok()
}

/// Apply every span this segment needs over `[start, end)`.
fn apply_segment_spans(env: &mut JNIEnv, ssb: &JObject, seg: &Seg, start: i32, end: i32) {
    // Foreground color (always).
    if let Some(span) = color_span(env, "android/text/style/ForegroundColorSpan", &seg.style.color)
    {
        set_span(env, ssb, &span, start, end);
    }
    // Absolute size in dip (always) — mirrors iOS's absolute point sizes.
    if let Some(span) = int_bool_span(
        env,
        "android/text/style/AbsoluteSizeSpan",
        seg.style.size.round() as i32,
        true,
    ) {
        set_span(env, ssb, &span, start, end);
    }
    // Bold / italic.
    let style = match (seg.style.bold, seg.style.italic) {
        (true, true) => Some(STYLE_BOLD_ITALIC),
        (true, false) => Some(STYLE_BOLD),
        (false, true) => Some(STYLE_ITALIC),
        (false, false) => None,
    };
    if let Some(s) = style {
        if let Some(span) = int_span(env, "android/text/style/StyleSpan", s) {
            set_span(env, ssb, &span, start, end);
        }
    }
    // Monospace.
    if seg.style.mono {
        if let Some(span) = typeface_span(env, "monospace") {
            set_span(env, ssb, &span, start, end);
        }
    }
    // Background tint.
    if let Some(bg) = &seg.style.bg {
        if let Some(span) = color_span(env, "android/text/style/BackgroundColorSpan", bg) {
            set_span(env, ssb, &span, start, end);
        }
    }
    // Underline (links).
    if seg.style.underline {
        if let Some(span) = empty_span(env, "android/text/style/UnderlineSpan") {
            set_span(env, ssb, &span, start, end);
        }
    }
    // Strikethrough.
    if seg.style.strike {
        if let Some(span) = empty_span(env, "android/text/style/StrikethroughSpan") {
            set_span(env, ssb, &span, start, end);
        }
    }
}

fn set_span(env: &mut JNIEnv, ssb: &JObject, span: &JObject, start: i32, end: i32) {
    let _ = env.call_method(
        ssb,
        "setSpan",
        "(Ljava/lang/Object;III)V",
        &[
            JValue::Object(span),
            JValue::Int(start),
            JValue::Int(end),
            JValue::Int(SPAN_INCLUSIVE_EXCLUSIVE),
        ],
    );
}

fn color_span<'a>(
    env: &mut JNIEnv<'a>,
    class: &str,
    color: &str,
) -> Option<JObject<'a>> {
    let argb = parse_color_argb(color).unwrap_or(0xFF00_0000u32 as i32);
    int_span(env, class, argb)
}

fn int_span<'a>(env: &mut JNIEnv<'a>, class: &str, arg: i32) -> Option<JObject<'a>> {
    let c = env.find_class(class).ok()?;
    env.new_object(&c, "(I)V", &[JValue::Int(arg)]).ok()
}

fn int_bool_span<'a>(
    env: &mut JNIEnv<'a>,
    class: &str,
    arg: i32,
    flag: bool,
) -> Option<JObject<'a>> {
    let c = env.find_class(class).ok()?;
    env.new_object(
        &c,
        "(IZ)V",
        &[JValue::Int(arg), JValue::Bool(flag as u8)],
    )
    .ok()
}

fn typeface_span<'a>(env: &mut JNIEnv<'a>, family: &str) -> Option<JObject<'a>> {
    let c = env.find_class("android/text/style/TypefaceSpan").ok()?;
    let fam = env.new_string(family).ok()?;
    env.new_object(&c, "(Ljava/lang/String;)V", &[JValue::Object(&fam)])
        .ok()
}

fn empty_span<'a>(env: &mut JNIEnv<'a>, class: &str) -> Option<JObject<'a>> {
    let c = env.find_class(class).ok()?;
    env.new_object(&c, "()V", &[]).ok()
}

/// Last-resort: a plain TextView with unstyled text, so the framework
/// gets a node back even if a span class lookup failed.
fn fallback_view(env: &mut JNIEnv, context: &JObject, full: &str) -> GlobalRef {
    let class = env.find_class("android/widget/TextView").unwrap();
    let tv = env
        .new_object(
            &class,
            "(Landroid/content/Context;)V",
            &[JValue::Object(context)],
        )
        .unwrap();
    if let Ok(s) = env.new_string(full) {
        let _ = env.call_method(
            &tv,
            "setText",
            "(Ljava/lang/CharSequence;)V",
            &[JValue::Object(&s)],
        );
    }
    env.new_global_ref(&tv).unwrap()
}

/// Parse a CSS-style color (`#RGB`, `#RRGGBB`, `#AARRGGBB`, `rgb(...)`,
/// `rgba(...)`) to a packed Android ARGB int. Mirrors codeblock's inline
/// parser — the framework's canonical parser lives behind an `Rgba`
/// intermediate that's overkill for this one JNI hop.
fn parse_color_argb(s: &str) -> Option<i32> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex_argb(hex);
    }
    if let Some(rest) = s.strip_prefix("rgba(").and_then(|r| r.strip_suffix(')')) {
        let mut p = rest.split(',').map(|x| x.trim());
        let r = p.next()?.parse::<u8>().ok()?;
        let g = p.next()?.parse::<u8>().ok()?;
        let b = p.next()?.parse::<u8>().ok()?;
        let a = (p.next()?.parse::<f32>().ok()?.clamp(0.0, 1.0) * 255.0).round() as u8;
        return Some(pack_argb(a, r, g, b));
    }
    if let Some(rest) = s.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) {
        let mut p = rest.split(',').map(|x| x.trim());
        let r = p.next()?.parse::<u8>().ok()?;
        let g = p.next()?.parse::<u8>().ok()?;
        let b = p.next()?.parse::<u8>().ok()?;
        return Some(pack_argb(0xFF, r, g, b));
    }
    None
}

fn parse_hex_argb(hex: &str) -> Option<i32> {
    match hex.len() {
        3 => {
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
