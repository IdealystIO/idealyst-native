//! `Select` / `Autocomplete` group headings and `Autocomplete`'s remote mode,
//! mounted on the recording host.
//!
//! CrewForge's want_0842baa4 (a grouped searchable picker) and want_3e23bb0c:
//! a host fetching matches per keystroke needs rows drawn as given, focus and
//! the typed text surviving each list swap, and a committed value that keeps its
//! label once its row leaves the list.

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use idea_ui::{install_idea_theme, light_theme, Autocomplete, Select, SelectOption};
use runtime_core::primitives::key::KeyEvent;
use runtime_core::{signal, ui, Element, Signal};

fn mount(h: &Harness, build: impl FnOnce() -> Element) -> runtime_scene::Realized<u32> {
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let realized = h.mount(tree);
    h.flush();
    realized
}

fn press_key(h: &Harness, key: &str) {
    let handler = h.key_down_handler(0).expect("the input has a key handler");
    handler(&KeyEvent {
        key: key.to_string(),
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
        selection_start: 0,
        selection_end: 0,
    });
    h.flush();
}

fn type_query(h: &Harness, text: &str) {
    h.text_input_change(0)(text.to_string());
    h.flush();
}

fn focus(h: &Harness, on: bool) {
    h.text_input_focus_handler(0)(on);
    h.flush();
}

/// Every live node's kind under the mounted roots, in order, in one string.
fn live(h: &Harness) -> String {
    h.live_roots().into_iter().map(|r| h.live_tree(r)).collect::<Vec<_>>().join("\n")
}

thread_local! {
    static INPUT_TEXT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// What the input shows: the last value the host was told to display.
fn input_text(h: &Harness) -> String {
    for line in h.take_log() {
        if let Some((_, value)) = line.strip_prefix("update_text_input_value ").and_then(|l| l.split_once(' ')) {
            INPUT_TEXT.with(|t| *t.borrow_mut() = value.trim_matches('"').to_string());
        }
    }
    INPUT_TEXT.with(|t| t.borrow().clone())
}

fn grouped() -> Vec<SelectOption> {
    vec![
        SelectOption::new("meters", "Meters").group("Drilling"),
        SelectOption::new("holes", "Holes").group("Drilling"),
        SelectOption::new("hours", "Hours").group("Work hours"),
    ]
}

#[test]
fn select_menu_draws_group_headings_that_cannot_be_picked() {
    let h = Harness::new();
    let picked: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = picked.clone();
    let _r = mount(&h, move || {
        let value = signal(String::new());
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| sink.borrow_mut().push(id));
        ui! { Select(value = value, options = grouped(), on_change = on_change) }
    });
    h.press_handler(0)();
    h.flush();
    let tree = live(&h);
    let at = |needle: &str| tree.find(needle).unwrap_or_else(|| panic!("{needle} is drawn:\n{tree}"));
    assert!(at("Drilling") < at("Meters") && at("Meters") < at("Holes"), "{tree}");
    assert!(at("Holes") < at("Work hours") && at("Work hours") < at("Hours\""), "{tree}");
    assert!(
        !h.shared.press_by_node.borrow().keys().any(|n| h.kind_of(*n).is_some_and(|k| k.contains("Drilling"))),
        "a heading is not a press target"
    );
    h.press_labelled("Holes");
    h.flush();
    assert_eq!(*picked.borrow(), ["holes"]);
}

/// The keyboard cursor walks rows only: `ArrowDown` from "Holes" reaches "Hours"
/// across the "Work hours" heading, and a filtered-out group loses its heading.
#[test]
fn autocomplete_keyboard_cursor_skips_headings_and_an_emptied_group_hides() {
    let h = Harness::new();
    let picked: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = picked.clone();
    let _r = mount(&h, move || {
        let value = signal(String::new());
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| sink.borrow_mut().push(id));
        ui! { Autocomplete(value = value, options = grouped(), on_change = on_change) }
    });
    focus(&h, true);
    let tree = live(&h);
    assert!(tree.contains("Drilling") && tree.contains("Work hours"), "{tree}");

    press_key(&h, "ArrowDown"); // Meters → Holes
    press_key(&h, "ArrowDown"); // Holes → Hours: the heading between takes no stop
    press_key(&h, "Enter");
    assert_eq!(*picked.borrow(), ["hours"]);

    focus(&h, false);
    focus(&h, true);
    type_query(&h, "hole");
    let tree = live(&h);
    assert!(tree.contains("Drilling"), "{tree}");
    assert!(!tree.contains("Work hours"), "a group with no rows left has no heading:\n{tree}");
}

struct Remote {
    options: Signal<Vec<SelectOption>>,
    value: Signal<String>,
    queries: Rc<RefCell<Vec<String>>>,
    committed: Rc<RefCell<Vec<String>>>,
}

fn mount_remote(h: &Harness, allow_free_text: bool) -> (Remote, runtime_scene::Realized<u32>) {
    let queries: Rc<RefCell<Vec<String>>> = Rc::default();
    let committed: Rc<RefCell<Vec<String>>> = Rc::default();
    let handles: Rc<RefCell<Option<(Signal<Vec<SelectOption>>, Signal<String>)>>> = Rc::default();
    let (q, c, out) = (queries.clone(), committed.clone(), handles.clone());
    let realized = mount(h, move || {
        let options = signal(Vec::<SelectOption>::new());
        let value = signal(String::new());
        *out.borrow_mut() = Some((options, value));
        let on_query: Rc<dyn Fn(String)> = Rc::new(move |text| q.borrow_mut().push(text));
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |id: String| {
            c.borrow_mut().push(id.clone());
            value.set(id);
        });
        ui! {
            Autocomplete(
                value = value,
                options = options,
                on_change = on_change,
                filter = false,
                on_query = Some(on_query),
                allow_free_text = allow_free_text,
            )
        }
    });
    let (options, value) = handles.borrow().expect("built");
    (Remote { options, value, queries, committed }, realized)
}

