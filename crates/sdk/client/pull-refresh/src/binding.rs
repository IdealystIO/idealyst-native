//! The reactive half of a native refresh control, shared by the iOS and
//! Android handlers and un-gated so it runs on the host.
//!
//! A handler builds its platform control, wraps it in a [`NativeRefresh`],
//! and calls [`bind`]. That installs, in the mounting scope:
//!
//! - one effect that keeps the control showing the author's `refreshing`
//!   value (see [`policy::sync`]), and
//! - the tint, applied once or tracked, as the prop is static or reactive,
//!
//! and returns the closure the platform calls when the user completes a
//! pull. That closure runs `on_refresh`, then bumps a private signal the
//! sync effect also reads, so the effect re-runs after the flush that
//! commits whatever `on_refresh` wrote — and ends the spinner if the app
//! left `refreshing` false. Reading `refreshing` right after `on_refresh`
//! returns would not work: writes stage until the flush, so the read would
//! still see the old value.

use std::cell::Cell;
use std::rc::Rc;

use runtime_core::Reactive;
use runtime_shared::color::Rgba;
use runtime_shared::Color;
use runtime_vocabulary::style_attach::on_teardown;
use runtime_world::{effect, signal, untrack};

use crate::policy::{self, Sync};

/// The platform control, as the binding drives it.
pub(crate) trait NativeRefresh {
    /// Whether the control shows its spinner now. Flips to `true` on its
    /// own when the user completes a pull.
    fn is_refreshing(&self) -> bool;
    /// Show the spinner (a refresh the app started).
    fn begin(&self);
    /// Hide the spinner.
    fn end(&self);
    /// Color the spinner; `None` restores the platform's default.
    fn set_tint(&self, tint: Option<Rgba>);
}

/// The author's props, as the handlers read them.
pub(crate) struct Props {
    pub refreshing: Reactive<bool>,
    pub on_refresh: Rc<dyn Fn()>,
    pub tint: Reactive<Option<Color>>,
}

