//! The robot's `key` verb on macOS (`IntrospectionOps::native_key`) against
//! real AppKit: the press is posted as the NSEvents a keyboard produces and
//! delivered by `-[NSApplication sendEvent:]`, so the focused text field's
//! field editor types the character and deletes on Backspace, and the
//! app-level key source (an `NSEvent` local monitor) hears the key with its
//! modifiers — Meta+K arrives as Command down, K, Command up.
//!
//! The same real events cover two macOS key behaviors web and Windows have:
//! a focused field's `on_key_down` (Enter submits; `PreventDefault` stops
//! the field typing the key) and Escape calling the topmost shown
//! overlay's `on_dismiss` (`src/newcore/field_keys.rs`).
//!
//! `harness = false`: AppKit windows and the event queue are main-thread
//! only, and libtest runs cases on worker threads. Posted events are pulled
//! off the queue and sent the way `-[NSApplication run]` does.
//!
//! ```sh
//! cargo test -p backend-macos --test robot_native_key
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::RefCell;
    use std::rc::Rc;

    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSString};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::primitives::key::{AppKeyEvent, KeyEvent, KeyOutcome, KeyPhase, KeyboardSink};
    use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement};
    use runtime_vocabulary::caps::{
        AppEnvOps, IntrospectionOps, KeyChord, LifecycleOps, PortalOps, TextInputOps, ViewOps,
    };

    /// Pull every queued event and send it, as `-[NSApplication run]` does,
    /// for `turns` × 10 ms.
    fn pump(app: &AnyObject, turns: usize) {
        let mode = NSString::from_str("kCFRunLoopDefaultMode");
        for _ in 0..turns {
            loop {
                let until: Retained<AnyObject> =
                    unsafe { msg_send_id![class!(NSDate), dateWithTimeIntervalSinceNow: 0.01f64] };
                let event: *mut AnyObject = unsafe {
                    msg_send![app, nextEventMatchingMask: usize::MAX, untilDate: &*until, inMode: &*mode, dequeue: true]
                };
                if event.is_null() {
                    break;
                }
                let _: () = unsafe { msg_send![app, sendEvent: event] };
            }
        }
    }

    fn field_view(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn string_value(view: &NSView) -> String {
        let s: Retained<NSString> = unsafe { msg_send_id![view, stringValue] };
        s.to_string()
    }

    pub fn run() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let app: Retained<AnyObject> = unsafe { msg_send_id![class!(NSApplication), sharedApplication] };
        // A regular app, so its window can become key.
        let _: bool = unsafe { msg_send![&*app, setActivationPolicy: 0isize] };
        let rect = CGRect { origin: CGPoint { x: 100.0, y: 100.0 }, size: CGSize { width: 400.0, height: 300.0 } };
        let window: Retained<NSWindow> = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSWindow>(),
                rect,
                NSWindowStyleMask::Titled,
                NSBackingStoreType::NSBackingStoreBuffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        let content: Retained<NSView> = unsafe { msg_send_id![mtm.alloc::<NSView>(), initWithFrame: rect] };
        window.setContentView(Some(&content));
        let mut backend = MacosBackend::new(mtm);
        backend.set_host_root(content.clone());

        let a11y = AccessibilityProps::default();
        let changes: Rc<RefCell<Vec<String>>> = Rc::default();
        let on_change = {
            let changes = changes.clone();
            Rc::new(move |s: String| changes.borrow_mut().push(s)) as Rc<dyn Fn(String)>
        };
        // The field's on_key_down: Enter submits (and is claimed), "x" is
        // refused, everything else types.
        let field_keys: Rc<RefCell<Vec<KeyEvent>>> = Rc::default();
        let submits = Rc::new(std::cell::Cell::new(0u32));
        let on_key_down = {
            let (field_keys, submits) = (field_keys.clone(), submits.clone());
            Rc::new(move |e: &KeyEvent| {
                field_keys.borrow_mut().push(e.clone());
                match e.key.as_str() {
                    "Enter" => {
                        submits.set(submits.get() + 1);
                        KeyOutcome::PreventDefault
                    }
                    "x" => KeyOutcome::PreventDefault,
                    _ => KeyOutcome::Default,
                }
            }) as runtime_shared::primitives::key::KeyDownHandler
        };
        let mut root = backend.create_view(&a11y);
        let field = backend.create_text_input("", None, on_change, Some(on_key_down), None, false, &a11y);
        backend.insert(&mut root, field.clone());
        backend.finish(root);

        let keys: Rc<RefCell<Vec<AppKeyEvent>>> = Rc::default();
        {
            let keys = keys.clone();
            backend.set_keyboard_sink(Some(KeyboardSink::new(
                move |e| {
                    keys.borrow_mut().push(e.clone());
                    KeyOutcome::Default
                },
                || {},
            )));
        }

        let _: () = unsafe { msg_send![&*app, activateIgnoringOtherApps: true] };
        window.makeKeyAndOrderFront(None);
        let fv = field_view(&field);
        let _: bool = unsafe { msg_send![&*window, makeFirstResponder: &*fv] };
        pump(&app, 5);

        let press = |spec: &str| {
            let p = KeyChord::parse(spec).unwrap().resolve(backend.shortcut_modifier_is_meta());
            let inject = backend.native_key(&field, &p).expect("a window to send keys to");
            inject().expect("posted");
            pump(&app, 5);
        };
        for k in ["h", "i", "!"] {
            press(k);
        }
        assert_eq!(string_value(&fv), "hi!", "the field editor typed the characters");
        press("Backspace");
        assert_eq!(string_value(&fv), "hi", "Backspace deletes");
        assert_eq!(changes.borrow().last().map(String::as_str), Some("hi"), "on_change heard the edits");
        press("Meta+K");
        assert_eq!(string_value(&fv), "hi", "a shortcut types nothing");
        press("Enter");
        regression_macos_text_input_on_key_down_never_fired(&fv, &field_keys, &submits, &press);

        let got: Vec<(KeyPhase, String, String, bool)> =
            keys.borrow().iter().map(|e| (e.phase, e.key.clone(), e.code.clone(), e.meta)).collect();
        let down = |k: &str, c: &str, meta: bool| (KeyPhase::Down, k.to_string(), c.to_string(), meta);
        let up = |k: &str, c: &str, meta: bool| (KeyPhase::Up, k.to_string(), c.to_string(), meta);
        let shortcut: Vec<_> = got.iter().skip_while(|e| e.1 != "Meta").take(4).cloned().collect();
        assert_eq!(
            shortcut,
            vec![down("Meta", "MetaLeft", true), down("k", "KeyK", true), up("k", "KeyK", true), up("Meta", "MetaLeft", false)],
            "the app-level source hears Meta+K as a keyboard sends it: {got:?}"
        );
        assert!(got.contains(&down("Enter", "Enter", false)) && got.contains(&up("Enter", "Enter", false)), "{got:?}");
        assert!(got.contains(&down("Backspace", "Backspace", false)), "{got:?}");
        let bang = keys.borrow().iter().find(|e| e.key == "!").cloned().expect("! heard");
        assert!(bang.shift && bang.code == "Digit1", "{bang:?}");

        regression_macos_escape_never_dismissed_portal(&mut backend, &app, &field, &keys);

        backend.set_keyboard_sink(None);
        println!("robot_native_key: ok");
    }

    /// REGRESSION: `create_text_input_impl` dropped `on_key_down`, so on
    /// macOS no key — a user's or the robot's — reached a field's handler:
    /// Enter could not submit a sign-in form. Now the focused field's
    /// handler sees each key-down with its caret, and `PreventDefault`
    /// stops the field typing it.
    fn regression_macos_text_input_on_key_down_never_fired(
        fv: &NSView,
        field_keys: &RefCell<Vec<KeyEvent>>,
        submits: &std::cell::Cell<u32>,
        press: &dyn Fn(&str),
    ) {
        assert_eq!(submits.get(), 1, "Enter reached on_key_down: {:?}", field_keys.borrow());
        let typed: Vec<String> = field_keys.borrow().iter().map(|e| e.key.clone()).collect();
        assert_eq!(typed, ["h", "i", "!", "Backspace", "k", "Enter"], "every key-down in the field");
        let enter = field_keys.borrow().last().cloned().unwrap();
        assert_eq!((enter.selection_start, enter.selection_end), (2, 2), "caret after \"hi\"");
        assert!(field_keys.borrow()[4].meta, "Meta+K carries meta");
        press("x");
        assert_eq!(string_value(fv), "hi", "PreventDefault: the field doesn't type it");
        press("y");
        assert_eq!(string_value(fv), "hiy", "Default: it does");
    }

    /// REGRESSION: `create_portal_impl` ignored `on_dismiss`, so Escape never
    /// closed an overlay on macOS (web and Windows close the topmost one).
    /// Now Escape calls the topmost SHOWN portal's dismiss — the latest
    /// mounted, skipping a hidden one — and a released portal is forgotten.
    fn regression_macos_escape_never_dismissed_portal(
        backend: &mut MacosBackend,
        app: &AnyObject,
        field: &MacosNode,
        keys: &RefCell<Vec<AppKeyEvent>>,
    ) {
        let a11y = AccessibilityProps::default();
        let hits: Rc<RefCell<Vec<&'static str>>> = Rc::default();
        let dismiss = |name: &'static str| {
            let hits = hits.clone();
            Some(Rc::new(move || hits.borrow_mut().push(name)) as Rc<dyn Fn()>)
        };
        let target = || PortalTarget::Viewport(ViewportPlacement::FullScreen);
        let lower = backend.create_portal(target(), dismiss("lower"), false, &a11y);
        let upper = backend.create_portal(target(), dismiss("upper"), false, &a11y);
        let escape = |backend: &MacosBackend| {
            let p = KeyChord::parse("Escape").unwrap().resolve(true);
            backend.native_key(field, &p).expect("window")().expect("posted");
            pump(app, 5);
        };
        escape(backend);
        assert_eq!(hits.borrow().as_slice(), ["upper"], "the topmost overlay closes");
        backend.set_portal_hidden(&upper, true);
        escape(backend);
        assert_eq!(hits.borrow().as_slice(), ["upper", "lower"], "a hidden overlay is skipped");
        backend.release_portal(&lower);
        backend.release_portal(&upper);
        keys.borrow_mut().clear();
        escape(backend);
        assert_eq!(hits.borrow().len(), 2, "released overlays are forgotten");
        assert!(
            keys.borrow().iter().any(|e| e.key == "Escape" && e.phase == KeyPhase::Down),
            "with no overlay, Escape reaches the app"
        );
    }
}