fn address(id: &str, label: &str) -> SelectOption {
    SelectOption::new(id, label.to_string())
}

/// The whole want_3e23bb0c repro: rows swapped per keystroke draw as given even
/// though their labels don't contain the query, the input is never rebuilt and
/// never loses focus, the typed text stays, and the committed label outlives its
/// row.
#[test]
fn regression_autocomplete_remote_rows_swap_while_typing_and_the_commit_keeps_its_label() {
    let h = Harness::new();
    let (remote, _r) = mount_remote(&h, false);
    focus(&h, true);

    type_query(&h, "100 mccullum rd");
    assert_eq!(*remote.queries.borrow(), ["100 mccullum rd"], "the host hears the typed text");
    remote.options.set(vec![address("a1", "100 McCullum Road, London"), address("a2", "100 McCullum Way, Leeds")]);
    h.flush();
    let tree = live(&h);
    assert!(tree.contains("100 McCullum Road, London"), "a row not containing the query is drawn:\n{tree}");
    assert!(tree.contains("100 McCullum Way, Leeds"), "{tree}");

    let inputs_before = h.shared.text_input_changes.borrow().len();
    type_query(&h, "100 mccullum road");
    remote.options.set(vec![address("a1", "100 McCullum Road, London")]);
    h.flush();
    assert_eq!(h.shared.text_input_changes.borrow().len(), inputs_before, "the input is not rebuilt");
    assert!(!live(&h).contains("Leeds"), "the swap replaced the rows");
    let handled_blur = h.shared.focus_handlers.borrow().len();
    assert_eq!(handled_blur, 1, "one focus handler: the same input");
    assert_eq!(remote.queries.borrow().len(), 2);
    assert_eq!(remote.committed.borrow().len(), 0, "typing commits nothing");
    assert_eq!(input_text(&h), "100 mccullum road", "the typed text survives the swap");

    h.press_labelled("100 McCullum Road, London");
    h.flush();
    assert_eq!(*remote.committed.borrow(), ["a1"]);
    assert_eq!(remote.queries.borrow().len(), 2, "a commit's rewrite of the input is not a query");

    // The next search's rows don't include the committed one.
    remote.options.set(vec![address("b1", "Elsewhere")]);
    h.flush();
    assert_eq!(input_text(&h), "100 McCullum Road, London", "the committed label survives");

    // So does abandoning a search: Escape reverts to it, not to nothing.
    focus(&h, true);
    type_query(&h, "somewhere else");
    press_key(&h, "Escape");
    assert_eq!(input_text(&h), "100 McCullum Road, London");
    assert_eq!(remote.value.peek().as_str(), "a1");
}

#[test]
fn free_text_commits_on_enter_and_blur_and_escape_still_reverts() {
    let h = Harness::new();
    let (remote, _r) = mount_remote(&h, true);
    remote.options.set(vec![address("a1", "100 McCullum Road")]);
    h.flush();
    focus(&h, true);

    // Enter keeps the text, not the top match.
    type_query(&h, "my own address");
    press_key(&h, "Enter");
    assert_eq!(*remote.committed.borrow(), ["my own address"]);
    assert_eq!(input_text(&h), "my own address");

    // Blur commits too; text matching a row commits that row's id.
    focus(&h, true);
    type_query(&h, "100 McCullum Road");
    focus(&h, false);
    assert_eq!(remote.committed.borrow().last().unwrap(), "a1");

    // An arrow key engages the cursor: Enter picks the row after all.
    focus(&h, true);
    type_query(&h, "x");
    press_key(&h, "ArrowDown");
    remote.committed.borrow_mut().clear();
    remote.value.set(String::new());
    h.flush();
    press_key(&h, "Enter");
    assert_eq!(*remote.committed.borrow(), ["a1"]);

    // Escape abandons.
    focus(&h, true);
    remote.committed.borrow_mut().clear();
    type_query(&h, "typo");
    press_key(&h, "Escape");
    assert!(remote.committed.borrow().is_empty());
    assert_eq!(input_text(&h), "100 McCullum Road");

    // Blur with nothing changed is not a commit.
    focus(&h, true);
    focus(&h, false);
    assert!(remote.committed.borrow().is_empty(), "an untouched blur commits nothing");
}

/// A `loading` row while a fetch is out, in place of the empty text.
#[test]
fn autocomplete_loading_row_replaces_the_empty_text() {
    let h = Harness::new();
    let handles: Rc<RefCell<Option<Signal<bool>>>> = Rc::default();
    let out = handles.clone();
    let _r = mount(&h, move || {
        let value = signal(String::new());
        let loading = signal(true);
        *out.borrow_mut() = Some(loading);
        ui! { Autocomplete(value = value, filter = false, loading = loading, loading_text = Some("Searching".to_string())) }
    });
    focus(&h, true);
    let tree = live(&h);
    assert!(tree.contains("Searching") && !tree.contains("No results"), "{tree}");

    handles.borrow().unwrap().set(false);
    h.flush();
    let tree = live(&h);
    assert!(!tree.contains("Searching") && tree.contains("No results"), "{tree}");
}
