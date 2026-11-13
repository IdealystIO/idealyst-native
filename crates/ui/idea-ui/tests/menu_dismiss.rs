//! Escape and an outside click close a `Menu` the same way: every portal
//! it mounted (the dismiss catcher's backdrop and content, the panel) is
//! released once the exit hold runs out.

use std::rc::Rc;

use host_mock::Harness;
use idea_ui::{install_idea_theme, light_theme, Menu, MenuItem};
use runtime_core::{signal, ui, AnchorTarget, PressableHandle, Ref, Signal};

struct Mounted {
    h: Harness,
    _realized: runtime_scene::Realized<host_mock::Node>,
    open: Signal<bool>,
}

fn mount() -> Mounted {
    let h = Harness::new();
    let (tree, open) = h.world.enter(|| {
        install_idea_theme(light_theme());
        let open = signal(true);
        let trigger: Ref<PressableHandle> = Ref::new();
        let on_dismiss: Rc<dyn Fn()> = Rc::new(move || open.set(false));
        let tree = ui! {
            view {
                if open.get() {
                    Menu(target = Some(AnchorTarget::from(trigger)), on_dismiss = Some(on_dismiss.clone())) {
                        MenuItem(label = "Rename", on_select = Rc::new(|| {}) as Rc<dyn Fn()>)
                    }
                }
            }
        };
        (tree, open)
    });
    let realized = h.mount(tree);
    h.flush();
    Mounted { h, _realized: realized, open }
}

/// Portals created minus portals released, from the op log.
fn live_portals(log: &[String]) -> i64 {
    let made = log.iter().filter(|l| l.starts_with("create ") && l.ends_with(" portal")).count() as i64;
    let gone = log.iter().filter(|l| l.starts_with("release_portal")).count() as i64;
    made - gone
}

#[test]
fn escape_releases_every_portal_the_menu_mounted() {
    host_mock::pump::install_scheduler();
    let m = mount();
    let mut log = m.h.take_log();
    assert!(live_portals(&log) > 0, "{log:#?}");
    let escaped = m.h.world.enter(runtime_vocabulary::escape_dismiss::dismiss_topmost);
    assert!(escaped, "an open menu takes Escape");
    m.h.flush();
    assert!(!m.open.peek(), "Escape reached the menu's on_dismiss");
    host_mock::pump::pump_timers();
    m.h.flush();
    log.extend(m.h.take_log());
    assert_eq!(live_portals(&log), 0, "{log:#?}");
}

/// The catcher's full-screen backdrop stops taking clicks the moment the
/// menu starts leaving. It stayed hit-testable through the exit hold, so a
/// click on the trigger right after Escape landed on the invisible
/// backdrop instead.
#[test]
fn regression_a_leaving_menus_backdrop_lets_clicks_through() {
    host_mock::pump::install_scheduler();
    let m = mount();
    let styles = std::rc::Rc::new(std::cell::RefCell::new(Vec::<(host_mock::Node, Option<runtime_core::PointerEvents>)>::new()));
    let s = styles.clone();
    m.h.set_style_line(move |n, r| {
        s.borrow_mut().push((n, r.pointer_events));
        format!("apply_style n{n}")
    });
    m.h.world.enter(runtime_vocabulary::escape_dismiss::dismiss_topmost);
    m.h.flush();
    let through: Vec<_> = styles
        .borrow()
        .iter()
        .filter(|(_, p)| *p == Some(runtime_core::PointerEvents::None))
        .map(|(n, _)| *n)
        .collect();
    assert!(!through.is_empty(), "during the exit hold the backdrop stops taking clicks: {:?}", styles.borrow());
}
