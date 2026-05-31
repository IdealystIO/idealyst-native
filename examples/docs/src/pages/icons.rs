//! Icons page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "icons",
    title = "Icons",
    category = Reference,
    description = "Vector icons as const data — tree-shaken, theme-tintable, animatable.",
    related = ["primitives", "styles", "reactivity", "backends"],
    concepts = [IconData, IconRegistry, StrokeAnimation],

    section(heading = "Overview") {
        p("Icons in Idealyst are vector paths stored as ", code("&'static IconData"),
          " constants. Backends render them natively — inline SVG on web, ",
          code("CAShapeLayer"), " on iOS, ", code("VectorDrawable"),
          " on Android. Because the data is ", code("const"),
          ", only the icons your code actually references end up in the final binary; \
           LTO strips the rest."),
        p("You're not stuck with a particular icon pack. The framework ships \
           Lucide as a default, but icons are just data, and mixing multiple packs \
           (or rolling your own) is a matter of importing their constants — there's \
           no registry to configure, no central list to maintain."),
    },

    section(heading = "Using an icon") {
        code(rust, r##"
            use runtime_core::icon;
            use icons_lucide::{SEARCH, MENU};

            ui! {
                icon(SEARCH)
                icon(MENU).color(|| theme.primary())
            }
        "##),
        p("That's the entire surface for the simple case. ", code("icon(...)"),
          " takes an ", code("IconData"), " constant and returns a primitive you \
           can drop into any tree. The chainable methods (", code(".color"), ", ",
          code(".stroke"), ", ", code(".draw_in"),
          ") add reactive behavior; we'll cover them below."),
    },

    section(heading = "What IconData actually is") {
        p("The ", code("IconData"), " struct is tiny and const-constructible:"),
        code(rust, r##"
            pub struct IconData {
                pub view_box: (u16, u16),
                pub paths: &'static [&'static str],
                pub fill_rule: FillRule,
                pub filled: bool,
            }
        "##),
        list(
            [code("view_box"), " — the SVG viewBox dimensions (", code("(24, 24)"),
             " for most icon sets)."],
            [code("paths"), " — one or more SVG path ", code("d"),
             " strings. Multiple paths let you build multi-region icons (an outline \
              plus a filled bowl, for example)."],
            [code("fill_rule"), " — ", code("NonZero"), " (the SVG default) or ",
             code("EvenOdd"), "."],
            [code("filled"), " — ", code("false"),
             " (the default) strokes the paths with the icon color, leaving the \
              interior transparent — the outlined Lucide style. ", code("true"),
             " fills the paths with the icon color (using ", code("fill_rule"),
             ") and disables the stroke, for solid/silhouette glyphs."],
        ),
        p("Because the whole thing is ", code("const"),
          ", an icon pack is a Rust module of static constants. There's no init \
           code, no runtime registry, no per-icon allocation. The icons live in ",
          code(".rodata"), " alongside your string literals."),
    },

    section(heading = "How backends render") {
        p("The Icon primitive maps to native vector primitives, not raster images:"),
        list(
            ["Web — inline ", code("<svg>"), ". Each path becomes a ", code("<path>"),
             " element. Scales crisply at any size; theme-tintable via the ",
             code("color"), " override or CSS ", code("currentColor"), "."],
            ["iOS — a ", code("CAShapeLayer"),
             " per path. Vector all the way down; no anti-aliasing artifacts at the \
              icon's natural rendering scale."],
            ["Android — a ", code("VectorDrawable"),
             ". Same story — the platform rasterizes only at draw time, at the \
              exact display density."],
        ),
        p("This gives you sharp icons at every backbone size without shipping \
           multiple raster assets."),
    },

    section(heading = "One backend exception worth knowing") {
        p("Buttons are different. When you pass an icon to a ", code("Button"), " as ",
          code("leading_icon"), " or ", code("trailing_icon"),
          ", iOS and Android backends rasterize the SVG to a bitmap before handing \
           it to the native button widget:"),
        list(
            ["iOS — rasterizes to a ", code("UIImage"), " and passes it to ",
             code("UIButton.setImage"), "."],
            ["Android — rasterizes to a ", code("Drawable"),
             " and uses it as the button's compound drawable."],
        ),
        p("The reason: native button widgets on these platforms expect raster images \
           for their icon slots. Going through the native icon-on-button API gives \
           you the platform's standard tint behavior, hit testing, accessibility, \
           and layout — exactly the \"this is a button\" feel you want — at the cost \
           of a one-time rasterization per icon."),
        p("Standalone ", code("icon { ... }"),
          " invocations still go through the vector path. The rasterization is a ",
          code("Button"), "-only adaptation, not a global mode."),
        p("You don't choose between the two. The framework picks based on where the \
           icon is being used."),
    },

    section(heading = "Reactive color") {
        p("The default color is \"inherit\" — ", code("currentColor"),
          " on web, label color on native. To override, pass a closure:"),
        code(rust, r##"
            ui! {
                icon(STAR).color(|| {
                    if active.get() {
                        Color::from("#ffd700")
                    } else {
                        Color::from("#999999")
                    }
                })
            }
        "##),
        p("The closure runs inside an Effect. Reading a signal subscribes to it; \
           when the signal changes, only this icon's color updates on the backend. \
           No re-render, no parent rebuild."),
        p("For static colors, wrap the literal: ",
          code(".color(|| Color::from(\"#ff0000\"))"), "."),
    },

    section(heading = "Stroke draw animations") {
        p("Icons support stroke-draw animations — the path progressively reveals \
           itself from 0% to 100%. This works natively on every backend:"),
        list(
            ["Web — ", code("stroke-dasharray"), " + ", code("stroke-dashoffset"),
             " with a CSS transition."],
            ["iOS — ", code("CAShapeLayer.strokeEnd"), " driven by ",
             code("CABasicAnimation"), "."],
            ["Android — ", code("ObjectAnimator"), " on the path's ",
             code("trimPathEnd"), "."],
        ),
        p("Two ways to invoke it."),
    },

    section(heading = "Reactive stroke progress") {
        p("For programmatic control — a progress indicator that draws as a download \
           completes, an icon that animates in response to user input:"),
        code(rust, r##"
            let progress = signal!(0.0);

            ui! {
                icon(LOGO).stroke(move || progress.get())
            }

            // Anywhere:
            progress.set(0.75);    // logo path is now 75% drawn
        "##),
        p("The closure is wrapped in an Effect, same as ", code(".color"),
          ". Setting ", code("progress"), " re-fires the Effect, which calls a backend ",
          code("update_icon_stroke"),
          " operation — at most one cheap native animation update per change."),
    },

    section(heading = "Animate-in on mount") {
        p("For a one-shot draw-on when the icon first appears:"),
        code(rust, r##"
            use runtime_core::{StrokeAnimation, Easing};

            ui! {
                icon(LOGO).draw_in(StrokeAnimation::new(600, Easing::EaseOut))
            }
        "##),
        p("The animation fires once after the icon mounts. The ",
          code("StrokeAnimation"), " builder supports loops and autoreverses too:"),
        code(rust, r##"
            StrokeAnimation::new(800, Easing::EaseInOut)
                .range(0.2, 0.8)    // partial reveal
                .looping()          // forever
                .reverse()          // back and forth
        "##),
        p("Platforms that lack stroke-animation support (none of the shipped ones \
           today) would ignore the animation and render the icon fully drawn — the \
           API degrades gracefully without app code needing to check."),
    },

    section(heading = "Building your own icon suite") {
        p("The shipped ", code("icons-lucide"),
          " pack is one Rust crate. Rolling your own is the same shape."),
    },

    section(heading = "The simplest possible suite") {
        p("The minimum-viable icon pack is a Rust module of ", code("IconData"),
          " constants:"),
        code(rust, r##"
            // my_icons/src/lib.rs
            use runtime_core::{FillRule, IconData};

            pub const STAR: IconData = IconData {
                view_box: (24, 24),
                paths: &["M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"],
                fill_rule: FillRule::NonZero,
                filled: false,         // outlined (stroke); set true for a solid star
            };

            pub const LOGO: IconData = IconData {
                view_box: (32, 32),
                paths: &[
                    "M16 4l12 12-12 12L4 16z",       // outer diamond
                    "M16 10l6 6-6 6-6-6z",            // inner diamond
                ],
                fill_rule: FillRule::NonZero,
                filled: false,
            };
        "##),
        p("That's it. Copy path data from any SVG, drop it into a constant, use it \
           like any other icon:"),
        code(rust, r##"
            use runtime_core::icon;
            use my_icons::{STAR, LOGO};

            ui! {
                icon(STAR)
                icon(LOGO).color(|| theme.brand())
            }
        "##),
        p("For a few custom icons, this manual form is enough. No build step, no \
           registry, no codegen."),
    },

    section(heading = "Generating from a folder of SVGs") {
        p("For a real icon pack — dozens of icons, regular additions — you want a ",
          code("build.rs"), " to do the work. The pattern (used by ",
          code("icons-lucide"), "):"),
        code(text, r##"
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
        "##),
        p("The ", code("build.rs"),
          " walks the ", code("assets/"),
          " directory, parses each SVG's path data and viewBox, and writes a Rust \
           source file into ", code("OUT_DIR"), " with one constant per icon. The crate's ",
          code("src/lib.rs"), " ", code("include!"), "s that generated file."),
        code(rust, r##"
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
                            filled: false, \
                         }};\n",
                        vb.0, vb.1,
                        paths.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>().join(",")
                    ));
                }

                std::fs::write(dest, out).unwrap();
                println!("cargo:rerun-if-changed=assets");
            }
        "##),
        p("The actual ", code("icons-lucide"),
          " build script handles edge cases (multiple paths per icon, fill rules, \
           viewBox variations) — read its source as a working reference. But the \
           shape is exactly the above."),
        p("A new icon is then \"drop the SVG in ", code("assets/"),
          ", rebuild.\" The generated constant uses the file's ",
          code("SCREAMING_SNAKE_CASE"), " name, so ", code("assets/arrow-right.svg"),
          " becomes ", code("ARROW_RIGHT"), "."),
    },

    section(heading = "Mixing packs") {
        p("Multiple icon packs coexist without ceremony. They're just Rust modules \
           of constants:"),
        code(rust, r##"
            use runtime_core::icon;
            use icons_lucide::SEARCH;       // first-party pack
            use my_icons::LOGO;             // custom pack
            use heroicons::CHECK;           // third-party pack (hypothetical)

            ui! {
                icon(SEARCH)
                icon(LOGO)
                icon(CHECK)
            }
        "##),
        p("Each pack ships its own crate. Your app's ", code("Cargo.toml"),
          " lists the packs it pulls in:"),
        code(text, r##"
            [dependencies]
            icons-lucide = "0.1"
            my-icons = { path = "../my-icons" }
            heroicons = "0.2"
        "##),
        p("There's no name collision worry across packs because each pack's icons \
           are in its own module path (", code("icons_lucide::SEARCH"), " vs ",
          code("heroicons::SEARCH"),
          "). Use one pack for most icons, supplement with another for icons your \
           primary pack doesn't include, add your own for brand assets — every \
           icon is just a ", code("&'static IconData"),
          ", identical at the use site regardless of where it came from."),
    },

    section(heading = "Tree-shaking") {
        p("Because ", code("IconData"), " is ", code("const"),
          ", the linker can drop any icon your code doesn't reference. In release \
           builds (LTO on), an app that uses three Lucide icons ships only those \
           three icons' path strings — not the whole 1000-icon pack."),
        p("This makes \"vendor a big icon pack so you have everything available\" a \
           reasonable strategy. Unused icons don't show up in the WASM bundle, the \
           iOS binary, or the Android APK."),
        p("You can verify with ", code("twiggy"),
          " (web) or the platform's binary inspector if you're curious about what \
           survived."),
    },

    section(heading = "Where to read more") {
        list(
            ["Primitives — Icon — the primitive entry with constructor variants."],
            ["Styles — using theme colors for icon tints."],
            ["Reactivity — what's happening when ", code(".color"), " or ",
             code(".stroke"), " takes a closure."],
            [code("icons-lucide"),
             " source — the reference implementation for building your own pack \
              from SVGs."],
        ),
    },
}
