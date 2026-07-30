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
    use render_wgpu::headless::Screenshotter;
    use runtime_shared::{Color, Length, StyleRules, Tokenized};
    use runtime_vocabulary::builders::IntoSceneElement;
    use runtime_vocabulary::{text, view};

    fn rules(build: impl FnOnce(&mut StyleRules)) -> StyleRules {
        let mut r = StyleRules::default();
        build(&mut r);
        r
    }

    // Dark full-bleed background with a lighter card containing text.
    let app = || {
        view()
            .child(
                view()
                    .child(
                        text()
                            .content("Idealyst — headless render")
                            .style(rules(|r| {
                                r.font_size = Some(Tokenized::Literal(Length::Px(22.0)));
                                r.color = Some(Tokenized::Literal(Color("#e8ecf4".into())));
                            })),
                    )
                    .child(
                        text()
                            .content("rasterized with no window")
                            .style(rules(|r| {
                                r.font_size = Some(Tokenized::Literal(Length::Px(15.0)));
                                r.color = Some(Tokenized::Literal(Color("#8a93a6".into())));
                            })),
                    )
                    .style(rules(|r| {
                        r.background = Some(Tokenized::Literal(Color("#1b2030".into())));
                        r.padding_top = Some(Tokenized::Literal(Length::Px(28.0)));
                        r.padding_bottom = Some(Tokenized::Literal(Length::Px(28.0)));
                        r.padding_left = Some(Tokenized::Literal(Length::Px(32.0)));
                        r.padding_right = Some(Tokenized::Literal(Length::Px(32.0)));
                        r.margin_top = Some(Tokenized::Literal(Length::Px(48.0)));
                        r.margin_left = Some(Tokenized::Literal(Length::Px(40.0)));
                    })),
            )
            .style(rules(|r| {
                r.width = Some(Tokenized::Literal(Length::Percent(100.0)));
                r.height = Some(Tokenized::Literal(Length::Percent(100.0)));
                r.background = Some(Tokenized::Literal(Color("#0c0e15".into())));
            }))
            .into_scene_element()
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
    // `_app` must outlive the capture — dropping it unmounts the tree.
    let _app = render_wgpu::newcore::start(shot.backend(), |_| {}, app);
    let png = shot.capture_png().expect("capture");
    std::fs::write("headless-demo.png", &png).expect("write png");
    println!(
        "wrote headless-demo.png ({} bytes, {}x{})",
        png.len(),
        shot.size().0,
        shot.size().1
    );
}