/// Bind `control` to `props` for the life of the enclosing scope and return
/// the pull callback. `commit` runs after each pull, once `on_refresh` has
/// returned: a native control's callback is outside every framework-wrapped
/// handler, so the platform handlers pass their backend's
/// `newcore::schedule_flush` here, or `on_refresh`'s writes would sit staged
/// until some unrelated event flushed.
///
/// The returned closure is inert once the scope tears down, so a pull the
/// platform delivers late never writes a disposed signal (a stale write
/// panics).
pub(crate) fn bind(props: &Props, control: Rc<dyn NativeRefresh>, commit: fn()) -> Rc<dyn Fn()> {
    let pulls = signal(0u64);
    {
        let control = control.clone();
        let refreshing = props.refreshing.clone();
        let _ = effect(move || {
            let _ = pulls.get();
            let wanted = refreshing.get();
            // The control is platform state, not a signal; untracked keeps
            // a future signal-reading implementation from re-subscribing.
            let native = untrack(|| control.is_refreshing());
            match policy::sync(native, wanted) {
                Sync::Begin => control.begin(),
                Sync::End => control.end(),
                Sync::Keep => {}
            }
        });
    }
    {
        let control = control.clone();
        props
            .tint
            .clone()
            .bind(move |tint| control.set_tint(policy::tint_rgba(tint.as_ref())));
    }

    let alive = Rc::new(Cell::new(true));
    {
        let alive = alive.clone();
        on_teardown(move || alive.set(false));
    }
    let on_refresh = props.on_refresh.clone();
    Rc::new(move || {
        if !alive.get() {
            return;
        }
        on_refresh();
        pulls.update(|n| n.wrapping_add(1));
        commit();
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A control that behaves like the native ones: `begin`/`end` set its
    /// state, and [`FakeControl::user_pulls`] flips it on by itself.
    #[derive(Default)]
    struct FakeControl {
        refreshing: Cell<bool>,
        calls: RefCell<Vec<String>>,
    }

    impl FakeControl {
        fn user_pulls(&self, pulled: &Rc<dyn Fn()>) {
            self.refreshing.set(true);
            pulled();
        }
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.calls.borrow_mut())
        }
    }

    impl NativeRefresh for FakeControl {
        fn is_refreshing(&self) -> bool {
            self.refreshing.get()
        }
        fn begin(&self) {
            self.refreshing.set(true);
            self.calls.borrow_mut().push("begin".into());
        }
        fn end(&self) {
            self.refreshing.set(false);
            self.calls.borrow_mut().push("end".into());
        }
        fn set_tint(&self, tint: Option<Rgba>) {
            self.calls.borrow_mut().push(format!("tint {tint:?}"));
        }
    }

    fn no_commit() {}

    #[test]
    fn a_pull_runs_on_refresh_and_the_app_ends_it_by_clearing_refreshing() {
        let world = runtime_world::World::new();
        let control = Rc::new(FakeControl::default());
        let (loading, pulled, count) = world.enter(|| {
            let loading = signal(false);
            let count = Rc::new(Cell::new(0));
            let on_refresh: Rc<dyn Fn()> = {
                let count = count.clone();
                Rc::new(move || {
                    count.set(count.get() + 1);
                    loading.set(true);
                })
            };
            let props = Props {
                refreshing: loading.into(),
                on_refresh,
                tint: Reactive::Static(None),
            };
            let pulled = bind(&props, control.clone(), no_commit);
            (loading, pulled, count)
        });
        assert_eq!(control.take(), vec!["tint None"], "mounting begins nothing");

        control.user_pulls(&pulled);
        world.flush();
        assert_eq!(count.get(), 1);
        assert!(control.is_refreshing(), "the app took the refresh up: the spinner stays");
        assert!(control.take().is_empty());

        world.enter(|| loading.set(false));
        world.flush();
        assert_eq!(control.take(), vec!["end"]);
        assert!(!control.is_refreshing());
    }

    /// The bug this guards: an `on_refresh` that never sets `refreshing`
    /// (a synchronous reload, a static `false`) left the native spinner
    /// running forever, because nothing the effect reads had changed.
    #[test]
    fn regression_a_pull_the_app_ignores_does_not_leave_the_spinner_running() {
        let world = runtime_world::World::new();
        let control = Rc::new(FakeControl::default());
        let pulled = world.enter(|| {
            let props = Props {
                refreshing: Reactive::Static(false),
                on_refresh: Rc::new(|| {}),
                tint: Reactive::Static(None),
            };
            bind(&props, control.clone(), no_commit)
        });
        let _ = control.take();

        control.user_pulls(&pulled);
        world.flush();
        assert_eq!(control.take(), vec!["end"]);
        assert!(!control.is_refreshing());
    }

    #[test]
    fn a_refresh_the_app_starts_shows_the_spinner() {
        let world = runtime_world::World::new();
        let control = Rc::new(FakeControl::default());
        let loading = world.enter(|| {
            let loading = signal(true);
            let props = Props {
                refreshing: loading.into(),
                on_refresh: Rc::new(|| {}),
                tint: Reactive::Static(None),
            };
            let _ = bind(&props, control.clone(), no_commit);
            loading
        });
        assert_eq!(control.take(), vec!["begin", "tint None"], "an initial true shows at mount");

        world.enter(|| loading.set(false));
        world.flush();
        assert_eq!(control.take(), vec!["end"]);
    }

    #[test]
    fn the_pull_callback_commits_after_on_refresh() {
        runtime_tls::thread_local! {
            static COMMITS: Cell<u32> = const { Cell::new(0) };
        }
        fn commit() {
            COMMITS.with(|c| c.set(c.get() + 1));
        }
        let world = runtime_world::World::new();
        let control = Rc::new(FakeControl::default());
        let pulled = world.enter(|| {
            let props = Props {
                refreshing: Reactive::Static(false),
                on_refresh: Rc::new(|| {}),
                tint: Reactive::Static(None),
            };
            bind(&props, control.clone(), commit)
        });
        control.user_pulls(&pulled);
        assert_eq!(COMMITS.with(|c| c.get()), 1);
    }

    /// A pull the platform delivers after the scope tore down must not
    /// write the disposed `pulls` signal (a stale write panics) nor run
    /// `on_refresh` against a screen that is gone.
    #[test]
    fn regression_a_late_pull_after_teardown_is_inert() {
        let world = runtime_world::World::new();
        let control = Rc::new(FakeControl::default());
        let count = Rc::new(Cell::new(0));
        let (pulled, owned) = world.enter(|| {
            runtime_world::collect_owned(|| {
                let count = count.clone();
                let props = Props {
                    refreshing: Reactive::Static(false),
                    on_refresh: Rc::new(move || count.set(count.get() + 1)),
                    tint: Reactive::Static(None),
                };
                bind(&props, control.clone(), no_commit)
            })
        });
        world.enter(|| drop(owned));
        control.user_pulls(&pulled);
        world.flush();
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn a_reactive_tint_follows_its_signal() {
        let world = runtime_world::World::new();
        let control = Rc::new(FakeControl::default());
        let tint = world.enter(|| {
            let tint = signal(Some(Color("#00ff00".into())));
            let props = Props {
                refreshing: Reactive::Static(false),
                on_refresh: Rc::new(|| {}),
                tint: tint.into(),
            };
            let _ = bind(&props, control.clone(), no_commit);
            tint
        });
        assert_eq!(control.take(), vec![format!("tint {:?}", Some(Rgba::new(0, 255, 0, 255)))]);

        world.enter(|| tint.set(None));
        world.flush();
        assert_eq!(control.take(), vec!["tint None"]);
    }
}
