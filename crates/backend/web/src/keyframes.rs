//! Native CSS keyframe loops — the web twin of the macOS render-server
//! `CAKeyframeAnimation` path behind `ViewOps::install_keyframe_animation`.
//!
//! A forever loop driven by the wasm-side per-frame clock costs a
//! reactive tick + a style write EVERY frame and trips the debug
//! `[anim-stuck]` watchdog (it pins the 60 Hz clock by design). The
//! same loop as a CSS `@keyframes` animation runs on the compositor and
//! costs the wasm side nothing per frame. Only props with a pure-CSS
//! single-property representation map; unmapped props return `false`
//! and keep the per-frame `AnimatedValue` path.
//!
//! Contract notes (mirrors the macOS impl):
//! - The animation drives the RENDERED value only; inline styles are
//!   untouched. Callers must not drive the same prop through
//!   `set_animated_f32` concurrently — the framework's "native path
//!   taken → no fallback animator" convention guarantees that.
//! - One `animation` shorthand per element: a re-install (e.g. the
//!   Progress sweep re-ranging after a resize) REPLACES the element's
//!   animation. Composing several keyframe props on one element is not
//!   supported — the first caller that needs it should switch this to
//!   a per-element prop→name map joined with commas.
//! - `@keyframes` rules are deduped by content hash and appended to a
//!   dedicated `<style id="iy-keyframes">` sheet (never the engine's
//!   managed class sheet — its rule indices are load-bearing). Rules
//!   are tiny and never removed; distinct installs are bounded by
//!   distinct (prop, track-width) pairs in practice.

use std::cell::RefCell;
use std::collections::HashSet;

use runtime_shared::animation::AnimProp;
use wasm_bindgen::JsCast;

thread_local! {
    /// Content hashes of `@keyframes` rules already inserted.
    static INSTALLED: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

const KEYFRAMES_STYLE_ID: &str = "iy-keyframes";

/// The dedicated `<style>` hosting all generated `@keyframes` rules,
/// created on first use.
fn keyframes_sheet(document: &web_sys::Document) -> Option<web_sys::CssStyleSheet> {
    let el = match document.get_element_by_id(KEYFRAMES_STYLE_ID) {
        Some(el) => el,
        None => {
            let el = document.create_element("style").ok()?;
            el.set_id(KEYFRAMES_STYLE_ID);
            document.head()?.append_child(&el).ok()?;
            el
        }
    };
    let style_el: web_sys::HtmlStyleElement = el.dyn_into().ok()?;
    style_el.sheet()?.dyn_into().ok()
}

/// CSS declaration for one keyframe sample of `prop`, or `None` when
/// the prop has no single-property CSS mapping.
fn css_decl(prop: AnimProp, v: f32) -> Option<String> {
    match prop {
        AnimProp::Opacity => Some(format!("opacity:{v}")),
        AnimProp::TranslateX => Some(format!("transform:translateX({v}px)")),
        _ => None,
    }
}

/// Install `keyframes` on `el` as a compositor-driven CSS animation.
/// Returns `true` when handled; `false` signals the framework to fall
/// back to the per-frame clock path.
pub(crate) fn install(
    el: &web_sys::Element,
    prop: AnimProp,
    keyframes: &[(f32, f32)],
    duration_ms: u32,
    repeat_forever: bool,
    autoreverse: bool,
) -> bool {
    if keyframes.len() < 2 || duration_ms == 0 {
        return false;
    }
    let Some(html): Option<&web_sys::HtmlElement> = el.dyn_ref() else {
        return false;
    };

    // Rule body first — bail before touching the DOM if the prop
    // doesn't map.
    let mut body = String::new();
    for (t, v) in keyframes {
        let Some(decl) = css_decl(prop, *v) else { return false };
        let pct = (t.clamp(0.0, 1.0) * 100.0) as f64;
        body.push_str(&format!("{pct}%{{{decl}}}"));
    }

    // Content-addressed name so identical installs (every Progress bar
    // at the same track width) share one rule.
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        body.hash(&mut h);
        h.finish()
    };
    let name = format!("iy-kf-{hash:x}");

    let Some(document) = el.owner_document() else { return false };
    let Some(sheet) = keyframes_sheet(&document) else { return false };
    let fresh = INSTALLED.with(|s| s.borrow_mut().insert(hash));
    if fresh {
        let rule = format!("@keyframes {name}{{{body}}}");
        let idx = sheet.css_rules().map(|r| r.length()).unwrap_or(0);
        // A rejected rule (engine quirk — see `style.rs` on Firefox's
        // spec-mandated SyntaxError) just falls back to the clock path.
        if sheet.insert_rule_with_index(&rule, idx).is_err() {
            INSTALLED.with(|s| s.borrow_mut().remove(&hash));
            return false;
        }
    }

    let count = if repeat_forever { "infinite" } else { "1" };
    let direction = if autoreverse { "alternate" } else { "normal" };
    // ease-in-out between keyframes — matches the macOS impl's
    // per-segment `easeInEaseOut` timing function and the per-frame
    // fallback's `ease_in_out` tweens.
    let shorthand = format!("{name} {duration_ms}ms ease-in-out {count} {direction} both");
    html.style().set_property("animation", &shorthand).is_ok()
}
