//! An app rooted in several nodes, or in a reactive region, hands UIKit a
//! layout-transparent anchor as its root.
//!
//! REGRESSION: the anchor had no box, so the pass that lays out every root
//! computed each child against the whole window, and several root nodes sat
//! on top of one another. They stack; a lone one fills the window.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator
//! and run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test anchor_root --no-run
//! xcrun simctl spawn booted target/aarch64-apple-ios-sim/debug/deps/anchor_root-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::cell::RefCell;
    use std::rc::Rc;

    use backend_ios::{IosBackend, IosNode};
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Length, StyleRules};
    use runtime_vocabulary::caps::{LifecycleOps, StyleOps, ViewOps};

    const WIN_W: f64 = 402.0;
    const WIN_H: f64 = 874.0;

    fn frames(mtm: MainThreadMarker, heights: &[Option<f32>]) -> Vec<(f64, f64, f64, f64)> {
        let backend = Rc::new(RefCell::new(IosBackend::new(mtm)));
        backend_ios::install_global_self(Rc::downgrade(&backend));
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: WIN_W, height: WIN_H } };
        let window: Retained<UIWindow> = unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame] };
        let host: Retained<UIView> = unsafe { msg_send_id![mtm.alloc::<UIView>(), initWithFrame: frame] };
        unsafe { window.addSubview(&host) };
        let mut b = backend.borrow_mut();
        b.set_host_root(host);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_anchor();
        let children: Vec<IosNode> = heights
            .iter()
            .map(|h| {
                let v = b.create_view(&a11y);
                let rules = StyleRules { height: h.map(|h| Length::Px(h).into()), ..Default::default() };
                b.apply_style(&v, &Rc::new(rules));
                b.insert(&mut root, v.clone());
                v
            })
            .collect();
        b.finish(root);
        b.run_layout();
        children
            .iter()
            .map(|c| {
                let f: CGRect = unsafe { msg_send![c.as_view(), frame] };
                (f.origin.x, f.origin.y, f.size.width, f.size.height)
            })
            .collect()
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_ios::install_scheduler();
        assert_eq!(
            frames(mtm, &[Some(40.0), Some(50.0)]),
            [(0.0, 0.0, WIN_W, 40.0), (0.0, 40.0, WIN_W, 50.0)],
            "several root nodes stack"
        );
        assert_eq!(frames(mtm, &[None]), [(0.0, 0.0, WIN_W, WIN_H)], "a lone root node fills the window");
        println!("anchor_root: 2 passed");
    }
}
