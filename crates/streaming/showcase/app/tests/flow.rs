//! The showcase, driven end to end against the mock backend with its REAL
//! bundle (the wasm the build script embeds): every tab, the bundle-defined
//! shop navigator, context, host functions, app components, and teardown.
//! `--features inline` runs the same flow with the remote screens compiled
//! in-process (the native baseline).

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

    /// Press what the screen shows as `label` (an idea-ui `Button`, a
    /// primitive button, a tab), then flush.
    fn press(&self, label: &str) {
        // The tab shell marks the active tab `[Label]`.
        let active = format!("[{label}]");
        self.h.press_labelled(if self.sees(&active) { &active } else { label });
        self.h.flush();
    }

    /// Press the pressable next to the text `label` — an idea-ui `Switch`'s
    /// track sits beside its label, not around it.
    fn press_beside(&self, label: &str) {
        let shared = &self.h.shared;
        let quoted = format!("text {label:?}");
        let text = shared.kinds.borrow().iter().filter(|(_, k)| **k == quoted).map(|(n, _)| *n).last();
        let text = text.unwrap_or_else(|| panic!("no text {label:?}:\n{}", self.screen()));
        let row = shared.parent.borrow()[&text];
        let press = self
            .h
            .children_of(row)
            .into_iter()
            .find_map(|n| shared.press_by_node.borrow().get(&n).cloned())
            .unwrap_or_else(|| panic!("nothing pressable beside {label:?}:\n{}", self.screen()));
        press();
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
    // Built with `--features inline`, the same screens run in-process: no
    // tree came from the bundle.
    assert_eq!(
        runtime_vocabulary::remote::host::live_trees() > 0,
        !cfg!(feature = "inline"),
        "remote trees mounted: {}",
        runtime_vocabulary::remote::host::live_trees()
    );
    assert!(r.sees(&format!("running on {}", std::env::consts::OS)), "{}", r.screen());
    assert!(r.sees("Shipping update") && r.sees("Orders placed today"), "{}", r.screen());
    r.press("♥ 0");
    assert!(r.sees("♥ 1"), "{}", r.screen());

    // Remote code switches the APP's theme and pushes onto the APP's toast
    // queue: idea-ui's global functions are host functions.
    let background = |r: &Run| {
        r.h.world.enter(|| runtime_shared::Tokenized::<runtime_shared::Color>::token("color-background", runtime_shared::Color("unset".into())).resolve().0)
    };
    let light_bg = background(&r);
    r.press("☾ Dark");
    let dark_bg = background(&r);
    assert_ne!(light_bg, dark_bg, "the bundle's Dark button didn't switch the app's theme");
    assert_eq!(dark_bg, "#0a0e17", "the app's dark theme is active");
    r.press("☀ Light");
    assert_eq!(background(&r), light_bg, "and back to light");
    assert!(!r.sees("Hello from the bundle"));
    r.press("Toast");
    assert!(r.sees("Hello from the bundle"), "the bundle's toast reached the app's ToastHost:\n{}", r.screen());

    // Settings (native) drives the FeedPrefs the feed reads live.
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

    // idea-ui's controls (app components; their handlers are bundle code),
    // then the bundle writes the app's cart. The slider maps a touch's x
    // against its width: one far past the end is its max, 5.
    let drag = r.h.shared.touch_handlers.borrow().last().unwrap().1.clone();
    drag(&touch_at(10_000.0));
    r.press_beside("Gift wrap");
    // The test's own copy of the handler owns the bundle's callback.
    drop(drag);
    r.h.flush();
    assert!(r.sees("Quantity: 5") && r.sees("Gift wrap: yes"), "{}", r.screen());
    r.press("Add to cart");
    assert!(r.sees("Trail Mug   ·   cart: 5") && r.sees("cart: 5   ·"), "both the bundle's header and the app shell:\n{}", r.screen());

    // Back to the list through the bundle's header (StackNav::pop).
    r.press("‹ Back");
    assert!(r.sees("Shop   ·   cart: 5") && !r.sees("Quantity:"), "{}", r.screen());

    // The native settings see the same cart.
    r.press("Settings");
    assert!(r.sees("5 item(s)"), "{}", r.screen());

    // Teardown leaves nothing behind.
    let Run { h, mut _realized } = r;
    drop(_realized.take());
    h.flush();
    h.forget_handlers();
    assert_eq!(runtime_vocabulary::remote::handles::held_handles(), 0);
    assert_eq!(runtime_vocabulary::remote::host::live_trees(), 0);
}

/// The Tools tab (remote): bundle code calling every kind of host
/// function — plain, struct and `Result`, number lists, async, and the
/// generic ones on the bundle's OWN types (a `#[derive(Key)]` struct and a
/// `#[derive(Remote)]` value the app never compiles). `--features inline`
/// runs the same code natively, calling the generic functions with those
/// types directly, and must print the same.
#[test]
fn the_tools_tab_calls_every_kind_of_host_function() {
    let r = Run::start();
    r.press("Tools");
    for want in [
        "words: 9",
        "invoice: $56.10",
        "empty invoice: Err(Empty)",
        "histogram: [2, 1, 3]",
        "by team, then hired: Sam, Ada, Bea, Ola, Lin, Kim",
        "teams: eng, ops, sales",
        "eng: Ada (2019), Sam (2017), Bea (2021)",
        "ops: Lin (2021), Ola (2018)",
        "sales: Kim (2021)",
        "orders: Ada→Mug, Sam→Lamp, Sam→Tent",
        "stats: n=4 min=1.5 max=9 mean=4.25",
        "sorted: [-40, -3, 0, 7, 12]",
        "digest: …",
    ] {
        assert!(r.sees(want), "missing {want:?}:\n{}", r.screen());
    }
    // The async ones answer once the app's executor runs them.
    r.settle();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in b"remote" {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    assert!(r.sees("latest hires: [2021, 2021]"), "{}", r.screen());
    assert!(r.sees(&format!("digest: {h:016x}")), "{}", r.screen());
}

fn touch_at(x: f32) -> runtime_core::TouchEvent {
    runtime_core::TouchEvent {
        id: runtime_core::TouchId(1),
        phase: runtime_core::TouchPhase::Began,
        position: runtime_core::TouchPoint::new(x, 0.0),
        window_position: runtime_core::TouchPoint::new(x, 0.0),
        timestamp_ns: 0,
        force: None,
    }
}
