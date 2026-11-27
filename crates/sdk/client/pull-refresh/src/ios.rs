//! iOS: a `UIRefreshControl` set as `refreshControl` on the first vertical
//! `UIScrollView` in the child's tree.
//!
//! The wrapper itself is the passthrough anchor (a Taffy contents node), so
//! the child lays out as if unwrapped; the control rides the scroll view
//! the child already made. UIKit owns the spinner, the snap, the haptic and
//! the held-down content inset while refreshing. The windowing of every
//! scroller here reads `contentOffset` in content space — the collection
//! view behind `flat_list` is windowed by UIKit itself, and `virtual_grid`'s
//! `GridMetrics::visible_window` clamps a negative offset (the inset the
//! control adds while refreshing) to row 0 and ends the window at
//! `offset + viewport` — so rows are neither lost nor misplaced while the
//! content is held down.

use std::cell::RefCell;
use std::rc::Rc;

use backend_ios::{IosBackend, IosNode};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{class, declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, CGPoint, CGRect, MainThreadMarker};
use runtime_scene::{Element, MountCx};
use runtime_shared::color::Rgba;
use runtime_vocabulary::style_attach::on_teardown;

use crate::binding::{self, NativeRefresh};
use crate::policy::{self, Probe};
use crate::RefreshablePrim;

/// `UIControlEventValueChanged`.
const VALUE_CHANGED: usize = 1 << 12;
/// `UICollectionViewScrollDirectionHorizontal`.
const SCROLL_DIRECTION_HORIZONTAL: isize = 1;

/// `UIEdgeInsets`, for `adjustedContentInset`.
#[repr(C)]
#[derive(Clone, Copy)]
struct EdgeInsets {
    top: CGFloat,
    left: CGFloat,
    bottom: CGFloat,
    right: CGFloat,
}

unsafe impl objc2::Encode for EdgeInsets {
    const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
        "UIEdgeInsets",
        &[CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING],
    );
}

pub(crate) struct TargetIvars {
    pulled: RefCell<Option<Rc<dyn Fn()>>>,
}

declare_class!(
    /// The refresh control's value-changed target. UIControl does not
    /// retain its targets, so the teardown closure holds this.
    struct RefreshTarget;

    unsafe impl ClassType for RefreshTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystPullRefreshTarget";
    }

    impl DeclaredClass for RefreshTarget {
        type Ivars = TargetIvars;
    }

    unsafe impl RefreshTarget {
        #[method(refreshPulled:)]
        fn refresh_pulled(&self, _sender: &AnyObject) {
            // Cloned out first: `pulled` runs author code, which may
            // tear this screen down and clear the slot.
            let pulled = self.ivars().pulled.borrow().clone();
            if let Some(pulled) = pulled {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pulled())).is_err() {
                    // Unwinding through an Obj-C frame is undefined
                    // behaviour; crash loud instead.
                    std::process::abort();
                }
            }
        }
    }
);

impl RefreshTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(TargetIvars { pulled: RefCell::new(None) });
        unsafe { msg_send_id![super(this), init] }
    }
}

fn is_kind(view: &AnyObject, class: &objc2::runtime::AnyClass) -> bool {
    unsafe { msg_send![view, isKindOfClass: class] }
}

/// How the scroller search treats one `UIView`.
fn probe(view: &Retained<AnyObject>) -> Probe {
    if !is_kind(view, class!(UIScrollView)) {
        return Probe::Container;
    }
    // A `text_area` / selectable text is a `UITextView`, which IS a
    // UIScrollView.
    if is_kind(view, class!(UITextView)) {
        return Probe::Opaque;
    }
    if is_kind(view, class!(UICollectionView)) {
        let layout: Option<Retained<AnyObject>> = unsafe { msg_send_id![&**view, collectionViewLayout] };
        if let Some(layout) = layout {
            if is_kind(&layout, class!(UICollectionViewFlowLayout)) {
                let dir: isize = unsafe { msg_send![&layout, scrollDirection] };
                if dir == SCROLL_DIRECTION_HORIZONTAL {
                    return Probe::Opaque;
                }
            }
        }
        return Probe::Scroller;
    }
    // The backend's `scroll_view(axis = Horizontal)` springs sideways only
    // (`create_scroll_view_impl`); every vertical or two-axis one springs
    // vertically.
    let h: bool = unsafe { msg_send![&**view, alwaysBounceHorizontal] };
    let v: bool = unsafe { msg_send![&**view, alwaysBounceVertical] };
    if h && !v {
        Probe::Opaque
    } else {
        Probe::Scroller
    }
}

