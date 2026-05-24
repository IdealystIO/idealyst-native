# Icons

Icons in Idealyst are vector paths stored as `&'static IconData`
constants. Backends render them natively — inline SVG on web,
`CAShapeLayer` on iOS, `VectorDrawable` on Android. Because the
data is `const`, only the icons your code actually references end
up in the final binary; LTO strips the rest.

You're not stuck with a particular icon pack. The framework ships
**Lucide** as a default, but icons are just data, and mixing
multiple packs (or rolling your own) is a matter of importing
their constants — there's no registry to configure, no central
list to maintain.

## Using an icon

```rust
use runtime_core::icon;
use icons_lucide::{SEARCH, MENU};

ui! {
    icon(SEARCH)
    icon(MENU).color(|| theme.primary())
}
```

That's the entire surface for the simple case. `icon(...)` takes
an `IconData` constant and returns a primitive you can drop into
any tree. The chainable methods (`.color`, `.stroke`, `.draw_in`)
add reactive behavior; we'll cover them below.

## What `IconData` actually is

The `IconData` struct is tiny and const-constructible:

```rust
pub struct IconData {
    pub view_box: (u16, u16),
    pub paths: &'static [&'static str],
    pub fill_rule: FillRule,
}
```

- **`view_box`** — the SVG viewBox dimensions (`(24, 24)` for
  most icon sets).
- **`paths`** — one or more SVG path `d` strings. Multiple paths
  let you build multi-region icons (an outline plus a filled
  bowl, for example).
- **`fill_rule`** — `NonZero` (the SVG default) or `EvenOdd`.

Because the whole thing is `const`, an icon pack is a Rust module
of static constants. There's no init code, no runtime registry,
no per-icon allocation. The icons live in `.rodata` alongside
your string literals.

## How backends render

The Icon primitive maps to native vector primitives, not raster
images:

- **Web** — inline `<svg>`. Each path becomes a `<path>` element.
  Scales crisply at any size; theme-tintable via the `color`
  override or CSS `currentColor`.
- **iOS** — a `CAShapeLayer` per path. Vector all the way down;
  no anti-aliasing artifacts at the icon's natural rendering
  scale.
- **Android** — a `VectorDrawable`. Same story — the platform
  rasterizes only at draw time, at the exact display density.

This gives you sharp icons at every backbone size without
shipping multiple raster assets.

### One backend exception worth knowing

Buttons are different. When you pass an icon to a `Button` as
`leading_icon` or `trailing_icon`, **iOS and Android backends
rasterize the SVG to a bitmap** before handing it to the native
button widget:

- iOS — rasterizes to a `UIImage` and passes it to
  `UIButton.setImage`.
- Android — rasterizes to a `Drawable` and uses it as the
  button's compound drawable.

The reason: native button widgets on these platforms expect
raster images for their icon slots. Going through the native
icon-on-button API gives you the platform's standard tint
behavior, hit testing, accessibility, and layout — exactly the
"this is a button" feel you want — at the cost of a one-time
rasterization per icon.

Standalone `Icon { ... }` invocations still go through the
vector path. The rasterization is a `Button`-only adaptation,
not a global mode.

You don't choose between the two. The framework picks based on
where the icon is being used.

## Reactive color

The default color is "inherit" — `currentColor` on web, label
color on native. To override, pass a closure:

```rust
ui! {
    icon(STAR).color(|| {
        if active.get() {
            Color::from("#ffd700")
        } else {
            Color::from("#999999")
        }
    })
}
```

The closure runs inside an Effect. Reading a signal subscribes
to it; when the signal changes, only this icon's color updates
on the backend. No re-render, no parent rebuild.

For static colors, wrap the literal: `.color(|| Color::from("#ff0000"))`.

## Stroke draw animations

Icons support **stroke-draw animations** — the path
progressively reveals itself from 0% to 100%. This works
natively on every backend:

- Web — `stroke-dasharray` + `stroke-dashoffset` with a CSS
  transition.
- iOS — `CAShapeLayer.strokeEnd` driven by `CABasicAnimation`.
- Android — `ObjectAnimator` on the path's `trimPathEnd`.

Two ways to invoke it.

### Reactive stroke progress

For programmatic control — a progress indicator that draws as a
download completes, an icon that animates in response to user
input:

```rust
let progress = signal!(0.0);

ui! {
    icon(LOGO).stroke(move || progress.get())
}

// Anywhere:
progress.set(0.75);    // logo path is now 75% drawn
```

The closure is wrapped in an Effect, same as `.color`. Setting
`progress` re-fires the Effect, which calls a backend
`update_icon_stroke` operation — at most one cheap native
animation update per change.

### Animate-in on mount

For a one-shot draw-on when the icon first appears:

```rust
use runtime_core::{StrokeAnimation, Easing};

ui! {
    icon(LOGO).draw_in(StrokeAnimation::new(600, Easing::EaseOut))
}
```

The animation fires once after the icon mounts. The
`StrokeAnimation` builder supports loops and autoreverses too:

