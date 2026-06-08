//! Render a small UI headlessly (no window) and save it as a PNG.
//!
//!   cargo run -p render-wgpu --features headless --example headless_screenshot
//!
//! Writes `headless-demo.png` in the current directory. Demonstrates the
//! offscreen screenshot path: real wgpu renderer + shaders, real text
//! (bundled font), no surface, no window.

#[cfg(not(feature = "headless"))]
fn main() {
    eprintln!("re-run with `--features headless`");
}

#[cfg(feature = "headless")]
fn main() {
    use std::rc::Rc;
    use render_wgpu::headless::Screenshotter;
    use runtime_core::{
        Color, Element, IntoStyleSource, Length, SafeAreaSides, StyleApplication, StyleRules,
        StyleSheet, TextSource, Tokenized,
    };

    fn styled_view(children: Vec<Element>, build: impl FnOnce(&mut StyleRules)) -> Element {
        let sheet = Rc::new(StyleSheet::r#static({
            let mut r = StyleRules::default();
            build(&mut r);
            r
        }));
        Element::View {
            children,
            style: Some(StyleApplication::new(sheet).into_style_source()),
            ref_fill: None,
            safe_area_sides: SafeAreaSides::NONE,
            on_touch: None,
            is_container: false,
            accessibility: Default::default(),
        }
    }

    fn label(text: &str, hex: &'static str, size: f32) -> Element {
        let sheet = Rc::new(StyleSheet::r#static({
            let mut r = StyleRules::default();
            r.font_size = Some(Tokenized::Literal(Length::Px(size)));
            r
        }));
        let style = StyleApplication::new(sheet).override_color(Color(hex.to_string()));
        Element::Text {
            source: TextSource::Static(text.to_string()),
            style: Some(style.into_style_source()),
            ref_fill: None,
            accessibility: Default::default(),
        }
    }

    let app = || {
        // Dark full-bleed background with a lighter card containing text.
        styled_view(
            vec![styled_view(
                vec![
                    label("Idealyst — headless render", "#e8ecf4", 22.0),
                    label("rasterized with no window", "#8a93a6", 15.0),
                ],
                |r| {
                    r.background = Some(Tokenized::Literal(Color("#1b2030".into())));
                    r.padding_top = Some(Tokenized::Literal(Length::Px(28.0)));
                    r.padding_bottom = Some(Tokenized::Literal(Length::Px(28.0)));
                    r.padding_left = Some(Tokenized::Literal(Length::Px(32.0)));
                    r.padding_right = Some(Tokenized::Literal(Length::Px(32.0)));
                    r.margin_top = Some(Tokenized::Literal(Length::Px(48.0)));
                    r.margin_left = Some(Tokenized::Literal(Length::Px(40.0)));
                },
            )],
            |r| {
                r.width = Some(Tokenized::Literal(Length::Percent(100.0)));
                r.height = Some(Tokenized::Literal(Length::Percent(100.0)));
                r.background = Some(Tokenized::Literal(Color("#0c0e15".into())));
            },
        )
    };

    let mut shot = match Screenshotter::new(560, 240) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no wgpu adapter available: {e}");
            return;
        }
    };
    if shot.software {
        eprintln!("(using software adapter)");
    }
    shot.mount(app);
    let png = shot.capture_png().expect("capture");
    std::fs::write("headless-demo.png", &png).expect("write png");
    println!(
        "wrote headless-demo.png ({} bytes, {}x{})",
        png.len(),
        shot.size().0,
        shot.size().1
    );
}
