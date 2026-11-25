//! A `transform` list's translates against real UIKit: px and percent entries
//! add up, against the view's laid-out size, before and after its first
//! layout.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator and
//! run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test percent_translate --no-run
//! xcrun simctl spawn booted target/aarch64-apple-ios-sim/debug/deps/percent_translate-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::rc::Rc;

    use backend_ios::{IosBackend, IosNode};
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use objc2_ui_kit::UIView;
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Length, Position, StyleRules, Transform};
    use runtime_vocabulary::caps::{LifecycleOps, StyleOps, ViewOps};

    const W: f32 = 200.0;
    const H: f32 = 100.0;

    /// REGRESSION: a sliding card's `[translateX(100%), translateX(-24px)]`
    /// lost its -24: the resolved percent was written over the px.
    fn regression_percent_and_px_translates_add_up(mtm: MainThreadMarker) {
        let px = |v: f32| Some(runtime_shared::Tokenized::Literal(Length::Px(v)));
        let mut backend = IosBackend::new(mtm);
        let frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: 400.0, height: 300.0 },
        };
        let host: Retained<UIView> =
            unsafe { msg_send_id![mtm.alloc::<UIView>(), initWithFrame: frame] };
        backend.set_host_root(host);
        let a11y = AccessibilityProps::default();
        let style = |transform: Vec<Transform>| {
            Rc::new(StyleRules {
                position: Some(Position::Absolute),
                top: px(0.0),
                left: px(0.0),
                width: px(W),
                height: px(H),
                transform: Some(transform),
                ..Default::default()
            })
        };
        let x = |l| Transform::TranslateX(l);
        let mut root = backend.create_view(&a11y);
        backend.apply_style(
            &root,
            &Rc::new(StyleRules { width: px(400.0), height: px(300.0), ..Default::default() }),
        );
        let card = backend.create_view(&a11y);
        backend.apply_style(
            &card,
            &style(vec![
                x(Length::Percent(100.0)),
                x(Length::Px(-24.0)),
                Transform::TranslateY(Length::Px(5.0)),
                Transform::TranslateY(Length::Percent(-50.0)),
            ]),
        );
        backend.insert(&mut root, card.clone());
        backend.finish(root);
        backend.run_layout();
        assert_eq!(painted(&card), (176.0, -45.0), "after the first layout");

        backend.apply_style(&card, &style(vec![x(Length::Px(-8.0)), x(Length::Percent(50.0))]));
        assert_eq!(painted(&card), (92.0, 0.0), "on a restyle, in the other order");
        backend.apply_style(&card, &style(vec![x(Length::Px(7.0)), x(Length::Px(3.0))]));
        assert_eq!(painted(&card), (10.0, 0.0), "px alone, the percent dropped");
        backend.run_layout();
        assert_eq!(painted(&card), (10.0, 0.0), "and a layout pass leaves it");
    }

    /// Where the view's box paints in its parent (`frame` includes the
    /// transform).
    fn painted(node: &IosNode) -> (f64, f64) {
        let v: &UIView = node.as_view();
        let f: CGRect = unsafe { msg_send![v, frame] };
        (f.origin.x, f.origin.y)
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        regression_percent_and_px_translates_add_up(mtm);
        println!("percent_translate: 1 passed");
    }
}