```rust
StrokeAnimation::new(800, Easing::EaseInOut)
    .range(0.2, 0.8)    // partial reveal
    .looping()          // forever
    .reverse()          // back and forth
```

Platforms that lack stroke-animation support (none of the
shipped ones today) would ignore the animation and render the
icon fully drawn — the API degrades gracefully without app code
needing to check.

## Building your own icon suite

The shipped `icons-lucide` pack is one Rust crate. Rolling your
own is the same shape.

### The simplest possible suite

The minimum-viable icon pack is a Rust module of `IconData`
constants:

```rust
// my_icons/src/lib.rs
use runtime_core::{FillRule, IconData};

pub const STAR: IconData = IconData {
    view_box: (24, 24),
    paths: &["M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"],
    fill_rule: FillRule::NonZero,
};

pub const LOGO: IconData = IconData {
    view_box: (32, 32),
    paths: &[
        "M16 4l12 12-12 12L4 16z",       // outer diamond
        "M16 10l6 6-6 6-6-6z",            // inner diamond
    ],
    fill_rule: FillRule::NonZero,
};
```

That's it. Copy path data from any SVG, drop it into a
constant, use it like any other icon:

```rust
use runtime_core::icon;
use my_icons::{STAR, LOGO};

ui! {
    icon(STAR)
    icon(LOGO).color(|| theme.brand())
}
```

For a few custom icons, this manual form is enough. No build
step, no registry, no codegen.

### Generating from a folder of SVGs

For a real icon pack — dozens of icons, regular additions —
you want a `build.rs` to do the work. The pattern (used by
`icons-lucide`):

```
my-icons/
  Cargo.toml
  build.rs              # reads assets/*.svg, emits constants
  src/
    lib.rs              # include!(concat!(env!("OUT_DIR"), "/icons_generated.rs"))
  assets/
    star.svg
    logo.svg
    arrow-right.svg
    ...
```

The `build.rs` walks the `assets/` directory, parses each
SVG's path data and viewBox, and writes a Rust source file
into `OUT_DIR` with one constant per icon. The crate's
`src/lib.rs` `include!`s that generated file.

```rust
// build.rs (sketch)
fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = std::path::Path::new(&out_dir).join("icons_generated.rs");
    let mut out = String::new();

    for entry in std::fs::read_dir("assets").unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("svg") {
            continue;
        }

        let name = path.file_stem().unwrap().to_str().unwrap()
            .to_uppercase().replace('-', "_");
        let (vb, paths) = parse_svg(&path);

        out.push_str(&format!(
            "pub const {name}: ::runtime_core::IconData = \
             ::runtime_core::IconData {{ \
                view_box: ({}, {}), \
                paths: &[{}], \
                fill_rule: ::runtime_core::FillRule::NonZero, \
             }};\n",
            vb.0, vb.1,
            paths.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>().join(",")
        ));
    }

    std::fs::write(dest, out).unwrap();
    println!("cargo:rerun-if-changed=assets");
}
```

The actual `icons-lucide` build script handles edge cases
(multiple paths per icon, fill rules, viewBox variations) — read
its source as a working reference. But the shape is exactly the
above.

A new icon is then "drop the SVG in `assets/`, rebuild." The
generated constant uses the file's `SCREAMING_SNAKE_CASE`
name, so `assets/arrow-right.svg` becomes `ARROW_RIGHT`.

## Mixing packs

Multiple icon packs coexist without ceremony. They're just Rust
modules of constants:

```rust
use runtime_core::icon;
use icons_lucide::SEARCH;       // first-party pack
use my_icons::LOGO;             // custom pack
use heroicons::CHECK;           // third-party pack (hypothetical)

ui! {
    icon(SEARCH)
    icon(LOGO)
    icon(CHECK)
}
```

Each pack ships its own crate. Your app's `Cargo.toml` lists
the packs it pulls in:

```toml
[dependencies]
icons-lucide = "0.1"
my-icons = { path = "../my-icons" }
heroicons = "0.2"
```

There's no name collision worry across packs because each
pack's icons are in its own module path
(`icons_lucide::SEARCH` vs `heroicons::SEARCH`). Use one pack
for most icons, supplement with another for icons your primary
pack doesn't include, add your own for brand assets — every
icon is just a `&'static IconData`, identical at the use site
regardless of where it came from.

## Tree-shaking

Because `IconData` is `const`, the linker can drop any icon
your code doesn't reference. In release builds (LTO on), an
app that uses three Lucide icons ships only those three icons'
path strings — not the whole 1000-icon pack.

This makes "vendor a big icon pack so you have everything
available" a reasonable strategy. Unused icons don't show up in
the WASM bundle, the iOS binary, or the Android APK.

You can verify with `twiggy` (web) or the platform's binary
inspector if you're curious about what survived.

## Where to read more

- [Primitives — Icon](#) — the primitive entry with constructor
  variants.
- [Styles](#) — using theme colors for icon tints.
- [Reactivity](#) — what's happening when `.color` or `.stroke`
  takes a closure.
- [icons-lucide source](#) — the reference implementation for
  building your own pack from SVGs.
