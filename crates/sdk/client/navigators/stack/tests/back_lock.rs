//! A screen's `back_enabled` reaches the native presenter, and the SDK's
//! own `bind` lets go of the handle when the navigator unmounts.
//!
//! The presenter is a probe (a real one needs UIKit or an Android
//! activity): it records every back-lock the handler sends, which is the
//! whole contract the SDK takes part in — its options type is read by the
//! extractor it hands the vocabulary builder.

#![cfg(not(target_arch = "wasm32"))]

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::glue::{IntoElement, Ref};
use runtime_vocabulary::handlers::nav_native_push::{
    clear_stack_presenter, install_stack_presenter, NativePushHandle, StackPresenter,
};
use stack_navigator::{Route, Screen, StackBuilder, StackHandle, StackNavigator, StackScreenExt};

const HOME: Route<()> = Route::<()>::new("home", "/");
const BOARD: Route<()> = Route::<()>::new("board", "/board");

#[derive(Default)]
struct Probe {
    back: RefCell<Vec<bool>>,
}

struct ProbePresenter(Rc<Probe>);

impl StackPresenter for ProbePresenter {
    fn attach(&self, outlet: Rc<dyn Any>) -> Option<NativePushHandle> {
        outlet.downcast_ref::<host_mock::Node>()?;
        let reveal = || Rc::new(|_node: Rc<dyn Any>| {}) as Rc<dyn Fn(Rc<dyn Any>)>;
        let probe = self.0.clone();
        Some(NativePushHandle {
            host: Rc::new(()),
            seat: reveal(),
            push: reveal(),
            pop: reveal(),
            replace: reveal(),
            reset: reveal(),
            set_user_pop: Rc::new(|_| {}),
            set_back_enabled: Rc::new(move |enabled| probe.back.borrow_mut().push(enabled)),
        })
    }
}

/// Regression: `.back_enabled(false)` was carried on the screen's options
/// and read by nothing, so swipe-back and system Back still popped it.
#[test]
fn regression_back_enabled_false_locks_the_presenter_while_on_top() {
    let probe: Rc<Probe> = Rc::default();
    install_stack_presenter(Rc::new(ProbePresenter(probe.clone())));
    let h = Harness::new();
    let nav: Ref<StackHandle> = Ref::new();
    let realized = h.mount(
        StackNavigator::new(&HOME)
            .screen(HOME, |_| Screen::new(view().child(text().content("HOME")).build()))
            .screen(BOARD, |_| Screen::new(view().child(text().content("BOARD")).build()).back_enabled(false))
            .bind(nav)
            .into_element(),
    );
    h.world.flush();
    assert_eq!(probe.back.borrow().last(), Some(&true));

    nav.get().expect("bound").push(&BOARD, ());
    h.world.flush();
    assert_eq!(probe.back.borrow().last(), Some(&false), "the board locks back");

    nav.get().expect("bound").pop();
    h.world.flush();
    assert_eq!(probe.back.borrow().last(), Some(&true));

    drop(realized);
    assert!(nav.get().is_none(), "the SDK's bind releases its handle at unmount");
    clear_stack_presenter();
}
