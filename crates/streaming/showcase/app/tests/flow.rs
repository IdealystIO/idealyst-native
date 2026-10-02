//! The showcase, driven end to end against the mock backend with its REAL
//! bundle (the wasm the build script embeds): every tab, the bundle-defined
//! shop navigator, context, host functions, app components, and teardown.

use host_mock::{pump, Harness};
use runtime_core::ui;
use remote_showcase::App;

struct Run {
    h: Harness,
    _realized: Option<runtime_scene::Realized<host_mock::Node>>,
}

impl Run {
    fn start() -> Run {
        pump::install_executor();
        pump::install_scheduler();
        remote_showcase::install();
        let h = Harness::new();
        let tree = h.world.enter(|| ui! { App() });
        let realized = h.mount(tree);
        h.flush();
        Run { h, _realized: Some(realized) }
    }

    /// Every text on screen.
    fn screen(&self) -> String {
        self.h.live_roots().iter().map(|n| self.h.live_tree(*n)).collect::<Vec<_>>().join("\n")
    }

    /// Press the most recently created button whose label (as created)
    /// starts with `label` (buttons are recorded in creation order).
    fn press(&self, label: &str) {
        let buttons: Vec<String> =
            self.h.shared.kinds.borrow().values().filter(|k| k.starts_with("button ")).cloned().collect();
        // The tab shell marks the active tab `[Label]`.
        let (plain, active) = (format!("button \"{label}"), format!("button \"[{label}]"));
        let i = buttons
            .iter()
            .rposition(|k| k.starts_with(&plain) || k.starts_with(&active))
            .unwrap_or_else(|| panic!("no button {label:?}: {buttons:?}"));
        let press = self.h.shared.button_presses.borrow()[i].clone();
        press();
        self.h.flush();
    }

    fn settle(&self) {
        pump::pump_tasks();
        pump::pump_timers();
        pump::pump_tasks();
        self.h.flush();
    }

    fn sees(&self, what: &str) -> bool {
        self.screen().contains(what)
    }
}

#[test]
fn the_showcase_runs_its_remote_screens() {
    let r = Run::start();

    // Feed (remote): app components, a sync host function, bundle state.
    assert!(r.sees("Feed — rendered by the bundle"), "{}", r.screen());
    assert!(r.sees(&format!("running on {}", std::env::consts::OS)), "{}", r.screen());
    assert!(r.sees("Shipping update") && r.sees("Orders placed today"), "{}", r.screen());
    r.press("♥ 0");
    assert!(r.sees("♥ 1"), "{}", r.screen());

    // Settings (native) drives the Theme the feed reads live.
    r.press("Settings");
    assert!(r.sees("Settings — native"), "{}", r.screen());
    r.press("Compact feed: off");
    r.press("Feed");
    assert!(r.sees("density: compact") && !r.sees("Orders placed today"), "compact hides bodies:\n{}", r.screen());

    // Shop (a navigator defined in the bundle).
    r.press("Shop");
    assert!(r.sees("Trail Mug") && r.sees("$18.00") && r.sees("Shop   ·   cart: 0"), "{}", r.screen());
    (r.h.link_activation(0))(); // Trail Mug's route link
    r.h.flush();
    assert!(r.sees("Quantity: 1") && r.sees("Loading reviews…"), "{}", r.screen());
    assert!(r.sees("Trail Mug   ·   cart: 0"), "the header shows the screen's title option:\n{}", r.screen());

    // The async host function answers.
    r.settle();
    assert!(r.sees("Reviews: “Survived a whole summer.”"), "{}", r.screen());

    // Controls, then the bundle writes the app's cart.
    let slide = r.h.shared.slider_changes.borrow().last().unwrap().clone();
    slide(3.0);
    let gift = r.h.shared.toggle_changes.borrow().last().unwrap().clone();
    gift(true);
    // The test's own copies of the handlers own the bundle's callbacks.
    drop((slide, gift));
    r.h.flush();
    assert!(r.sees("Quantity: 3") && r.sees("Gift wrap: yes"), "{}", r.screen());
    r.press("Add to cart");
    assert!(r.sees("Trail Mug   ·   cart: 3") && r.sees("cart: 3   ·"), "both the bundle's header and the app shell:\n{}", r.screen());

    // Back to the list through the bundle's header (StackNav::pop).
    r.press("‹ Back");
    assert!(r.sees("Shop   ·   cart: 3") && !r.sees("Quantity:"), "{}", r.screen());

    // The native settings see the same cart.
    r.press("Settings");
    assert!(r.sees("3 item(s)"), "{}", r.screen());

    // Teardown leaves nothing behind.
    let Run { h, mut _realized } = r;
    drop(_realized.take());
    h.flush();
    h.forget_handlers();
    assert_eq!(runtime_vocabulary::remote::handles::held_handles(), 0);
    assert_eq!(runtime_vocabulary::remote::host::live_trees(), 0);
}
