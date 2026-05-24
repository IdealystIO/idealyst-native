# Fonts and typefaces

Custom typefaces are bundled into the binary and registered with each
backend at first style-apply. There's one declaration site (the
`typeface!` + `face!` macros) and one consumer surface
(`StyleRules.font_family`); everything else — embedding the bytes,
hashing a stable id, wiring the right native API on each platform —
is the framework's problem.

Implementation: `runtime_core::assets` (declaration macros + types),
`runtime_core::style::ensure_typefaces_registered_with` (registration
walk), `Backend::register_asset` + `Backend::register_typeface`
(per-platform receivers).

---

## The standard interface

Every project, every platform, one shape:

```rust
use runtime_core::{face, typeface, FontStyle, FontWeight, SystemFallback, Typeface};

pub static INTER: Typeface = typeface! {
    name: "Inter",
    faces: [
        face!(weight: FontWeight::Normal, style: FontStyle::Normal,
              src: "../fonts/Inter-Regular.ttf"),
        face!(weight: FontWeight::Bold,   style: FontStyle::Normal,
              src: "../fonts/Inter-Bold.ttf"),
        face!(weight: FontWeight::Bold,   style: FontStyle::Italic,
              src: "../fonts/Inter-BoldItalic.ttf"),
    ],
    fallback: SystemFallback::SansSerif,
};
```

Then in a stylesheet:

```rust
font_family: Some((&INTER).into()),
font_weight: Some(FontWeight::Bold),
font_style: Some(FontStyle::Italic),
```

That's the whole API. Don't hand-roll `Typeface { ... }` literals,
don't compute `AssetId(const_hash(...))` yourself, don't call
`include_bytes!` directly — the macros do all of that and keep the id
scheme stable across crates.

### What the macros generate

- `face!(weight: …, style: …, src: "path/relative/to/this/file.ttf")`
  - Expands to a `TypefaceFace { weight, style, asset, source }`
    literal.
  - `src` is resolved like `include_bytes!` — relative to the **calling
    source file**, not the crate root.
  - `asset: AssetId(const_hash(crate_name + "::" + path))` — same
    bytes embedded under the same id every build, but two crates
    embedding the same file get distinct ids.
  - `source: AssetSource::Embedded { bytes, extension }` — bytes from
    `include_bytes!`, extension derived from the trailing `.ext` of
    the path literal.

- `typeface! { name, faces: [...], fallback }`
  - Expands to a `Typeface { id, family_name, faces, fallback }`
    literal.
  - `id: TypefaceId(const_hash(crate_name + "::typeface::" + name))`.
  - Must be assigned to a `static` (or `const`) so the `&'static`
    references on `faces` are stable.

Only `Embedded` sources are supported for fonts — URL-loaded fonts
are intentionally not part of the contract. Ship the bytes with the
app or don't ship the font.

---

## Adding a typeface — checklist

1. Drop the `.ttf`/`.otf` files into a `fonts/` directory next to
   the source file that declares the typeface. Convention is one
   directory per crate (e.g. `examples/welcome/fonts/`); the macros
   don't enforce it, but it keeps the `src:` paths short and
   uniform.
