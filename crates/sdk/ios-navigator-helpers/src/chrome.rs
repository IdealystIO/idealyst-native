//! UINavigationBar / navigationItem chrome configuration.
//!
//! Moved verbatim from `backend-ios-mobile::imp::mod` after the
//! navigator-substrate refactor. The shape changed in one place: the
//! `options` argument is now the helper crate's
//! `IosScreenOptions` instead of the deleted `runtime_core::ScreenOptions`
//! struct — every field is identical, only the type's home moved.

use crate::{BarButton, IosScreenOptions};
use backend_ios::CallbackTarget;
use backend_ios_core::style::{color_to_uicolor, font_weight_to_uikit};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::{UIColor, UINavigationController, UIView, UIViewController};
use runtime_core::StyleRules;
use std::rc::Rc;

/// Configure a UIViewController's navigationItem and the parent
/// UINavigationBar from `IosScreenOptions`. Called after mounting a
/// screen in a stack or drawer navigator.
///
/// Returns retained callback targets that must be kept alive (caller
/// stores or forgets them — UIKit holds them weakly via `setTarget:`).
pub fn apply_header_options(
    vc: &UIViewController,
    options: &IosScreenOptions,
    mtm: MainThreadMarker,
) -> Vec<Retained<NSObject>> {
    apply_header_options_with_nav(vc, None, options, mtm)
}

/// Variant of [`apply_header_options`] that takes the parent
/// `UINavigationController` explicitly. The drawer navigator owns its
/// embedded nav controller and the rootVC's `navigationController`
/// property unexpectedly returns nil (even after `setViewControllers:`) —
/// so the drawer passes the nav controller through directly. Stack
/// navigators use the no-arg form and fall back to
/// `vc.navigationController` lookup.
pub fn apply_header_options_with_nav(
    vc: &UIViewController,
    explicit_nav_ctrl: Option<&Retained<NSObject>>,
    options: &IosScreenOptions,
    mtm: MainThreadMarker,
) -> Vec<Retained<NSObject>> {
    let mut retained = Vec::new();

    let nav_ctrl_obj: Option<Retained<NSObject>> = match explicit_nav_ctrl {
        Some(n) => Some(n.clone()),
        None => unsafe {
            let p: *const NSObject = msg_send![vc, navigationController];
            if p.is_null() {
                None
            } else {
                Retained::retain(p as *mut NSObject)
            }
        },
    };

    if let Some(false) = options.header_shown {
        if let Some(ref nav_ctrl) = nav_ctrl_obj {
            let _: () = unsafe {
                msg_send![&**nav_ctrl, setNavigationBarHidden: true, animated: false]
            };
        }
        return vec![];
    }

    if let Some(ref title) = options.title {
        let ns = NSString::from_str(title);
        let _: () = unsafe { msg_send![vc, setTitle: &*ns] };
    }

    // Header bar style — background, title color, and tint for back
    // chevron / bar buttons. Resolve a `UINavigationBarAppearance`
    // and assign it both as `standardAppearance` and
    // `scrollEdgeAppearance` so it stays correct whether or not the
    // top of the screen scrolls under the bar.
    let header_bg = options.header_background.as_ref().map(|f| f());
    let title_color = options.title_color.as_ref().map(|f| f());
    let header_tint = options.header_tint.as_ref().map(|f| f());
    let has_bar_style = header_bg.is_some() || title_color.is_some() || header_tint.is_some();
    if has_bar_style {
        if let Some(ref nav_ctrl) = nav_ctrl_obj {
            let nav_bar: Retained<NSObject> =
                unsafe { msg_send_id![&**nav_ctrl, navigationBar] };
            let appearance: Retained<NSObject> = unsafe {
                msg_send_id![objc2::class!(UINavigationBarAppearance), new]
            };
            let _: () = unsafe { msg_send![&appearance, configureWithOpaqueBackground] };
            if let Some(ref bg) = header_bg {
                let c = color_to_uicolor(bg);
                let _: () = unsafe { msg_send![&appearance, setBackgroundColor: &*c] };
            }
            if let Some(ref tc) = title_color {
                let c = color_to_uicolor(tc);
                let key = NSString::from_str("NSColor");
                let dict: Retained<NSObject> = unsafe {
                    msg_send_id![
                        objc2::class!(NSDictionary),
                        dictionaryWithObject: &*c,
                        forKey: &*key
                    ]
                };
                let _: () = unsafe { msg_send![&appearance, setTitleTextAttributes: &*dict] };
            }
            let _: () = unsafe { msg_send![&nav_bar, setStandardAppearance: &*appearance] };
            let _: () = unsafe { msg_send![&nav_bar, setScrollEdgeAppearance: &*appearance] };
            let _: () = unsafe { msg_send![&nav_bar, setCompactAppearance: &*appearance] };
            let nav_item: Retained<NSObject> = unsafe { msg_send_id![vc, navigationItem] };
            let _: () = unsafe { msg_send![&nav_item, setStandardAppearance: &*appearance] };
            let _: () = unsafe { msg_send![&nav_item, setScrollEdgeAppearance: &*appearance] };
            let _: () = unsafe { msg_send![&nav_item, setCompactAppearance: &*appearance] };
            if let Some(ref tint) = header_tint {
                let c = color_to_uicolor(tint);
                let _: () = unsafe { msg_send![&nav_bar, setTintColor: &*c] };
            }
        }
    }

    if let Some(ref btn) = options.header_left {
        apply_bar_button(
            vc,
            btn,
            header_tint.clone(),
            BarSide::Left,
            mtm,
            &mut retained,
        );
    }

    if let Some(ref btn) = options.header_right {
        apply_bar_button(
            vc,
            btn,
            header_tint.clone(),
            BarSide::Right,
            mtm,
            &mut retained,
        );
    }

    retained
}

