//! Web clock: the browser's `Date` and `Intl`, through web-glue bindings
//! (no wasm-bindgen, no js-sys). Both exist in windows and workers alike, so
//! nothing here needs a `window`.

use web_glue::string;

web_glue::import! {
    // `new Date(ms).getTimezoneOffset()` is UTC minus local, in minutes, for
    // THAT instant — the browser applies the zone's rule for the date, so a
    // past or future DST state comes out right.
    fn js_tz_offset_at(ms: f64) -> f64 = "(ms) => new Date(ms).getTimezoneOffset()";
    // 1 and the zone name written to `out`, or 0 when the engine has no
    // `Intl` (or reports no zone). `resolvedOptions().timeZone` is the
    // IANA name the browser resolved for the user (`"Europe/Paris"`).
    fn js_tz_name(out: usize) -> u32 =
        "(o) => { try { const z = Intl.DateTimeFormat().resolvedOptions().timeZone; \
           if (typeof z !== 'string' || z === '') return 0; G.retStr(z, o); return 1; } \
           catch (e) { return 0; } }";
}

pub(crate) fn now_micros() -> i64 {
    // `Date.now()` is whole milliseconds; `as` saturates the (impossible)
    // out-of-range case instead of wrapping.
    (web_glue::dom::date_now() as i64).saturating_mul(1_000)
}

pub(crate) fn offset_seconds_at(unix_micros: i64) -> i32 {
    let ms = unix_micros.div_euclid(1_000) as f64;
    let utc_minus_local = unsafe { js_tz_offset_at(ms) };
    // NaN (an instant outside `Date`'s ±8.64e15 ms range) reads as UTC.
    if !utc_minus_local.is_finite() {
        return 0;
    }
    // Negate: JS reports UTC − local, this crate local − UTC. Historic LMT
    // offsets can be fractional minutes; round to the second.
    (-utc_minus_local * 60.0).round() as i32
}

pub(crate) fn timezone() -> Option<String> {
    let mut hit = 0;
    let name = string::receive(|o| hit = unsafe { js_tz_name(o) });
    (hit != 0).then_some(name)
}