2. Declare a `pub static MY_FACE: Typeface = typeface! { ... };`
   with one `face!` per `.ttf` you bundled. Add **all** weights /
   styles you actually plan to use — see [Why bundle every weight](#why-bundle-every-weight)
   below.
3. Reference it from a stylesheet via `font_family: Some((&MY_FACE).into())`.
4. Pick a sensible `SystemFallback` for the family
   (`SansSerif`/`Serif`/`Monospace`/`None`). Used if a backend
   can't resolve the registered face at runtime — usually only on
   web pre-`@font-face`-load, but native paths also consult it
   when the picked face is None.

There is no separate "register" call. The framework's
`ensure_typefaces_registered_with` walks every `StyleRules` it sees,
finds any `FontFamily::Typeface(tf)`, and calls
`Backend::register_asset` + `Backend::register_typeface` once per
unique `TypefaceId`. So the first style-apply that mentions the
typeface registers it; later applies short-circuit through a
thread-local seen-set.

---

## Why bundle every weight

The framework's face picker chooses the closest registered face by
`(style_match, weight_distance)`. If you bundle only `Regular` and
ask for `Bold`, the picker returns `Regular`. From there each
platform tries to "make it bold":

- **Web** synthesizes bold via the browser's font-weight algorithm
  (visually mediocre — uneven stroke widths).
- **iOS** does no synthesis — it just renders `Regular` at the
  requested size. The text reads as Regular weight.
- **Android** turns on `Paint.setFakeBoldText(true)` — algorithmic
  faux-bold, the worst of the three.

None of these match a real Bold cut. If you want Bold, ship
`Inter-Bold.ttf`. If you want SemiBold, ship `Inter-SemiBold.ttf`.
The macro form makes it cheap — one extra `face!` line per weight.

The same applies to italic: bundle a real italic face for every
weight you also use upright. If you skip them the backends will
fake-italicize, which on Android in particular doesn't render well.

---

## Per-platform mechanics

The same `typeface!` declaration takes a different path through each
backend. The framework hands all three the same data — `face.asset`
(an opaque id), `face.source` (the embedded bytes), `tf.family_name`
— and each translates it to the native font API.

### Web (`backend-web`)

At `register_asset` the backend wraps the embedded bytes in a `Blob`
URL and emits an `@font-face` rule:

```css
@font-face {
  font-family: "Inter";
  src: url(blob:…) format("truetype");
  font-weight: 700;
  font-style: normal;
}
```

`StyleRules.font_family: Typeface(INTER)` then maps to inline
`font-family: "Inter"` on the element, and the browser's own
font-weight/style matching picks the right `@font-face` rule by the
declared `font-weight` and `font-style` descriptors on each.
Multi-weight families just emit multiple `@font-face` rules sharing a
`font-family` name.

Caching: blob URLs survive for the lifetime of the page; the
framework's thread-local seen-set keeps the backend from re-emitting
the same rule.

### iOS (`backend-ios-mobile` + `backend-ios-core`)

At `register_asset` the backend hands the bytes to CoreText via
`CGFontCreateWithDataProvider` →
`CTFontManagerRegisterGraphicsFont`. CoreText returns the file's
**PostScript name** (e.g., `Inter-Regular`, `Inter-Bold`), which the
backend stores keyed by `AssetId` in `asset_psnames`.

At `register_typeface`, each face's `asset_id` is looked up in
`asset_psnames` to recover its PostScript name, and the result is
stored under the `TypefaceId`. At style-apply, the face picker
chooses a `(weight, style)`-matching face and resolves it via
`+[UIFont fontWithName: <postscript_name> size: <size>]`. UIKit
honors the PostScript name directly — no synthesis, no fallback to
system unless the PostScript-name lookup itself fails.

iOS emits `[font] register_asset id=… → PostScript name …` log
lines on success and `…FAILED to register with CoreText` on failure;
filter by process in the iOS unified log (`log show --predicate
'process == "AppName"'`).

### Android (`backend-android-mobile`)

`Typeface.createFromFile` is the only public API that takes raw
bytes pre-API-26, so at `register_asset` the backend writes the
bytes to `Context.cacheDir/idealyst-fonts/<asset_id>.<ext>` and
calls `Typeface.createFromFile(path)`. The resulting Android
`Typeface` is stored as a `GlobalRef` keyed by `AssetId`.

At style-apply, the face picker chooses the closest registered face,
and the backend calls `TextView.setTypeface(typeface, int style)`.
**Here Android needs careful handling**:

- `Typeface.createFromFile` always returns a typeface whose
  `getStyle()` is `NORMAL`, regardless of what weight is actually
  inside the file — Android doesn't parse OS/2 metadata on this
  load path.
- The two-arg `setTypeface(tf, style)` treats `style` as a synthesis
  request: bits set in `style` but not present in `tf.getStyle()`
  trigger `Paint.setFakeBoldText(true)` / `setTextSkewX(...)`.
- Naively passing `BOLD` whenever the caller asked for Bold would
  therefore fake-bold *on top of* a real Bold typeface, which renders
  noticeably too thick (or substitutes a system font on some Android
  versions).

The fix is in [`apply_resolved_font_to_textview`](../crates/backend/android/mobile/src/imp/font.rs):
the `synthesis_style` helper computes the int flag as the
**difference** between requested and picked-face style. Pass `NORMAL`
when the registered face already satisfies the request (no synthesis
needed); pass `BOLD` / `ITALIC` only for axes the registered family
genuinely lacks. So:

| Requested      | Registered faces            | Picker returns | setTypeface flag |
|----------------|-----------------------------|----------------|------------------|
| Bold, upright  | Regular only                | Regular        | `BOLD` (synth)   |
| Bold, upright  | Regular + Bold              | Bold           | `NORMAL`         |
| Bold, italic   | Regular + Bold (no italics) | Bold           | `ITALIC` (synth) |
| Bold, italic   | full Bold-Italic registered | Bold-Italic    | `NORMAL`         |

Android emits `[font] register_asset id=… → Typeface OK` /
`…FAILED — Typeface.createFromFile returned null` log lines and
`[font] register_typeface family=… faces_resolved=N/M` summaries.
Filter logcat with `adb logcat -v time | grep '\[font\]'`.

### Other backends

`backend-android-tv` and `backend-android-core` don't have their own
font apply paths — only `backend-android-mobile` and `backend-ios-mobile`
materialize custom typefaces. runtime-server / dev-client traffic preserves the
`TypefaceId` and `family_name` on the wire (`WireFontFamily::Typeface
{ id, family_name }`) and rehydrates a stub `Typeface` on the replay
side; the actual face bytes were already registered against the
target backend by the recording side, so the replay just references
them by id.

---

## Diagnostics

If text doesn't look right:

1. Check that the registration walk actually fired. iOS logs `[font]
   register_asset id=… → PostScript name …` per face; Android logs
   `[font] register_asset … → Typeface OK` + a closing
   `[font] register_typeface family=… faces_resolved=N/N`. **N must
   equal the number of `face!` entries you wrote.** A mismatch means
   one or more face files failed to decode.
2. Confirm the bytes embedded. The byte-count printed alongside each
   id should match the file size on disk; if it's `0` the
   `include_bytes!` resolved a wrong path. (`face!` resolves `src`
   relative to the calling source file, not the crate root — adjust
   the `../` prefix accordingly.)
3. If the headline is rendering bold *too* thick on Android, the
   synthesis bug above has regressed — verify the `setTypeface` call
   in [`apply_resolved_font_to_textview`](../crates/backend/android/mobile/src/imp/font.rs)
   is still passing `synthesis_style(...)` and not the raw
   `typeface_style(...)` flag.
4. If the same project renders correctly on web and broken on
   iOS / Android, suspect a missing face: e.g. you asked for
   `FontWeight::Bold` but only registered `FontWeight::Normal`.
   The web backend silently synthesizes; native does not.

For a quick visual canary, register an unmistakable display face
(e.g. `Press Start 2P`) alongside Inter and apply it to a known label
— if that label renders in 8-bit pixel glyphs, the custom-font
pipeline is wired end-to-end and any remaining issue is about
weight/style matching rather than registration.
