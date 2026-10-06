//! Regression (reported against idea-ui 3.2.0): a Field's LIVE `error`
//! painted in the help grey instead of the Danger tone.
//!
//! `Field(error = rx!(err.get()))` with `err` starting at `None` resolved
//! the help line's tone ONCE at build — to "default" (muted grey,
//! rgb(100,105,122)) — so the first validation error showed in grey. Only
//! the input border (`reactive_error_drives_border_color_live`) followed
//! the live error. Mounted through the real `realize` path against
//! `host-mock`, because the help line is an on-demand guarded node (a
//! `when`) that the build-tree mirror can't look inside.

use std::rc::Rc;

use idea_ui::{install_idea_theme, light_theme, Field};
use runtime_core::{rx, signal, ui, Signal};

/// The `color` the host was last told to paint the text node showing
/// `content`, read from the op log.
fn last_text_color(harness: &host_mock::Harness, content: &str) -> String {
    let ops = harness.ops();
    let created = format!("text \"{content}\"");
    let updated = format!("\"{content}\"");
    // The node id that carries `content` — created with it, or updated to it.
    let node = ops
        .iter()
        .find_map(|o| {
            let is_create = o.starts_with("create ") && o.ends_with(&created);
            let is_update = o.starts_with("update_text ") && o.ends_with(&updated);
            if is_create || is_update {
                o.split_whitespace().nth(1).map(str::to_string)
            } else {
                None
            }
        })
        .unwrap_or_else(|| panic!("no text node shows {content:?}:\n{}", ops.join("\n")));
    let prefix = format!("apply_style {node} ");
    ops.iter()
        .rev()
        .find_map(|o| o.strip_prefix(&prefix).map(str::to_string))
        .unwrap_or_else(|| panic!("text {node} was never styled:\n{}", ops.join("\n")))
}

fn harness() -> host_mock::Harness {
    let h = host_mock::Harness::new();
    h.set_style_line(|n, r| format!("apply_style n{n} color={:?}", r.color));
    h
}

/// The help-line colour of a Field whose error is a FIXED `Some` — the
/// oracle for "the Danger tone": a static error always resolved right.
fn static_error_color() -> String {
    let h = harness();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        let value = signal(String::new());
        ui! {
            Field(
                value = value,
                on_change = Rc::new(move |v: String| value.set(v)) as Rc<dyn Fn(String)>,
                error = Some("Required".to_string()),
            )
        }
    });
    let _r = h.mount(tree);
    h.flush();
    last_text_color(&h, "Required")
}

/// The help-line colour of a Field with a fixed HELP text and no error —
/// the muted help grey.
fn help_color() -> String {
    let h = harness();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        let value = signal(String::new());
        ui! {
            Field(
                value = value,
                on_change = Rc::new(move |v: String| value.set(v)) as Rc<dyn Fn(String)>,
                help = Some("Required".to_string()),
            )
        }
    });
    let _r = h.mount(tree);
    h.flush();
    last_text_color(&h, "Required")
}

#[test]
fn regression_live_error_paints_help_line_in_danger_tone() {
    let danger = static_error_color();
    let grey = help_color();
    assert_ne!(danger, grey, "oracle: the error and help colours differ");

    let h = harness();
    let (tree, err) = h.world.enter(|| {
        install_idea_theme(light_theme());
        let value = signal(String::new());
        let err: Signal<Option<String>> = signal(None);
        let tree = ui! {
            Field(
                value = value,
                on_change = Rc::new(move |v: String| value.set(v)) as Rc<dyn Fn(String)>,
                error = rx!(err.get()),
            )
        };
        (tree, err)
    });
    let _r = h.mount(tree);
    h.flush();

    // Validation fails AFTER the Field is built.
    h.world.enter(|| err.set(Some("Required".to_string())));
    h.flush();
    assert_eq!(
        last_text_color(&h, "Required"),
        danger,
        "a live error must paint in the Danger tone, not the help grey ({grey})"
    );
}

/// A live `help` with a live `error` beside it: the line is grey while it
/// shows help, flips to Danger when the error arrives, and back to grey
/// when the error clears — the tone follows in place, both ways.
#[test]
fn live_error_tone_flips_back_when_the_error_clears() {
    let danger = static_error_color();
    let grey = help_color();

    let h = harness();
    let (tree, err, help) = h.world.enter(|| {
        install_idea_theme(light_theme());
        let value = signal(String::new());
        let err: Signal<Option<String>> = signal(None);
        let help: Signal<Option<String>> = signal(Some("Required".to_string()));
        let tree = ui! {
            Field(
                value = value,
                on_change = Rc::new(move |v: String| value.set(v)) as Rc<dyn Fn(String)>,
                help = rx!(help.get()),
                error = rx!(err.get()),
            )
        };
        (tree, err, help)
    });
    let _r = h.mount(tree);
    h.flush();
    assert_eq!(last_text_color(&h, "Required"), grey, "help text starts grey");

    h.world.enter(|| err.set(Some("Required".to_string())));
    h.flush();
    assert_eq!(last_text_color(&h, "Required"), danger, "the error turns it Danger");

    h.world.enter(|| err.set(None));
    h.flush();
    assert_eq!(last_text_color(&h, "Required"), grey, "clearing the error returns to grey");
    let _ = help;
}