enum BarSide {
    Left,
    Right,
}

fn apply_bar_button(
    vc: &UIViewController,
    btn: &BarButton,
    fallback_tint: Option<runtime_core::Color>,
    side: BarSide,
    mtm: MainThreadMarker,
    retained: &mut Vec<Retained<NSObject>>,
) {
    let image: Retained<NSObject> = unsafe {
        let name = NSString::from_str(&btn.icon);
        msg_send_id![objc2::class!(UIImage), systemImageNamed: &*name]
    };
    let on_press = btn.on_press.clone();
    let target = CallbackTarget::new(mtm, on_press);
    let sel = objc2::sel!(invoke);
    let bar_item: Retained<NSObject> =
        unsafe { msg_send_id![objc2::class!(UIBarButtonItem), new] };
    let _: () = unsafe { msg_send![&bar_item, setImage: &*image] };
    let _: () = unsafe { msg_send![&bar_item, setTarget: &*target] };
    let _: () = unsafe { msg_send![&bar_item, setAction: sel] };
    let tint = btn.tint.clone().or(fallback_tint);
    if let Some(t) = tint {
        let c = color_to_uicolor(&t);
        let _: () = unsafe { msg_send![&bar_item, setTintColor: &*c] };
    }
    let nav_item: Retained<NSObject> = unsafe { msg_send_id![vc, navigationItem] };
    match side {
        BarSide::Left => {
            let _: () = unsafe { msg_send![&nav_item, setLeftBarButtonItem: &*bar_item] };
        }
        BarSide::Right => {
            let _: () = unsafe { msg_send![&nav_item, setRightBarButtonItem: &*bar_item] };
        }
    }
    let obj: Retained<NSObject> = unsafe {
        Retained::retain(Retained::as_ptr(&target) as *mut NSObject).unwrap()
    };
    retained.push(obj);
}

// ---------------------------------------------------------------------------
// Stack slot styling — header / title / button
// ---------------------------------------------------------------------------

pub(crate) fn apply_nav_header_style(
    controller: &UINavigationController,
    nav_view: &UIView,
    style: &Rc<StyleRules>,
) {
    unsafe {
        let nav_bar: Retained<NSObject> = msg_send_id![controller, navigationBar];
        let appearance: Retained<NSObject> =
            msg_send_id![objc2::class!(UINavigationBarAppearance), new];

        if let Some(ref bg) = style.background {
            let _: () = msg_send![&appearance, configureWithOpaqueBackground];
            let bg_val = bg.resolve();
            let c = color_to_uicolor(&bg_val);
            let _: () = msg_send![&appearance, setBackgroundColor: &*c];
            nav_view.setBackgroundColor(Some(&c));
            let top_vc: Option<Retained<UIViewController>> =
                msg_send_id![controller, topViewController];
            if let Some(vc) = top_vc {
                if let Some(vc_view) = vc.view() {
                    vc_view.setBackgroundColor(Some(&c));
                }
            }
        } else {
            let _: () = msg_send![&appearance, configureWithTransparentBackground];
        }

        let clear = UIColor::clearColor();
        let _: () = msg_send![&appearance, setShadowColor: &*clear];

        // Write to ALL three appearance buckets on the nav bar AND
        // the top VC's navigationItem. On iOS 15+ the item-level
        // appearance shadows the bar-level one, so a re-themed
        // bar appearance is silently ignored if any prior per-screen
        // `apply_header_options_with_nav` call wrote to the item.
        // We mirror that path's thoroughness here so theme swaps win
        // regardless of order.
        let _: () = msg_send![&nav_bar, setStandardAppearance: &*appearance];
        let _: () = msg_send![&nav_bar, setScrollEdgeAppearance: &*appearance];
        let _: () = msg_send![&nav_bar, setCompactAppearance: &*appearance];
        let top_vc: Option<Retained<UIViewController>> =
            msg_send_id![controller, topViewController];
        if let Some(vc) = top_vc {
            let nav_item: Retained<NSObject> = msg_send_id![&*vc, navigationItem];
            let _: () = msg_send![&nav_item, setStandardAppearance: &*appearance];
            let _: () = msg_send![&nav_item, setScrollEdgeAppearance: &*appearance];
            let _: () = msg_send![&nav_item, setCompactAppearance: &*appearance];
        }
        // Force the nav bar to re-render with the new appearance.
        // Without this, setting the standardAppearance on an already-
        // visible bar can be silently coalesced — the user sees the
        // old appearance until the next layout pass (rotation,
        // navigation, etc).
        let _: () = msg_send![&nav_bar, setNeedsLayout];
    }
}