fn subviews(view: &Retained<AnyObject>) -> Vec<Retained<AnyObject>> {
    let list: Retained<AnyObject> = unsafe { msg_send_id![&**view, subviews] };
    let n: usize = unsafe { msg_send![&list, count] };
    (0..n)
        .map(|i| unsafe { msg_send_id![&list, objectAtIndex: i] })
        .collect()
}

/// The attached control, as the binding drives it.
struct IosRefresh {
    control: Retained<AnyObject>,
    scroll: Retained<AnyObject>,
}

impl NativeRefresh for IosRefresh {
    fn is_refreshing(&self) -> bool {
        unsafe { msg_send![&self.control, isRefreshing] }
    }

    fn begin(&self) {
        // `beginRefreshing` does not move the content, so a refresh the app
        // starts would spin out of sight. When the list sits at its top,
        // scroll the control into view the way a pull leaves it — the
        // spinner Android shows for the same call.
        let inset: EdgeInsets = unsafe { msg_send![&self.scroll, adjustedContentInset] };
        let offset: CGPoint = unsafe { msg_send![&self.scroll, contentOffset] };
        let at_top = offset.y <= -inset.top + 0.5;
        let _: () = unsafe { msg_send![&self.control, beginRefreshing] };
        let window: Option<Retained<AnyObject>> = unsafe { msg_send_id![&self.scroll, window] };
        if at_top && window.is_some() {
            let frame: CGRect = unsafe { msg_send![&self.control, frame] };
            let to = CGPoint { x: offset.x, y: -inset.top - frame.size.height };
            let _: () = unsafe { msg_send![&self.scroll, setContentOffset: to, animated: true] };
        }
    }

    fn end(&self) {
        let _: () = unsafe { msg_send![&self.control, endRefreshing] };
    }

    fn set_tint(&self, tint: Option<Rgba>) {
        let color: Option<Retained<AnyObject>> = tint.map(|c| {
            let [r, g, b, a] = c.to_srgb_f32();
            unsafe {
                msg_send_id![
                    class!(UIColor),
                    colorWithRed: r as CGFloat,
                    green: g as CGFloat,
                    blue: b as CGFloat,
                    alpha: a as CGFloat
                ]
            }
        });
        let _: () = unsafe { msg_send![&self.control, setTintColor: color.as_deref()] };
    }
}

pub(crate) fn mount(
    cx: &mut MountCx<'_, IosBackend>,
    prim: &Rc<RefreshablePrim>,
    children: Vec<Element>,
) -> IosNode {
    let node = crate::mount_passthrough(cx, prim, children);
    let roots: Vec<Retained<AnyObject>> = cx
        .realized_children()
        .iter()
        .flat_map(|live| live.collect_nodes())
        .map(|n: IosNode| {
            let view: &AnyObject = n.as_view().as_ref();
            unsafe { Retained::retain(view as *const AnyObject as *mut AnyObject) }
                .expect("a mounted node has a view")
        })
        .collect();
    let Some(scroll) = policy::first_scroller(&roots, probe, subviews) else {
        // Not a scroll container: the child renders unchanged.
        return node;
    };
    attach(prim, scroll);
    node
}

fn attach(prim: &Rc<RefreshablePrim>, scroll: Retained<AnyObject>) {
    let mtm = MainThreadMarker::new().expect("Refreshable mounts on the main thread");
    let control: Retained<AnyObject> = unsafe { msg_send_id![class!(UIRefreshControl), new] };
    let target = RefreshTarget::new(mtm);
    let _: () = unsafe {
        msg_send![
            &control,
            addTarget: &*target,
            action: objc2::sel!(refreshPulled:),
            forControlEvents: VALUE_CHANGED
        ]
    };
    // A pull needs the spring even when the content is shorter than the
    // view; UIKit gives no pull without it.
    let bounced: bool = unsafe { msg_send![&scroll, alwaysBounceVertical] };
    let _: () = unsafe { msg_send![&scroll, setAlwaysBounceVertical: true] };
    let _: () = unsafe { msg_send![&scroll, setRefreshControl: &*control] };

    let native = Rc::new(IosRefresh { control: control.clone(), scroll: scroll.clone() });
    let pulled = binding::bind(&prim.props, native, backend_ios::newcore::schedule_flush);
    *target.ivars().pulled.borrow_mut() = Some(pulled);

    on_teardown(move || {
        *target.ivars().pulled.borrow_mut() = None;
        let _: () = unsafe {
            msg_send![
                &control,
                removeTarget: &*target,
                action: objc2::sel!(refreshPulled:),
                forControlEvents: VALUE_CHANGED
            ]
        };
        let none: Option<&AnyObject> = None;
        let _: () = unsafe { msg_send![&scroll, setRefreshControl: none] };
        let _: () = unsafe { msg_send![&scroll, setAlwaysBounceVertical: bounced] };
    });
}
