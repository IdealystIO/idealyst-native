//! Quitting an app: the process ends with the app still mounted, and the
//! thread-locals its teardown reaches are destroyed one after another.
//!
//! Each case runs in a child process of this test (it has to exit for real);
//! the parent reads how it ended. No window is ordered on screen.
//!
//! `harness = false`: AppKit views are main-thread only.
//!
//! ```sh
//! cargo test -p backend-macos --test quit_teardown
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--quit") => imp::child(),
        _ => imp::run(),
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::RefCell;
    use std::process::Command;
    use std::rc::Rc;

    use backend_macos::MacosBackend;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSString};
    use runtime_vocabulary::builders::{navigator_outlet, scroll_view, swap_navigator, text, view};
    use runtime_vocabulary::glue::{effect, on_cleanup, signal};
    use runtime_shared::primitives::navigator::Route;

    thread_local! {
        /// Created after the kernel's own thread-local, so destroyed before it.
        static APP_STATE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static LIVE: RefCell<Vec<()>> = const { RefCell::new(Vec::new()) };
    }

    /// Reaches a thread-local as it drops, as a handle an effect holds does.
    struct Held;
    impl Held {
        fn new() -> Self {
            LIVE.with(|l| l.borrow_mut().push(()));
            Held
        }
    }
    impl Drop for Held {
        fn drop(&mut self) {
            LIVE.with(|l| l.borrow_mut().pop());
        }
    }

    const HOME: Route = Route::new("home", "/");

    pub fn child() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let mut backend = MacosBackend::new(mtm);
        let host = backend.create_host_root().expect("host root");
        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: 400.0, height: 300.0 },
        };
        let _: () = unsafe { msg_send![&host, setFrame: rect] };
        backend.set_host_root(host);
        let backend = Rc::new(RefCell::new(backend));
        backend_macos::install_global_self(Rc::downgrade(&backend));
        let app = backend_macos::newcore::start(backend, |_| {}, || {
            let count = signal(0);
            effect(move || {
                count.get();
                APP_STATE.with(|s| s.borrow_mut().push("open".into()));
                let held = Held::new();
                on_cleanup(move || {
                    APP_STATE.with(|s| s.borrow_mut().push("closed".into()));
                    drop(held);
                });
            });
            let held = Held::new();
            effect(move || {
                let _ = &held;
                count.get();
            });
            swap_navigator(&HOME)
                .screen(HOME, move |_: ()| {
                    scroll_view()
                        .child(text().content(move || format!("{}", count.get())))
                        .build()
                })
                .layout(|| view().child(navigator_outlet()).build())
                .build()
        });
        match std::env::args().nth(2).as_deref() {
            // Cmd-Q, the last window closing: AppKit posts the notification,
            // then exits.
            Some("terminate") => {
                app.stop_at_quit();
                unsafe {
                    let center: Retained<AnyObject> =
                        msg_send_id![class!(NSNotificationCenter), defaultCenter];
                    let name =
                        NSString::from_str("NSApplicationWillTerminateNotification");
                    let _: () = msg_send![
                        &center,
                        postNotificationName: &*name,
                        object: std::ptr::null::<AnyObject>()
                    ];
                }
                APP_STATE.with(|s| println!("{}", s.borrow().join(" ")));
            }
            // `std::process::exit` from author code: no notification.
            Some("exit") => app.stop_at_quit(),
            other => panic!("unknown case {other:?}"),
        }
        std::process::exit(0);
    }

    fn quit(case: &str, stdout: &str) {
        let me = std::env::current_exe().expect("this test's binary");
        let out = Command::new(me).args(["--quit", case]).output().expect("the child runs");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{case}: {:?}\n{err}", out.status);
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), stdout, "{case}");
        println!("test quit_{case} ... ok");
    }

    pub fn run() {
        // REGRESSION (every quit ended in a crash report): the mounted app was
        // torn down by the kernel thread-local's destructor, where a cleanup
        // reaching a thread-local already destroyed aborts.
        quit("terminate", "open closed");
        quit("exit", "");
    }
}