pub(crate) fn apply_nav_title_style(
    controller: &UINavigationController,
    style: &Rc<StyleRules>,
) {
    unsafe {
        let nav_bar: Retained<NSObject> = msg_send_id![controller, navigationBar];
        let appearance: Retained<NSObject> = msg_send_id![&nav_bar, standardAppearance];
        let appearance: Retained<NSObject> = msg_send_id![&appearance, copy];

        let dict: Retained<NSObject> =
            msg_send_id![objc2::class!(NSMutableDictionary), new];

        if let Some(ref color) = style.color {
            let color_val = color.resolve();
            let c = color_to_uicolor(&color_val);
            let key: Retained<NSObject> = msg_send_id![
                objc2::class!(NSString),
                stringWithUTF8String: b"NSColor\0".as_ptr()
            ];
            let _: () = msg_send![&dict, setObject: &*c, forKey: &*key];
        }

        let size: CGFloat = style
            .font_size
            .as_ref()
            .map(|t| match t.resolve() {
                runtime_core::Length::Px(v) => v as CGFloat,
                _ => 17.0,
            })
            .unwrap_or(17.0);
        let weight = style
            .font_weight
            .unwrap_or(runtime_core::FontWeight::SemiBold);
        let ui_weight = font_weight_to_uikit(weight);
        let font: Retained<NSObject> = msg_send_id![
            objc2::class!(UIFont),
            systemFontOfSize: size,
            weight: ui_weight
        ];
        let key: Retained<NSObject> = msg_send_id![
            objc2::class!(NSString),
            stringWithUTF8String: b"NSFont\0".as_ptr()
        ];
        let _: () = msg_send![&dict, setObject: &*font, forKey: &*key];

        let _: () = msg_send![&appearance, setTitleTextAttributes: &*dict];
        // Mirror apply_nav_header_style's full appearance fan-out
        // (all three nav-bar buckets + the topVC nav_item override)
        // so theme swaps win regardless of which appearance the bar
        // is currently rendering from.
        let _: () = msg_send![&nav_bar, setStandardAppearance: &*appearance];
        let _: () = msg_send![&nav_bar, setScrollEdgeAppearance: &*appearance];
        let _: () = msg_send![&nav_bar, setCompactAppearance: &*appearance];
        let top_vc: Option<Retained<UIViewController>> =
            msg_send_id![controller, topViewController];
        if let Some(vc) = top_vc {
            let nav_item: Retained<NSObject> = msg_send_id![&*vc, navigationItem];
            let _: () = msg_send![&nav_item, setStandardAppearance: &*appearance];
            let _: () = msg_send![&nav_item, setScrollEdgeAppearance: &*appearance];
            let _: () = msg_send![&nav_item, setCompactAppearance: &*appearance];
        }
        let _: () = msg_send![&nav_bar, setNeedsLayout];
    }
}

pub(crate) fn apply_nav_button_style(
    controller: &UINavigationController,
    style: &Rc<StyleRules>,
) {
    unsafe {
        let nav_bar: Retained<NSObject> = msg_send_id![controller, navigationBar];
        if let Some(ref color) = style.color {
            let color_val = color.resolve();
            let c = color_to_uicolor(&color_val);
            let _: () = msg_send![&nav_bar, setTintColor: &*c];
        }
    }
}
