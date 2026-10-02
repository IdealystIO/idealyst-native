//! idea-ui as a remote component library. Each area of `spike-ideaui` is
//! mounted from its bundle — every idea-ui component in it an import of the
//! app's copy — and compared against the same tree rendered in-process:
//! the tree, the handle methods the components called (an anchored overlay
//! measuring its trigger through a `Ref` the bundle handed a `Button`), and
//! the tree again after the same interaction.

use host_mock::{pump, Harness};
use runtime_core::{ui, Element};
use stream_spike::IDEA_UI_WASM;

/// The tree as node kinds (and text) in order: no node ids, and no
/// `anchor`s — a remote mount adds anchors where the bundle's tree has
/// reactive holes, which is structure, not output.
fn shape(h: &Harness) -> Vec<String> {
    h.live_roots()
        .into_iter()
        .flat_map(|root| h.live_tree(root).lines().map(str::to_string).collect::<Vec<_>>())
        .map(|line| {
            let line = line.trim_start();
            line.split_once(' ').map_or(line, |(_, kind)| kind).to_string()
        })
        .filter(|kind| kind != "anchor")
        .collect()
}

/// `line` without node ids (`rect n12` → `rect`).
fn without_ids(line: String) -> String {
    line.split(' ').filter(|w| !(w.starts_with('n') && w[1..].parse::<u32>().is_ok())).collect::<Vec<_>>().join(" ")
}

/// Handle method calls, without node ids.
fn handle_calls() -> Vec<String> {
    host_mock::take_handle_log().into_iter().map(without_ids).collect()
}

#[derive(Debug, PartialEq)]
struct Run {
    mounted: Vec<String>,
    calls: Vec<String>,
    /// What the press did to the backend (style updates included), and the
    /// tree after it.
    pressed: Vec<String>,
    after: Vec<String>,
}

/// Mount `tree` with idea-ui's theme installed, as an app does; then press
/// what shows `press` (if any).
fn run(tree: impl FnOnce() -> Element, press: Option<&str>) -> Run {
    let h = Harness::new();
    h.world.enter(|| idea_ui::install_idea_theme(idea_ui::light_theme()));
    handle_calls();
    let realized = h.mount(h.world.enter(tree));
    h.flush();
    let mounted = shape(&h);
    let calls = handle_calls();
    h.take_log();
    if let Some(label) = press {
        h.press_labelled(label);
        h.flush();
    }
    // Sheet registration is a cache, not output: a sheet that crosses is
    // rebuilt app-side as a new object (the same rules), so a branch the
    // bundle builds registers it where native code reuses its static one.
    let pressed: Vec<String> =
        h.take_log().into_iter().filter(|l| !l.starts_with("register_stylesheet")).map(without_ids).collect();
    let after = shape(&h);
    drop(realized);
    h.flush();
    h.forget_handlers();
    Run { mounted, calls, pressed, after }
}

/// The area from the bundle renders, calls and responds exactly as it does
/// in-process — and shows `shows` (so an empty render can't pass).
fn same_as_native(remote: impl FnOnce() -> Element, native: impl FnOnce() -> Element, shows: &str, press: Option<&str>) {
    pump::install_executor();
    pump::install_scheduler();
    let _remote = stream_host::remote::install(IDEA_UI_WASM).expect("the bundle loads");
    let from_bundle = run(remote, press);
    let in_process = run(native, press);
    let failed: Vec<&String> = from_bundle.mounted.iter().filter(|l| l.contains('⚠')).collect();
    assert!(failed.is_empty(), "the remote area failed:\n{}", failed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"));
    assert!(
        from_bundle.mounted.iter().any(|l| l.contains(shows)),
        "the area doesn't show {shows:?}:\n{:#?}",
        from_bundle.mounted
    );
    assert_eq!(from_bundle.mounted, in_process.mounted, "remote and in-process trees differ");
    assert_eq!(from_bundle.calls, in_process.calls, "remote and in-process handle calls differ");
    if press.is_some() {
        assert!(!from_bundle.pressed.is_empty(), "the press reached nothing");
    }
    assert_eq!(from_bundle.pressed, in_process.pressed, "the press did different things to the backend");
    assert_eq!(from_bundle.after, in_process.after, "after the press, remote and in-process trees differ");
    assert_eq!(runtime_vocabulary::remote::host::live_trees(), 0, "the remote tree was torn down");
    assert_eq!(runtime_vocabulary::remote::handles::held_handles(), 0, "no handle outlives the tree");
}

#[test]
fn layout() {
    same_as_native(|| ui! { spike_ideaui::LayoutArea() }, spike_ideaui::layout_demo, "on a surface", None);
}

#[test]
fn text_and_status() {
    same_as_native(|| ui! { spike_ideaui::StatusArea() }, spike_ideaui::status_demo, "Unsaved changes", None);
}

#[test]
fn actions() {
    same_as_native(|| ui! { spike_ideaui::ActionsArea() }, spike_ideaui::actions_demo, "pressed 0", Some("Primary"));
}

#[test]
fn forms() {
    same_as_native(|| ui! { spike_ideaui::FormsArea() }, spike_ideaui::forms_demo, "Wi-Fi", Some("Grid"));
}

#[test]
fn dates() {
    same_as_native(|| ui! { spike_ideaui::DatesArea() }, spike_ideaui::dates_demo, "October 2026", None);
}

#[test]
fn navigation() {
    same_as_native(|| ui! { spike_ideaui::NavigationArea() }, spike_ideaui::navigation_demo, "Home", Some("Two"));
}

#[test]
fn overlays() {
    same_as_native(|| ui! { spike_ideaui::OverlaysArea() }, spike_ideaui::overlays_demo, "in a popover", Some("Menu"));
}

#[test]
fn data() {
    same_as_native(|| ui! { spike_ideaui::DataArea() }, spike_ideaui::data_demo, "Admin", Some("Second"));
}

