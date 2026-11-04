//! `IntrospectionOps::introspect_native` on real UIKit objects.
//!
//! REGRESSION (system check, iOS): the backend left `introspect_native` at its
//! default, so the conformance checks that read the widget (a text field's
//! written value, a text area's text, a dropped opacity) skipped here and a
//! reset that only a host-side policy test had seen went unproven on the widget.
//!
//! Every value is asserted against what the view holds after going through the
//! style path or UIKit's own setters: the layer's background, radius and border,
//! the view's alpha, a field's string and font. A second style that drops a prop
//! must read back as the rest value, which the style rules cannot say.
//!
//! REGRESSION (`is_shown`, iOS): it multiplied `alpha` by the layer's `opacity`,
//! which is the same field, so a view at 0.05 counted as fully transparent.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator and run
//! the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test native_readback --no-run
//! xcrun simctl spawn <udid> target/aarch64-apple-ios-sim/debug/deps/native_readback-<hash>
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
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::introspect::{keys, NativeNode, NativeValue};
    use runtime_shared::{Color, Length, StyleRules};
    use runtime_vocabulary::caps::{IntrospectionOps, StyleOps, TextInputOps, TextOps, ViewOps};

    fn host(mtm: MainThreadMarker) -> (Retained<UIWindow>, Retained<UIView>) {
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 800.0 } };
        let window: Retained<UIWindow> = unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame] };
        let _: () = unsafe { msg_send![&window, makeKeyAndVisible] };
        let host: Retained<UIView> = unsafe { msg_send_id![mtm.alloc::<UIView>(), initWithFrame: frame] };
        unsafe { window.addSubview(&host) };
        (window, host)
    }

    fn place(host: &UIView, node: &IosNode) {
        let v: &UIView = node.as_view();
        let frame = CGRect { origin: CGPoint { x: 10.0, y: 20.0 }, size: CGSize { width: 200.0, height: 40.0 } };
        let _: () = unsafe { msg_send![v, setFrame: frame] };
        let _: () = unsafe { msg_send![host, addSubview: v] };
    }

    fn number(n: &NativeNode, key: &str) -> f32 {
        match n.props.get(key) {
            Some(NativeValue::Number(v)) | Some(NativeValue::Length(v)) => *v,
            other => panic!("{key}: {other:?}"),
        }
    }

    fn text(n: &NativeNode) -> String {
        match n.props.get(keys::TEXT) {
            Some(NativeValue::Text(t)) => t.clone(),
            other => panic!("text: {other:?}"),
        }
    }

    fn regression_ios_no_native_readback(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let (_window, host) = host(mtm);
        let a11y = AccessibilityProps::default();

        assert!(backend.supports_native_introspection());

        // Off-window: no geometry, so no read yet.
        let view = backend.create_view(&a11y);
        assert!(backend.introspect_native(&view).is_none(), "a view outside a window reads as not laid out");
        place(&host, &view);

        let mut rules = StyleRules::default();
        rules.opacity = Some(0.5.into());
        rules.background = Some(Color("#336699".into()).into());
        rules.border_top_left_radius = Some(Length::Px(8.0).into());
        rules.border_top_right_radius = Some(Length::Px(8.0).into());
        rules.border_bottom_left_radius = Some(Length::Px(8.0).into());
        rules.border_bottom_right_radius = Some(Length::Px(8.0).into());
        for side in [
            &mut rules.border_top_width,
            &mut rules.border_right_width,
            &mut rules.border_bottom_width,
            &mut rules.border_left_width,
        ] {
            *side = Some(Length::Px(2.0).into());
        }
        rules.border_top_color = Some(Color("#ff0000".into()).into());
        backend.apply_style(&view, &Rc::new(rules));

        let n = backend.introspect_native(&view).expect("a view in a window reads");
        assert!(n.class.contains("View"), "class {}", n.class);
        assert_eq!((n.frame.x, n.frame.y, n.frame.width, n.frame.height), (10.0, 20.0, 200.0, 40.0));
        assert_eq!(number(&n, keys::OPACITY), 0.5);
        assert_eq!(number(&n, keys::CORNER_RADIUS), 8.0);
        assert_eq!(number(&n, keys::BORDER_WIDTH), 2.0);
        assert_eq!(n.props.get(keys::BORDER_STYLE), Some(&NativeValue::Text("solid".into())));
        assert_eq!(n.props.get(keys::HIDDEN), Some(&NativeValue::Flag(false)));
        match n.props.get(keys::BACKGROUND_COLOR) {
            Some(NativeValue::Color(c)) => {
                assert!((c[0] - 0.2).abs() < 0.02 && (c[1] - 0.4).abs() < 0.02 && (c[2] - 0.6).abs() < 0.02, "{c:?}")
            }
            other => panic!("background: {other:?}"),
        }

        // The style drops every prop: the widget goes back, and the read says so.
        backend.apply_style(&view, &Rc::new(StyleRules::default()));
        let n = backend.introspect_native(&view).unwrap();
        assert_eq!(number(&n, keys::OPACITY), 1.0, "a dropped opacity reads opaque");
        assert!(n.props.get(keys::CORNER_RADIUS).is_none(), "{:?}", n.props);
        assert!(n.props.get(keys::BORDER_WIDTH).is_none(), "{:?}", n.props);

        // Text controls: their string (empty included) and resolved font.
        let label = backend.create_text("Label", &a11y);
        place(&host, &label);
        let mut st = StyleRules::default();
        st.font_size = Some(Length::Px(18.0).into());
        backend.apply_style(&label, &Rc::new(st));
        let n = backend.introspect_native(&label).unwrap();
        assert_eq!(text(&n), "Label");
        assert_eq!(number(&n, keys::FONT_SIZE), 18.0);
        assert_eq!(number(&n, keys::FONT_WEIGHT), 400.0);
        assert!(matches!(n.props.get(keys::FONT_FAMILY), Some(NativeValue::Text(f)) if !f.is_empty()));
        assert!(matches!(n.props.get(keys::TEXT_COLOR), Some(NativeValue::Color(_))));
        backend.update_text(&label, "Changed");
        assert_eq!(text(&backend.introspect_native(&label).unwrap()), "Changed");

        let field = backend.create_text_input("seed", None, Rc::new(|_| {}), None, None, false, &a11y);
        place(&host, &field);
        assert_eq!(text(&backend.introspect_native(&field).unwrap()), "seed");
        backend.update_text_input_value(&field, "");
        assert_eq!(text(&backend.introspect_native(&field).unwrap()), "", "an empty field shows \"\"");

        let area = backend.create_text_area("", None, true, None, None, Rc::new(|_| {}), None, &a11y);
        place(&host, &area);
        backend.update_text_area_value(&area, "two\nlines");
        assert_eq!(text(&backend.introspect_native(&area).unwrap()), "two\nlines");

        // A framework child is its own element; a native sub-object isn't.
        let mut parent = backend.create_view(&a11y);
        place(&host, &parent);
        let child = backend.create_view(&a11y);
        backend.insert(&mut parent, child);
        let n = backend.introspect_native(&parent).unwrap();
        assert!(n.children.is_empty(), "framework child pruned at the boundary: {:?}", n.children);
    }

    fn regression_ios_is_shown_squared_the_opacity(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let (_window, host) = host(mtm);
        let view = backend.create_view(&AccessibilityProps::default());
        place(&host, &view);
        let _: () = unsafe { msg_send![view.as_view(), setAlpha: 0.05_f64] };
        assert_eq!(backend.is_shown(&view), Some(true), "0.05 is faint, not invisible");
    }

    pub fn run() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        regression_ios_no_native_readback(mtm);
        regression_ios_is_shown_squared_the_opacity(mtm);
        println!("native_readback: ok");
    }
}
