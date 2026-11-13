//! CrewForge report (idea-ui 16.1, A6): a labelled Button could not reach a
//! 44 pt tap target on a phone. `size::Lg` is ~42 pt, the default `Md`
//! measured 23–26 pt at a 390 pt viewport, the root slot override has no
//! breakpoint arm, and a wrapper can't stretch a non-block Button.
//!
//! The fix floors Button's box at `MIN_TOUCH_TARGET_PX` on the `Xs` base
//! and releases it at `TOUCH_TARGET_RELEASE` (`Md`) — `idea_ui::touch`.
//! These tests pin both halves of the realization:
//!
//! - **native** (host-mock, which resolves `__bp_*` overlays against the
//!   viewport at apply time, like every non-CSS backend): the pressable's
//!   applied rules carry the 44 px floor at a 390 px viewport and `auto`
//!   at 1024 px, and re-apply when the viewport crosses `Md`;
//! - **web premint**: the dumped CSS carries the floor on the root's base
//!   class and an `@media (min-width: 768px)` rule releasing it.

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use idea_theme::testing::with_test_world;
use idea_ui::touch::{no_touch_target, MIN_TOUCH_TARGET_PX};
use idea_ui::{install_idea_theme, light_theme, Button, ButtonSlotStyles, ButtonStyles};
use runtime_core::{ui, Element, Length, StyleRules, Tokenized, ViewportSize};

const PHONE: ViewportSize = ViewportSize { width: 390.0, height: 844.0 };
const DESKTOP: ViewportSize = ViewportSize { width: 1024.0, height: 768.0 };

type Applied = Rc<RefCell<Vec<StyleRules>>>;

fn set_viewport(h: &Harness, size: ViewportSize) {
    h.world.enter(|| runtime_vocabulary::viewport::viewport_ctx().set(size));
    h.world.flush();
}

/// Mount `build` at `viewport`, recording every applied style.
fn mount(viewport: ViewportSize, build: impl FnOnce() -> Element) -> (Harness, runtime_scene::Realized<u32>, Applied) {
    let h = Harness::new();
    h.shared.handles_states_natively.set(false);
    set_viewport(&h, viewport);
    let applied: Applied = Rc::new(RefCell::new(Vec::new()));
    let a = applied.clone();
    h.set_style_line(move |n, r| {
        a.borrow_mut().push(r.clone());
        format!("apply_style n{n}")
    });
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let realized = h.mount(tree);
    h.flush();
    (h, realized, applied)
}

fn literal(v: &Option<Tokenized<Length>>) -> Option<Length> {
    match v {
        Some(Tokenized::Literal(l)) => Some(*l),
        Some(other) => panic!("expected a literal length, got {other:?}"),
        None => None,
    }
}

/// The (min_width, min_height) of the last style applied to the Button's
/// box — the only node in these trees that sets `min_height` at all.
fn last_box_minimums(applied: &Applied) -> (Option<Length>, Option<Length>) {
    let applied = applied.borrow();
    let rules = applied
        .iter()
        .rev()
        .find(|r| r.min_height.is_some())
        .expect("the Button box applies a min_height at every breakpoint");
    (literal(&rules.min_width), literal(&rules.min_height))
}

fn save_button() -> Element {
    ui! { Button(label = "Save details") }
}

#[test]
fn regression_button_meets_44pt_touch_target_at_phone_width() {
    let (_h, realized, applied) = mount(PHONE, save_button);
    let floor = Length::Px(MIN_TOUCH_TARGET_PX);
    assert_eq!(
        last_box_minimums(&applied),
        (Some(floor), Some(floor)),
        "at 390 px (Xs) the Button box is floored at the 44 pt tap target"
    );
    drop(realized);
}

#[test]
fn button_keeps_natural_size_at_desktop_width() {
    let (_h, realized, applied) = mount(DESKTOP, save_button);
    assert_eq!(
        last_box_minimums(&applied),
        (Some(Length::Auto), Some(Length::Auto)),
        "from Md up the floor releases to the initial `auto` — the Button is its natural size"
    );
    drop(realized);
}

#[test]
fn touch_target_follows_the_viewport_across_md() {
    let (h, realized, applied) = mount(PHONE, save_button);
    set_viewport(&h, DESKTOP);
    assert_eq!(last_box_minimums(&applied).1, Some(Length::Auto), "growing past Md releases the floor");
    set_viewport(&h, PHONE);
    assert_eq!(
        last_box_minimums(&applied).1,
        Some(Length::Px(MIN_TOUCH_TARGET_PX)),
        "shrinking back to a phone restores it"
    );
    drop(realized);
}

/// An app whose layout switches at `Sm` (640) moves the release there:
/// with the release fixed at `Md` (768), controls changed size at a
/// different width from everything else.
#[test]
fn regression_touch_target_release_follows_the_apps_breakpoint() {
    const SM_TABLET: ViewportSize = ViewportSize { width: 700.0, height: 900.0 };
    let (h, realized, applied) = mount(SM_TABLET, save_button);
    assert_eq!(
        last_box_minimums(&applied).1,
        Some(Length::Px(MIN_TOUCH_TARGET_PX)),
        "at 700 the default (Md) release keeps the floor"
    );
    h.world.enter(|| idea_ui::touch::set_touch_target_release(runtime_core::Breakpoint::Sm));
    // A remount composes with the new release.
    drop(realized);
    let (_h, realized, applied) = mount(SM_TABLET, save_button);
    assert_eq!(last_box_minimums(&applied).1, Some(Length::Auto), "released from Sm up");
    let (_h2, realized2, applied2) = mount(PHONE, save_button);
    assert_eq!(
        last_box_minimums(&applied2).1,
        Some(Length::Px(MIN_TOUCH_TARGET_PX)),
        "a phone keeps it"
    );
    drop((realized, realized2));
}

#[test]
fn no_touch_target_opts_one_button_out() {
    let (_h, realized, applied) = mount(PHONE, || {
        ui! {
            Button(
                label = "Compact",
                styles = ButtonSlotStyles::default().root(no_touch_target()),
            )
        }
    });
    assert_eq!(
        last_box_minimums(&applied),
        (Some(Length::Auto), Some(Length::Auto)),
        "the root override wins over the Xs floor"
    );
    drop(realized);
}

/// Web realizes the floor as build-time CSS: the base class carries it and
/// an `@media` rule at the `Md` threshold releases it.
#[test]
fn touch_target_premints_as_base_rule_plus_md_media_release() {
    with_test_world(|| {
        install_idea_theme(light_theme());
        let classes = ButtonStyles::root()
            .raw("tone", "primary")
            .into_style_application()
            .preminted_class_list()
            .expect("the Button root premints");
        let base = classes.split(' ').next().expect("a base class").to_string();
        let css = premint_dump::dump_all_css();
        let base_rule = css
            .lines()
            .find(|l| l.starts_with(&format!(".{base} {{")))
            .unwrap_or_else(|| panic!("no base rule for .{base}"));
        assert!(
            base_rule.contains("min-width: 44px") && base_rule.contains("min-height: 44px"),
            "the base (Xs) rule floors the box: {base_rule}"
        );
        let md = runtime_core::breakpoints().md_min;
        let release = format!("@media (min-width: {md}px) {{ .{base} {{ min-width: auto; min-height: auto }} }}");
        assert!(
            css.lines().any(|l| l == release),
            "an @media rule at Md releases the floor; expected `{release}`"
        );
    });
}
