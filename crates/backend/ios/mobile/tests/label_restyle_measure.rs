//! A label restyled to a heavier weight is re-measured, on a real UILabel.
//!
//! REGRESSION: the sidebar's active row restyles its label from regular to
//! semibold. The weight changes the glyphs but not the layout `Style`, so
//! the label kept its regular width and the semibold text wrapped its last
//! word onto a clipped second line ("Cost" for "Cost items").
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator and
//! run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test label_restyle_measure --no-run
//! xcrun simctl spawn <udid> target/aarch64-apple-ios-sim/debug/deps/label_restyle_measure-<hash>
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

    use backend_ios::IosBackend;
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{AlignItems, FlexDirection, FontWeight, Length, StyleRules};
    use runtime_vocabulary::caps::{IntrospectionOps, LifecycleOps, StyleOps, TextOps, ViewOps};

    fn label_style(weight: FontWeight) -> Rc<StyleRules> {
        let mut r = StyleRules::default();
        r.font_size = Some(Length::Px(15.0).into());
        r.font_weight = Some(weight);
        Rc::new(r)
    }

    pub fn run() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = IosBackend::new(mtm);
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 200.0 } };
        let window: Retained<UIWindow> = unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame] };
        let _: () = unsafe { msg_send![&window, makeKeyAndVisible] };
        let host: Retained<UIView> = unsafe { msg_send_id![mtm.alloc::<UIView>(), initWithFrame: frame] };
        unsafe { window.addSubview(&host) };
        backend.set_host_root(host.clone());

        let a11y = AccessibilityProps::default();
        let mut root = backend.create_view(&a11y);
        let mut row = StyleRules::default();
        row.width = Some(Length::Px(300.0).into());
        row.flex_direction = Some(FlexDirection::Row);
        row.align_items = Some(AlignItems::FlexStart);
        backend.apply_style(&root, &Rc::new(row));
        let label = backend.create_text("Cost items", &a11y);
        backend.apply_style(&label, &label_style(FontWeight::Normal));
        backend.insert(&mut root, label.clone());
        backend.finish(root);
        backend.run_layout();
        let regular = backend.frame(&label).expect("label laid out");

        backend.apply_style(&label, &label_style(FontWeight::Bold));
        backend.run_layout();
        let bold = backend.frame(&label).expect("label laid out");
        let probe = CGSize { width: f64::MAX, height: f64::MAX };
        let fit: CGSize = unsafe { msg_send![label.as_view(), sizeThatFits: probe] };

        println!(
            "[label_restyle_measure] regular {}x{}, bold {}x{}, bold glyphs {}x{}",
            regular.width, regular.height, bold.width, bold.height, fit.width, fit.height
        );
        assert!(bold.width > regular.width, "the bold label is wider than the regular one");
        assert!(
            bold.width as f64 + 0.5 >= fit.width,
            "the bold label is laid out as wide as its glyphs ({} < {})",
            bold.width,
            fit.width
        );
        assert!((bold.height - regular.height).abs() < 0.5, "still one line");
        println!("label_restyle_measure: ok");
    }
}
