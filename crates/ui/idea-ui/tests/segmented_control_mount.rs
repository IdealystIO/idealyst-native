//! `SegmentedControl` mounted through the real `realize` path against
//! `host-mock`: the segment whose `id` equals `value` is painted with the
//! `SegmentButton` sheet's `selected = on` arm, every other segment with
//! the `off` arm, and a press moves the `on` arm to the pressed segment in
//! place. The build-tree unit tests in `segmented_control.rs` resolve the
//! style closures directly; this checks what the HOST is actually told to
//! paint, which is what CrewForge saw when the control looked like `Tabs`.

use std::rc::Rc;

use idea_ui::stylesheets::SegmentButton;
use idea_ui::{install_idea_theme, light_theme, SegmentOption, SegmentedControl};
use runtime_core::{resolve_style, signal, ui, Signal, StyleApplication};

/// The host-visible fill of a style: background and border color, token
/// names included, so the `on` and `off` arms print differently.
fn fill(r: &runtime_core::StyleRules) -> String {
    format!("bg={:?} border={:?}", r.background, r.border_left_color)
}

/// What the `SegmentButton` sheet resolves for one `selected` arm.
fn arm(h: &host_mock::Harness, selected: &str) -> String {
    h.world.enter(|| {
        let app = StyleApplication::new(SegmentButton::sheet()).with("selected", selected.to_string());
        fill(&resolve_style(&app))
    })
}

/// The last fill the host was told to paint on each segment pressable, in
/// creation (= option) order.
fn segment_fills(h: &host_mock::Harness) -> Vec<String> {
    let ops = h.ops();
    let pressables: Vec<String> = ops
        .iter()
        .filter_map(|o| o.strip_prefix("create ").filter(|r| r.ends_with(" pressable")))
        .map(|r| r.trim_end_matches(" pressable").to_string())
        .collect();
    assert!(!pressables.is_empty(), "no segment pressables:\n{}", ops.join("\n"));
    pressables
        .iter()
        .map(|node| {
            let prefix = format!("apply_style {node} ");
            ops.iter()
                .rev()
                .find_map(|o| o.strip_prefix(&prefix).map(str::to_string))
                .unwrap_or_else(|| panic!("segment {node} never styled:\n{}", ops.join("\n")))
        })
        .collect()
}

fn mount(initial: &str) -> (host_mock::Harness, Signal<String>, runtime_scene::Realized<u32>) {
    let h = host_mock::Harness::new();
    h.set_style_line(|n, r| format!("apply_style n{n} {}", fill(r)));
    h.record_all();
    let initial = initial.to_string();
    let (tree, value) = h.world.enter(|| {
        install_idea_theme(light_theme());
        let value = signal(initial);
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| value.set(v));
        let tree = ui! {
            SegmentedControl(
                value = value,
                on_change = on_change,
                options = vec![
                    SegmentOption::new("list", "List"),
                    SegmentOption::new("grid", "Grid"),
                    SegmentOption::new("map", "Map"),
                ],
            )
        };
        (tree, value)
    });
    let realized = h.mount(tree);
    h.flush();
    (h, value, realized)
}

#[test]
fn the_selected_segment_is_painted_with_the_selected_arm() {
    let (h, _value, _r) = mount("grid");
    let (on, off) = (arm(&h, "on"), arm(&h, "off"));
    assert_ne!(on, off, "the two arms must paint differently");
    assert_eq!(segment_fills(&h), vec![off.clone(), on.clone(), off.clone()], "only `grid` is selected");
}

#[test]
fn pressing_a_segment_moves_the_selected_arm_to_it() {
    let (h, value, _r) = mount("list");
    let (on, off) = (arm(&h, "on"), arm(&h, "off"));
    assert_eq!(segment_fills(&h), vec![on.clone(), off.clone(), off.clone()]);

    h.world.enter(|| h.press_labelled("Map"));
    h.flush();
    assert_eq!(h.world.enter(|| value.get()), "map", "the press commits the segment's id");
    assert_eq!(segment_fills(&h), vec![off.clone(), off.clone(), on.clone()], "the fill follows the value in place");
}
