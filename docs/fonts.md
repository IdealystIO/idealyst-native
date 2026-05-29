# Fonts and typefaces

Custom typefaces are declared once (the `typeface!` + `face!` macros)
and registered with each backend at first style-apply, with one
consumer surface (`StyleRules.font_family`). Everything else — hashing
a stable id, wiring the right native API on each platform, and
deciding whether the bytes ride inside the binary or are linked as a
separate file — is the framework's problem.

**Web links fonts as files; native embeds the bytes.** The binary only
carries the font bytes when a *byte-consuming* backend is in the build
(cosmic-text/wgpu Simulator, CoreText on iOS/macOS, Android
`Typeface`). Those backends turn on the `embed-font-bytes` cargo
feature on `runtime-core` through their own dep; the macro reacts to
the unified feature set. A pure web (DOM) build links none of them, so
`face!` emits a bytes-free path and the web backend serves the font as
a normal static file via `@font-face { src: url(...) }` — keeping the
fonts out of the wasm download. See `runtime_core::assets` for the
`embed-font-bytes` / `__face_source!` mechanism.

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
  - `asset: AssetId(const_hash(crate_name + "::" + path))` — stable id
    under the same path every build; two crates referencing the same
    file get distinct ids.
  - `source:` depends on the `embed-font-bytes` feature (set by the dep
    graph, see the intro):
    - **on** → `AssetSource::BundledEmbedded { path, bytes, extension }`
      — `include_bytes!` bytes for native/wgpu, plus a bundle path the
      web backend links.
    - **off** → `AssetSource::Bundled { path }` — path only, no
      `include_bytes!`, nothing read at compile time.
    The `path` is normalized from the `src:` literal (leading `./` /
    `../` stripped, e.g. `"../fonts/Inter-Bold.ttf"` →
    `"fonts/Inter-Bold.ttf"`), so the web backend can serve it at a
    root-absolute URL.

- `typeface! { name, faces: [...], fallback }`
  - Expands to a `Typeface { id, family_name, faces, fallback }`
    literal.
  - `id: TypefaceId(const_hash(crate_name + "::typeface::" + name))`.
  - Must be assigned to a `static` (or `const`) so the `&'static`
    references on `faces` are stable.

Only project-shipped font files are supported — arbitrary remote
(`Remote { url }`) fonts are intentionally not part of the contract.
Ship the file with the app or don't ship the font. On web that file is
served from the bundle; on native its bytes are embedded.

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
backend. The framework hands all of them the same data — `face.asset`
(an opaque id), `face.source` (a bundle path and, when
`embed-font-bytes` is on, the bytes), `tf.family_name` — and each
translates it to the native font API.

### Web (`backend-web`)

At `register_asset` the backend resolves a font to a **root-absolute
served-file URL** (`/{path}`) — never a blob, and the embedded bytes
(if any rode along as `BundledEmbedded`) are ignored. `register_typeface`
then emits one `@font-face` rule per face linking that file:

```css
@font-face {
  font-family: "Inter";
  src: url("/fonts/Inter-Bold.ttf") format("truetype");
  font-weight: 700;
  font-style: normal;
}
```

The browser fetches and HTTP-caches the `.ttf`/`.woff2` like any other
static asset, and only downloads the weights actually rendered. The
URL is root-absolute (not relative) so it resolves the same under the
SPA router regardless of the current document path. The file is served
from the project's top-level font dir: `idealyst dev --web` serves the
project root directly, and `idealyst build web` stages every top-level
asset directory (`fonts/`, `assets/`, …) into the deployed bundle — so
a `face!(src: "../fonts/Inter-Bold.ttf")` resolves to `/fonts/Inter-Bold.ttf`.

`StyleRules.font_family: Typeface(INTER)` maps to inline
`font-family: "Inter"` on the element, and the browser's own
font-weight/style matching picks the right `@font-face` rule by the
declared `font-weight` / `font-style` descriptors. Multi-weight
families just emit multiple `@font-face` rules sharing a `font-family`
name.

Caching: the framework's thread-local seen-set keeps the backend from
re-emitting the same rule. (A hand-rolled `embed_asset!` font with no
bundle path is the one case that still falls back to a `blob:` URL.)

#### Preloading fonts for the first paint

The `@font-face` rule isn't injected until wasm boots and the framework
walks the tree — by which time the first paint has already happened in
the fallback font, so when the real font arrives there's a visible
re-paint. The standard web fix is `<link rel="preload" as="font">` in
the static `<head>`: the browser starts fetching the font in parallel
with the wasm download, and by the time the runtime `@font-face` rule
appears, the font is already cached.

The framework doesn't auto-discover which fonts to preload (only the
project knows which weights are above the fold and which are rare).
Declare them in `Cargo.toml`:

```toml
[package.metadata.idealyst.app.web]
preload_fonts = [
    "fonts/Inter-Regular.ttf",
    "fonts/Inter-SemiBold.ttf",
    "fonts/Inter-Bold.ttf",
]
```

Both the build path (`idealyst build --web`'s `stage_bundle`) and the
dev server (`idealyst dev --web`'s `dev-http`) read this list and
splice one `<link rel="preload" as="font" crossorigin>` tag per entry
into the served HTML, so the dev loop and the deployed bundle preload
the same set. The `crossorigin` attribute is required for the preload
to dedupe with the framework's later `@font-face` fetch — without it
the browser issues two requests for the same font.

Trade-offs to keep in mind:
- **Preload only the weights actually rendered on the landing
  screen.** Each preload starts a parallel fetch competing with the
  wasm download; over-preloading rarer weights (Thin, ExtraLight, Black)
  for the marketing site you spend bandwidth that delays the wasm boot.
- **Empty list is fine.** Leaving `preload_fonts` unset means the
  runtime `@font-face` swap-in handles everything — there's a brief
  re-paint when the font arrives, which on a fast connection is barely
  perceptible. The choice is yours.
- **Source `index.html` stays untouched.** The injection happens in
  the staged bundle / served response. If you eject to a static bundle
  and edit `index.html` by hand, that's where the preload tags get
  baked in permanently; until then nothing in your repo carries them.

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
2. On native (`embed-font-bytes` on), confirm the bytes embedded: the
   byte-count printed alongside each id should match the file size on
   disk; if it's `0` the `include_bytes!` resolved a wrong path.
   (`face!` resolves `src` relative to the calling source file, not the
   crate root — adjust the `../` prefix accordingly.) On web, instead
   open DevTools → Network and confirm each rendered weight fetches its
   `/fonts/<Face>.ttf` with `200` (not a `404` — a `404` means the file
   isn't in the served root / staged bundle, or the `src:` path doesn't
   normalize to where the file actually lives).
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
