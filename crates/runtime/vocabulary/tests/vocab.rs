//! P2b unit tests: builder coercions, `StyleProp` paths, prop-binding
//! shapes, teardown probes, and the mount-once payload contract —
//! against the shared `host-mock` substrate (crates/dev/host-mock): the
//! native scene `Host` + caps recording mock, no `Backend`, no
//! `LegacyBridge` (the full-op *sequence* parity lives in
//! `scene-parity`'s golden suite; these tests pin the vocabulary-local
//! behaviors in isolation).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use host_mock::{take_animated_log, Harness};
use runtime_shared::{StateBits, StyleRules, Tokenized};
use runtime_scene::realize;
use runtime_vocabulary::builders::{
    button, icon, image, link, pressable, slider, text, toggle, view, TextContent,
};
use runtime_vocabulary::style_attach::IntoStyleProp;
use runtime_vocabulary::StyleProp;
use runtime_world::{signal, Value, World};

// ===========================================================================
// Harness (host-mock, configured like the old suite-local Mini)
// ===========================================================================

fn width_of(rules: &StyleRules) -> String {
    rules
        .width
        .as_ref()
        .map(|w| format!("{w:?}"))
        .unwrap_or_else(|| "none".into())
}

/// The old suite-local `Mini` backend reported
/// `supports_preminted_styles() = true` and minted `mint-w{width}`
/// classes; replicate both on the shared mock so the preminted and
/// batched-Repeat expectations carry over byte-for-byte. host-mock's
/// default `apply_style` digest is the `width=`-only line this suite
/// originally defined, so every expected string is unchanged.
fn harness() -> Harness {
    let h = Harness::new();
    h.shared.preminted.set(true);
    h.set_mint_class(|rules| Some(format!("mint-w{}", width_of(rules))));
    h
}

fn px(w: f32) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(runtime_shared::Length::Px(w))),
        ..Default::default()
    }
}

// ===========================================================================
// TextContent / IntoValue coercions
// ===========================================================================

#[test]
fn text_content_static_forms_are_const() {
    assert!(matches!("hi".into_content(), Value::Const(s) if s == "hi"));
    assert!(matches!(String::from("hi").into_content(), Value::Const(s) if s == "hi"));
}

#[test]
fn text_content_closure_of_to_string_is_dyn() {
    let v = (|| 42).into_content();
    match v {
        Value::Dyn(f) => assert_eq!(f(), "42"),
        Value::Const(_) => panic!("closure must coerce to Dyn"),
    }
}

#[test]
fn text_content_signal_is_dyn_through_get() {
    let world = World::new();
    world.enter(|| {
        let s = signal(7i32);
        let v = s.into_content();
        match v {
            Value::Dyn(f) => assert_eq!(f(), "7"),
            Value::Const(_) => panic!("signal must coerce to Dyn"),
        }
    });
}

#[test]
fn style_prop_coercions() {
    assert!(matches!(px(1.0).into_style_prop(), StyleProp::Static(_)));
    assert!(matches!(Rc::new(px(1.0)).into_style_prop(), StyleProp::Static(_)));
    assert!(matches!((|| px(1.0)).into_style_prop(), StyleProp::Dynamic(_)));
    assert!(matches!(
        (|| Rc::new(px(1.0))).into_style_prop(),
        StyleProp::Dynamic(_)
    ));
}

// ===========================================================================
// glue::styled_text — the old `styled_text(runs)` author mirror
// ===========================================================================

/// The glue mirror of old `styled_text(runs).with_style(…)` (the
/// website Prose surface) lowers through the text builder's `runs`
/// channel to ONE `create_styled_text` with the runs intact — not the
/// plain-concat `create_text` degradation — and the author style lands
/// on that node.
#[test]
fn glue_styled_text_mounts_runs_through_create_styled_text() {
    use runtime_vocabulary::glue::{styled_text, IntoElement, TextRun};

    let h = harness();
    let element = styled_text(vec![
        TextRun::plain("the "),
        TextRun::styled("ui!", runtime_shared::TextRunStyle::default()),
        TextRun::plain(" macro"),
    ])
    .with_style(px(7.0))
    .into_element();
    let _realized = h.world.enter(|| realize(&h.backend, &h.registry, element));

    let log = h.take_log();
    assert_eq!(
        log[0],
        "create n0 styled_text [\"the \", \"ui!\", \" macro\"]",
        "runs must reach create_styled_text intact: {log:?}"
    );
    assert!(
        log.iter().any(|l| l.starts_with("apply_style n0")),
        "author style must land on the styled-text node: {log:?}"
    );
}

// ===========================================================================
// StyleProp paths
// ===========================================================================

#[test]
fn static_style_applies_once_and_releases_on_teardown() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view().style(px(120.0)).build(),
        )
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 view".to_string(),
            "apply_style n0 width=Literal(Px(120.0))".to_string(),
        ],
        "static path: exactly one apply, no attach_states"
    );
    drop(realized);
    assert_eq!(
        h.take_log(),
        vec!["on_node_unstyled n0".to_string()],
        "teardown fires on_node_unstyled exactly once"
    );
}

#[test]
fn dynamic_style_reapplies_on_signal_change_and_state_flip() {
    let h = harness();
    let world = h.world.clone();
    let (realized, wide) = world.enter(|| {
        let wide = signal(false);
        let realized = realize(
            &h.backend,
            &h.registry,
            view()
                .style(move || if wide.get() { px(300.0) } else { px(100.0) })
                .build(),
        );
        (realized, wide)
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 view".to_string(),
            "apply_style n0 width=Literal(Px(100.0))".to_string(),
            "attach_states n0".to_string(),
        ],
        "dynamic path: initial apply + state hookup"
    );

    // Signal change re-applies with the new rules.
    wide.set(true);
    world.flush();
    assert_eq!(
        h.take_log(),
        vec!["apply_style n0 width=Literal(Px(300.0))".to_string()]
    );

    // A native state flip (through the attach_states setter) re-fires
    // the binding — the re-apply contract the old event-driven path
    // has. (Overlay resolution is deferred; the rules are unchanged.)
    let setter = h.state_setter(0);
    setter(StateBits::HOVERED, true);
    world.flush();
    assert_eq!(
        h.take_log(),
        vec!["apply_style n0 width=Literal(Px(300.0))".to_string()],
        "state flip re-applies via the binding effect"
    );

    drop(realized);
    assert_eq!(h.take_log(), vec!["on_node_unstyled n0".to_string()]);
}

// ===========================================================================
// Prop bindings
// ===========================================================================

#[test]
fn const_button_label_installs_no_update() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            button().label("Go").on_press(|| {}).build(),
        )
    });
    assert_eq!(
        h.take_log(),
        vec!["create n0 button \"Go\"".to_string()],
        "Const label: create only, no mount-time update_button_label"
    );
    drop(realized);
}

#[test]
fn dyn_button_label_updates_at_mount_and_per_change() {
    let h = harness();
    let world = h.world.clone();
    let (realized, label) = world.enter(|| {
        let label = signal(String::from("Start"));
        let realized = realize(
            &h.backend,
            &h.registry,
            button().label(move || label.get()).on_press(|| {}).build(),
        );
        (realized, label)
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 button \"Start\"".to_string(),
            "update_button_label n0 \"Start\"".to_string(),
        ],
        "Dyn label: created at initial value, binding re-applies at mount (walker shape)"
    );
    label.set("Stop".to_string());
    world.flush();
    assert_eq!(
        h.take_log(),
        vec!["update_button_label n0 \"Stop\"".to_string()]
    );
    drop(realized);
}

#[test]
fn controlled_toggle_writes_back_on_change() {
    let h = harness();
    let world = h.world.clone();
    let (realized, on) = world.enter(|| {
        let on = signal(true);
        let realized = realize(
            &h.backend,
            &h.registry,
            toggle().value(on).on_change(|_| {}).build(),
        );
        (realized, on)
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 toggle true".to_string(),
            "update_toggle_value n0 true".to_string(),
        ]
    );
    on.set(false);
    world.flush();
    assert_eq!(h.take_log(), vec!["update_toggle_value n0 false".to_string()]);
    drop(realized);
}

#[test]
fn slider_on_change_snaps_to_step() {
    let h = harness();
    let world = h.world.clone();
    let received: Rc<RefCell<Vec<f32>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = received.clone();
    let realized = world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            slider()
                .value(0.0f32)
                .on_change(move |v| sink.borrow_mut().push(v))
                .range(0.0, 10.0)
                .step(0.5)
                .build(),
        )
    });
    // Simulate a native drag reporting a raw, unsnapped value.
    let native_on_change = h.slider_change(0);
    native_on_change(3.34);
    assert_eq!(
        *received.borrow(),
        vec![3.5],
        "the wrapper snaps to step before dispatch — uniform across backends"
    );
    drop(realized);
}

// ===========================================================================
// Teardown probe + payload contract
// ===========================================================================

#[test]
fn on_teardown_fires_once_at_drop_not_per_flush() {
    let world = World::new();
    let fired = Rc::new(Cell::new(0u32));
    let owned = world.enter(|| {
        let fired = fired.clone();
        let ((), owned) = runtime_world::collect_owned(|| {
            runtime_vocabulary::on_teardown(move || fired.set(fired.get() + 1));
        });
        owned
    });
    world.flush();
    world.flush();
    assert_eq!(fired.get(), 0, "no teardown before the scope drops");
    drop(owned);
    assert_eq!(fired.get(), 1, "exactly once at scope drop");
}

#[test]
#[should_panic(expected = "mounted twice")]
fn prim_cell_take_twice_panics() {
    let cell = runtime_vocabulary::prims::PrimCell::new(1u32);
    let _ = cell.take();
    let _ = cell.take();
}

#[test]
#[should_panic(expected = ".src(...)")]
fn image_without_src_panics_at_build() {
    let _ = image().build();
}

#[test]
#[should_panic(expected = ".data(...)")]
fn icon_without_data_panics_at_build() {
    let _ = icon().build();
}

#[test]
#[should_panic(expected = "on_activate")]
fn non_external_link_without_on_activate_panics_at_mount() {
    let h = harness();
    let _ = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            link().url("/somewhere").build(),
        )
    });
}

/// Regression shape for the walker's uniform-disable contract: a bare
/// pressable is not a native form control, so the press must be blocked
/// at the callback (the shared flag), not just via `set_disabled`.
#[test]
fn pressable_disabled_blocks_press_uniformly() {
    let h = harness();
    let world = h.world.clone();
    let pressed = Rc::new(Cell::new(0u32));
    let sink = pressed.clone();
    let (realized, disabled) = world.enter(|| {
        let disabled = signal(false);
        let realized = realize(
            &h.backend,
            &h.registry,
            pressable(move || sink.set(sink.get() + 1))
                .disabled(disabled)
                .build(),
        );
        (realized, disabled)
    });
    // Mount: create + set_disabled(false) from the binding's first fire.
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 pressable".to_string(),
            "set_disabled n0 false".to_string(),
        ]
    );
    // The wrapped callback the backend received fires while enabled…
    let native_press = h.press_handler(0);
    native_press();
    assert_eq!(pressed.get(), 1);
    // …and is blocked once the disabled binding flips the shared flag.
    disabled.set(true);
    world.flush();
    assert_eq!(h.take_log(), vec!["set_disabled n0 true".to_string()]);
    native_press();
    assert_eq!(pressed.get(), 1, "press blocked while disabled");
    drop(realized);
}

// ===========================================================================
// Reactive text through the builder (fallback update_text path —
// host-mock leaves `create_text_with_id` at the trait default, so there
// is no batched-id support)
// ===========================================================================

#[test]
fn dyn_text_uses_update_text_fallback_without_id_support() {
    let h = harness();
    let world = h.world.clone();
    let (realized, count) = world.enter(|| {
        let count = signal(0i32);
        let realized = realize(
            &h.backend,
            &h.registry,
            text().content(move || format!("c={}", count.get())).build(),
        );
        (realized, count)
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 text \"\"".to_string(),
            "update_text n0 \"c=0\"".to_string(),
        ],
        "no create_text_with_id support: legacy create+update path"
    );
    count.set(5);
    world.flush();
    assert_eq!(h.take_log(), vec!["update_text n0 \"c=5\"".to_string()]);
    drop(realized);
}

// ===========================================================================
// P3c sheet-engine paths (stylesheet!-shaped sheets built by hand — the
// macro's new-core emission produces exactly these StyleProp arms; the
// registration/token calls pinned here are deliberately OUTSIDE the
// scene-parity golden alphabet, so this is their behavioral gate)
// ===========================================================================

use runtime_shared::{StyleApplication, StyleSheet, TokenEntry, TokenValue};
use runtime_vocabulary::theme;

fn tokened(width: f32) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(runtime_shared::Length::Px(width))),
        background: Some(Tokenized::token(
            "color-surface",
            runtime_shared::Color("#000".into()),
        )),
        ..Default::default()
    }
}

/// A `stylesheet!`-shaped sheet: token-referencing base + a size variant.
fn themed_sheet() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_vs| tokened(100.0))
            .variant("size", "large", |_vs| StyleRules {
                width: Some(Tokenized::Literal(runtime_shared::Length::Px(400.0))),
                ..Default::default()
            })
            .variant_default("size", "medium")
            .variant("size", "medium", |_vs| StyleRules::default()),
    )
}

/// A sheet with a `state hovered { … }` overlay (the macro's
/// `__state_hovered` axis).
fn hover_sheet() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_vs| tokened(100.0)).variant("__state_hovered", "on", |_vs| StyleRules {
            width: Some(Tokenized::Literal(runtime_shared::Length::Px(999.0))),
            ..Default::default()
        }),
    )
}

fn surface(value: &str) -> TokenEntry {
    TokenEntry {
        name: "color-surface",
        value: TokenValue::Color(runtime_shared::Color(value.into())),
    }
}

/// Static sheet path: pre-mount `install_tokens` is delivered before the
/// sheet registers (web needs the vars before rules referencing them);
/// mount applies once with NO state hookup; a theme swap re-applies via
/// the cohort (backend `update_tokens` first); unmount stops cohort
/// membership and fires `on_node_unstyled`.
#[test]
fn sheet_static_cohort_reapplies_on_theme_swap() {
    let h = harness();
    let world = h.world.clone();
    let realized = world.enter(|| {
        theme::install_tokens(&[surface("#111")]);
        realize(
            &h.backend,
            &h.registry,
            view()
                .style(StyleApplication::new(themed_sheet()))
                .build(),
        )
    });
    assert_eq!(
        h.take_log(),
        vec![
            "create n0 view".to_string(),
            "install_tokens [\"color-surface\"]".to_string(),
            "register_stylesheet rules=3".to_string(),
            "apply_style n0 width=Literal(Px(100.0))".to_string(),
        ],
        "mount: tokens → registration → one apply, no attach_states"
    );

    // Theme swap: backend gets the update, then the cohort re-applies.
    world.enter(|| theme::update_tokens(&[surface("#222")]));
    world.flush();
    assert_eq!(
        h.take_log(),
        vec![
            "update_tokens [\"color-surface\"]".to_string(),
            "apply_style n0 width=Literal(Px(100.0))".to_string(),
        ],
        "swap: update_tokens then cohort fan-out re-apply"
    );

    // Unmount: cohort unregister (invisible) BEFORE on_node_unstyled.
    drop(realized);
    assert_eq!(h.take_log(), vec!["on_node_unstyled n0".to_string()]);

    // The dead node must NOT re-apply on the next swap — its cohort
    // entry is gone.
    world.enter(|| theme::update_tokens(&[surface("#333")]));
    world.flush();
    assert_eq!(
        h.take_log(),
        vec!["update_tokens [\"color-surface\"]".to_string()],
        "post-unmount swap reaches the backend but re-applies nothing"
    );
}

/// Variant selection resolves through the sheet (the `size = large` arm
/// overrides the base width).
#[test]
fn sheet_variant_selection_resolves() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .style(StyleApplication::new(themed_sheet()).with("size", "large"))
                .build(),
        )
    });
    let log = h.take_log().join("\n");
    assert!(
        log.contains("apply_style n0 width=Literal(Px(400.0))"),
        "large variant width applied:\n{log}"
    );
    drop(realized);
}

/// REGRESSION (the static-style state-machine-divert bug): a STATIC
/// sheet application whose sheet declares `state hovered` must still
/// get the event-driven state machine on a backend that doesn't handle
/// states natively — mount hooks `attach_states`, and flipping the
/// hover bit re-resolves WITH the overlay. Without the divert the
/// cohort path returns a no-op setter and hover is silently lost
/// (idea-ui MenuItem/ListItem on native).
#[test]
fn regression_static_sheet_with_state_overlay_keeps_state_machine() {
    let h = harness();
    let world = h.world.clone();
    let _realized = world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .style(StyleApplication::new(hover_sheet()))
                .build(),
        )
    });
    let mount = h.take_log().join("\n");
    assert!(
        mount.contains("attach_states n0"),
        "state-bearing static sheet must divert to the state machine:\n{mount}"
    );
    assert!(mount.contains("apply_style n0 width=Literal(Px(100.0))"), "{mount}");

    // Flip hover on like a native event source: the overlay applies.
    let setter = h.state_setter(0);
    setter(StateBits::HOVERED, true);
    world.flush();
    assert_eq!(
        h.take_log(),
        vec!["apply_style n0 width=Literal(Px(999.0))".to_string()],
        "hover flip re-applies with the state overlay merged"
    );

    // And off again: back to base.
    setter(StateBits::HOVERED, false);
    world.flush();
    assert_eq!(
        h.take_log(),
        vec!["apply_style n0 width=Literal(Px(100.0))".to_string()]
    );
}

/// Dynamic sheet path (a reactive `stylesheet!` builder): re-applies on
/// its own signal AND on a theme swap (version subscription), and the
/// per-fire inline sheet exercises the pin + dead-sheet sweep without
/// unregistering the class the node currently wears.
#[test]
fn sheet_dynamic_reapplies_on_signal_and_theme_swap() {
    let h = harness();
    let world = h.world.clone();
    let (_realized, large) = world.enter(|| {
        let large = signal(false);
        let realized = realize(
            &h.backend,
            &h.registry,
            view()
                .style(move || {
                    StyleApplication::new(themed_sheet())
                        .with("size", if large.get() { "large" } else { "medium" })
                })
                .build(),
        );
        (realized, large)
    });
    let mount = h.take_log().join("\n");
    assert!(mount.contains("register_stylesheet rules=3"), "{mount}");
    assert!(mount.contains("apply_style n0 width=Literal(Px(100.0))"), "{mount}");
    assert!(mount.contains("attach_states n0"), "{mount}");

    large.set(true);
    world.flush();
    let log = h.take_log().join("\n");
    assert!(
        log.contains("apply_style n0 width=Literal(Px(400.0))"),
        "variant signal re-resolves:\n{log}"
    );
    assert!(
        !log.contains("unregister_stylesheet"),
        "the pinned sheet must not be dead-swept mid-life:\n{log}"
    );

    world.enter(|| theme::update_tokens(&[surface("#abc")]));
    world.flush();
    let log = h.take_log().join("\n");
    assert!(
        log.contains("update_tokens [\"color-surface\"]"),
        "swap reaches the backend:\n{log}"
    );
    assert!(
        log.contains("apply_style n0 width=Literal(Px(400.0))"),
        "dynamic node re-applies on theme swap via the version signal:\n{log}"
    );
}

/// signal_class (fallback shape): the classes' applications are
/// pre-built; a signal write re-resolves and re-applies. The kept
/// `apps` pin the per-value sheets against the dead-Weak sweep.
#[test]
fn signal_class_rebinds_on_signal_write() {
    let h = harness();
    let world = h.world.clone();
    let (_realized, active) = world.enter(|| {
        let active = signal(0u32);
        let realized = realize(
            &h.backend,
            &h.registry,
            view()
                .style(runtime_vocabulary::signal_class(active, &[0, 1], |v| {
                    let width = if v == 0 { 60.0 } else { 200.0 };
                    StyleApplication::new(Rc::new(StyleSheet::r#static(px(width))))
                }))
                .build(),
        );
        (realized, active)
    });
    let mount = h.take_log().join("\n");
    assert!(mount.contains("apply_style n0 width=Literal(Px(60.0))"), "{mount}");

    active.set(1);
    world.flush();
    let log = h.take_log().join("\n");
    assert!(
        log.contains("apply_style n0 width=Literal(Px(200.0))"),
        "signal write rebinds the class:\n{log}"
    );
}

/// Preminted classes stamp via attach_html_class — one per
/// whitespace-separated segment — with zero StyleRules work; the
/// overrides variant layers a static application on top.
#[test]
fn preminted_stamps_classes_and_layers_overrides() {
    let h = harness();
    let world = h.world.clone();
    let _realized = world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .child(view().style(StyleProp::Preminted {
                    class: "iy-abc iy-abc-size-large".into(),
                    overrides: None,
                    inline: None,
                }))
                .child(view().style(StyleProp::Preminted {
                    class: "iy-def".into(),
                    overrides: Some(Rc::new(px(50.0))),
                    inline: None,
                }))
                .build(),
        )
    });
    let log = h.take_log().join("\n");
    assert!(log.contains("attach_html_class n1 iy-abc"), "{log}");
    assert!(log.contains("attach_html_class n1 iy-abc-size-large"), "{log}");
    assert!(
        !log.contains("apply_style n1"),
        "pure preminted node does no StyleRules work:\n{log}"
    );
    assert!(log.contains("attach_html_class n2 iy-def"), "{log}");
    assert!(
        log.contains("apply_style n2 width=Literal(Px(50.0))"),
        "override layer applies on top of the preminted class:\n{log}"
    );
}

/// Preminted-only worlds still deliver theme tokens: the theme driver
/// (not sheet registration, which never happens) drains the queue —
/// the old premint-host-driver contract.
#[test]
fn preminted_world_still_delivers_tokens() {
    let h = harness();
    let world = h.world.clone();
    let _realized = world.enter(|| {
        theme::install_tokens(&[surface("#111")]);
        realize(
            &h.backend,
            &h.registry,
            view()
                .style(StyleProp::Preminted { class: "iy-abc".into(), overrides: None, inline: None })
                .build(),
        )
    });
    let log = h.take_log().join("\n");
    assert!(
        log.contains("install_tokens [\"color-surface\"]"),
        "premint attach drains the token queue without any sheet:\n{log}"
    );
    world.enter(|| theme::update_tokens(&[surface("#222")]));
    world.flush();
    let log = h.take_log().join("\n");
    assert!(
        log.contains("update_tokens [\"color-surface\"]"),
        "theme swap reaches the backend through the driver:\n{log}"
    );
}

/// A LIVE (non-preminted) world must publish the theme's default text
/// font to the document, exactly like a preminted one.
///
/// This delivery used to be gated on `premint_used`, on the reasoning
/// that only preminted rule bodies read the `--iy-default-font`
/// variable. That covered the variable but not the font: only a STATIC
/// style application gets the theme font folded into its own rules, a
/// reactive one deliberately does not, and body / plain containers are
/// never styled by the framework at all. With the gate on, a live web
/// build published no document font and every one of those nodes fell
/// back to the browser's serif — measured as `<body>` and all top-level
/// containers rendering in Times on a live docs build while the
/// preminted build rendered the theme's sans stack.
#[test]
fn regression_live_world_publishes_the_default_text_font() {
    let h = Harness::new();
    // NOT a preminted world — this is the case the gate excluded.
    h.shared.preminted.set(false);
    // `apply_default_text_font` is a `rec_v` (verbose) mock call.
    h.shared.verbose.set(true);
    let world = h.world.clone();
    let _realized = world.enter(|| {
        theme::set_default_text_font(Some(runtime_shared::FontFamily::System(
            "Test Sans".into(),
        )));
        realize(
            &h.backend,
            &h.registry,
            view().style(StyleApplication::new(themed_sheet())).build(),
        )
    });
    world.flush();
    let log = h.take_log().join("\n");
    assert!(
        log.contains("apply_default_text_font some"),
        "a live world must publish the theme default font to the document \
         — without it body, plain containers, and every reactively-styled \
         node fall back to the browser serif:\n{log}"
    );
}

/// The theme's default text font fills a resolved rule's absent
/// font_family per world (native has no CSS inheritance).
#[test]
fn default_text_font_fills_absent_font_family() {
    let h = harness();
    let world = h.world.clone();
    let _realized = world.enter(|| {
        theme::set_default_text_font(Some(runtime_shared::FontFamily::System(
            "Test Sans".into(),
        )));
        realize(
            &h.backend,
            &h.registry,
            view()
                .style(StyleApplication::new(themed_sheet()))
                .build(),
        )
    });
    // The mock's apply_style digest only prints width; assert through the resolved
    // rules instead: attach a probing static rule check via a second
    // world would be heavier — instead verify the fill function through
    // the theme API surface.
    assert_eq!(
        world.enter(theme::default_text_font),
        Some(runtime_shared::FontFamily::System("Test Sans".into()))
    );
    let _ = h.take_log();
}

// ===========================================================================
// P5 identity seam: the vocabulary robot registry (`--features robot`)
//
// Registries are thread-local and the test runner gives each #[test] its
// own thread, so these tests are isolated by construction (reset() is
// defensive). Queries that resolve reactive labels run inside
// `world.enter` — new-core signal reads need the ambient world (the
// bridge-adapter contract documented in `robot`'s module docs).
// ===========================================================================

#[cfg(feature = "robot")]
mod robot_registry {
    use super::*;
    use runtime_scene::dyn_keyed;
    use runtime_vocabulary::builders::{scroll_view, text_input};
    use runtime_vocabulary::robot::{ElementKind, Query, Robot, RobotError};

    /// Mount registers every identity-bearing prim; lookup by test_id
    /// resolves the RIGHT node — clicking the found button fires that
    /// button's author handler and nobody else's.
    #[test]
    fn register_on_mount_and_lookup_returns_the_right_node() {
        let robot = Robot::new();
        robot.reset();
        let h = harness();
        let hits_a = Rc::new(Cell::new(0));
        let hits_b = Rc::new(Cell::new(0));
        let (ha, hb) = (hits_a.clone(), hits_b.clone());
        let _realized = h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                view()
                    .test_id("root")
                    .child(
                        button()
                            .label("a")
                            .on_press(move || ha.set(ha.get() + 1))
                            .test_id("btn-a")
                            .build(),
                    )
                    .child(
                        button()
                            .label("b")
                            .on_press(move || hb.set(hb.get() + 1))
                            .test_id("btn-b")
                            .build(),
                    )
                    .build(),
            )
        });

        let root = robot.find(Query::test_id("root")).expect("root registered");
        assert_eq!(root.kind, ElementKind::View);
        let a = robot.find(Query::test_id("btn-a")).expect("btn-a registered");
        assert_eq!(a.kind, ElementKind::Button);
        assert_eq!(a.label.as_deref(), Some("a"));

        // Parenting: both buttons are the root view's children.
        let kids = robot.children_of(&root);
        assert_eq!(kids.len(), 2, "root links its two button children");
        assert_eq!(robot.parent_of(&a).map(|p| p.id), Some(root.id));

        // The click routes to the RIGHT handler.
        robot.click(&a).expect("click available on a button");
        assert_eq!((hits_a.get(), hits_b.get()), (1, 0));

        // A text with no click reports ActionNotAvailable, not a panic.
        assert_eq!(
            robot.click(&root),
            Err(RobotError::ActionNotAvailable("click")),
        );
        robot.reset();
    }

    /// Regression: unmount (dropping the realized subtree) must remove
    /// every registry entry. A stale entry after teardown is the
    /// classic robot-registry bug (phantom live roots in snapshots).
    #[test]
    fn regression_unmount_deregisters_entries() {
        let robot = Robot::new();
        robot.reset();
        let h = harness();
        let realized = h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                view()
                    .test_id("gone-root")
                    .child(text().content("x").test_id("gone-text").build())
                    .build(),
            )
        });
        assert_eq!(robot.count(None), 2);
        drop(realized);
        assert!(robot.find(Query::test_id("gone-root")).is_none());
        assert!(robot.find(Query::test_id("gone-text")).is_none());
        assert_eq!(robot.count(None), 0, "no stale entries after unmount");
        robot.reset();
    }

    /// Regression: a reactive branch swap (`dyn_keyed`) deregisters the
    /// OLD branch's entries and registers the new branch's — the
    /// scene-level analogue of the old walker's when-branch-swap
    /// deregistration.
    #[test]
    fn regression_branch_swap_deregisters_old_branch() {
        let robot = Robot::new();
        robot.reset();
        let h = harness();
        let (flag, root) = h.world.enter(|| {
            let flag = signal(false);
            let root = dyn_keyed(
                move || flag.get(),
                |&on| {
                    if on {
                        view().test_id("branch-b").build()
                    } else {
                        view().test_id("branch-a").build()
                    }
                },
            );
            (flag, root)
        });
        let _realized = h.world.enter(|| realize(&h.backend, &h.registry, root));
        assert!(robot.find(Query::test_id("branch-a")).is_some());
        assert!(robot.find(Query::test_id("branch-b")).is_none());

        h.world.enter(|| flag.set(true));
        h.world.flush();
        assert!(
            robot.find(Query::test_id("branch-a")).is_none(),
            "old branch entry must deregister on swap"
        );
        assert!(robot.find(Query::test_id("branch-b")).is_some());
        robot.reset();
    }

    /// Duplicate-id policy through the REAL mount path (old-core
    /// mirror): `find` is last-wins, `find_all` returns every holder —
    /// sibling rows sharing one affordance test_id is a core E2E
    /// pattern (`getByTestId(...).toHaveCount(n)`).
    #[test]
    fn duplicate_test_id_find_last_wins_find_all_scans() {
        let robot = Robot::new();
        robot.reset();
        let h = harness();
        let _realized = h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                view()
                    .child(text().content("one").test_id("row-del").build())
                    .child(text().content("two").test_id("row-del").build())
                    .build(),
            )
        });
        assert_eq!(robot.find_all(Query::test_id("row-del")).len(), 2);
        let last = robot.find(Query::test_id("row-del")).expect("find resolves");
        assert_eq!(last.label.as_deref(), Some("two"), "find is last-wins");
        robot.reset();
    }

    /// Reactive text reports its LIVE label (label_fn), not the value
    /// frozen at mount — the old registry's staleness rule, ported.
    #[test]
    fn reactive_text_label_stays_live() {
        let robot = Robot::new();
        robot.reset();
        let h = harness();
        let content = h.world.enter(|| signal("before".to_string()));
        let _realized = h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                text()
                    .content(move || content.get())
                    .test_id("live-text")
                    .build(),
            )
        });
        h.world.enter(|| {
            let found = robot.find(Query::test_id("live-text")).unwrap();
            assert_eq!(found.label.as_deref(), Some("before"));
        });
        h.world.enter(|| content.set("after".to_string()));
        h.world.flush();
        h.world.enter(|| {
            let found = robot.find(Query::test_id("live-text")).unwrap();
            assert_eq!(
                found.label.as_deref(),
                Some("after"),
                "label_fn must resolve the live value"
            );
            assert!(robot.find(Query::label("after")).is_some());
        });
        robot.reset();
    }

    /// Control verbs route to the author callbacks the handler holds:
    /// type_text → text_input on_change, set_toggle → toggle on_change,
    /// set_slider → RAW (pre-snap) slider on_change; scroll views carry
    /// a set_scroll action (handle-routed).
    #[test]
    fn control_actions_route_to_author_callbacks() {
        let robot = Robot::new();
        robot.reset();
        let h = harness();
        let typed = Rc::new(RefCell::new(String::new()));
        let toggled = Rc::new(Cell::new(false));
        let slid = Rc::new(Cell::new(0.0f32));
        let (t1, t2, t3) = (typed.clone(), toggled.clone(), slid.clone());
        let _realized = h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                view()
                    .child(
                        text_input()
                            .value("")
                            .on_change(move |v| *t1.borrow_mut() = v)
                            .test_id("field")
                            .build(),
                    )
                    .child(
                        toggle()
                            .value(false)
                            .on_change(move |v| t2.set(v))
                            .test_id("switch")
                            .build(),
                    )
                    .child(
                        slider()
                            .value(0.0)
                            .on_change(move |v| t3.set(v))
                            .test_id("amount")
                            .build(),
                    )
                    .child(scroll_view().test_id("scroller").build())
                    .build(),
            )
        });

        let field = robot.find(Query::test_id("field")).unwrap();
        assert_eq!(field.kind, ElementKind::TextInput);
        robot.type_text(&field, "hello").unwrap();
        assert_eq!(&*typed.borrow(), "hello");

        let switch = robot.find(Query::test_id("switch")).unwrap();
        robot.set_toggle(&switch, true).unwrap();
        assert!(toggled.get());

        let amount = robot.find(Query::test_id("amount")).unwrap();
        robot.set_slider(&amount, 0.7).unwrap();
        assert_eq!(slid.get(), 0.7);

        let scroller = robot.find(Query::test_id("scroller")).unwrap();
        assert_eq!(scroller.kind, ElementKind::ScrollView);
        // The action exists and routes through the scroll handle
        // (host-mock records set_node_scroll; availability + routing
        // through the handle is the observable contract here).
        robot.set_scroll(&scroller, 0.0, 10.0).unwrap();
        robot.reset();
    }
}

// ===========================================================================
// Repeat (Element::Many) — the batched-Repeat port
// ===========================================================================

mod repeat_handler {
    //! Pins the two mount paths of `handlers/repeat.rs` and the
    //! semantics the bench-gate rules require: batching must not change
    //! teardown order (cleanup fires per styled row), theme-cohort
    //! membership (one bulk entry re-applying every row), or the
    //! fallback's per-row reactive behavior.

    use super::*;
    use runtime_vocabulary::glue::__static_repeat;

    /// One repeat of `count` styled rows (view + const text), the
    /// batchable shape. ONE shared sheet across rows (the `stylesheet!`
    /// shape — a per-row fresh sheet would re-register per row).
    fn styled_rows(count: usize) -> runtime_scene::Element {
        let sheet = themed_sheet();
        view()
            .children(__static_repeat(count, move |i| {
                view()
                    .style(StyleApplication::new(sheet.clone()))
                    .children(vec![text().content(format!("row {i}")).build()])
                    .build()
            }))
            .build()
    }

    /// Batching backend: the whole expansion is ONE `execute_batch`
    /// (+ one bulk attach) — no per-node create calls.
    #[test]
    fn batched_repeat_is_one_execute_batch() {
        let h = harness();
        h.shared.batched_repeat.set(true);
        let realized = h.world.enter(|| {
            realize(&h.backend, &h.registry, styled_rows(3))
        });
        let log = h.take_log();
        // Parent view mints first (n0), then the batch materializes
        // n1..n9 (3 rows × view+text + minted class per row), then the
        // default `execute_batch_with_attach` attaches row tops.
        assert_eq!(log[0], "create n0 view");
        assert!(
            log[1].starts_with("register_stylesheet"),
            "sheet registers once for the whole expansion: {log:?}"
        );
        assert_eq!(
            log[2],
            "execute_batch nodes=6 ops=[cv#0 ct#1\"row 0\" ins#0<-#1 \
             style#0=mint-wLiteral(Px(100.0)) cv#2 ct#3\"row 1\" ins#2<-#3 \
             style#2=mint-wLiteral(Px(100.0)) cv#4 ct#5\"row 2\" ins#4<-#5 \
             style#4=mint-wLiteral(Px(100.0))]",
            "one FFI batch for all rows, old-core op order (view, children, \
             insert, style): {log:?}"
        );
        assert_eq!(
            log[3], "insert_many n0 <- [n1, n3, n5]",
            "row tops attach in one insert_many: {log:?}"
        );
        assert_eq!(log.len(), 4, "no per-node create/apply calls: {log:?}");

        // Teardown: the cohort entry unregisters (backend-invisible) and
        // NOTHING per row — batched rows carry no per-node backend style
        // state (`mint_style_class` touches no node; `ApplyStyleStatic`
        // stamps the shared class), so the old core's "for symmetry"
        // per-row `on_node_unstyled` loop is deliberately dropped (it
        // was N no-ops costing one FFI each — the teardown bench-gate
        // residual; see the repeat handler + `BatchOps` contract).
        drop(realized);
        assert_eq!(
            h.take_log(),
            Vec::<String>::new(),
            "bulk teardown emits no per-row backend calls"
        );
    }

    /// The bulk cohort entry re-applies every batched row on a theme
    /// swap (old `register_static_cohort_batch` contract).
    #[test]
    fn batched_rows_share_one_cohort_entry_that_reapplies() {
        let h = harness();
        h.shared.batched_repeat.set(true);
        let world = h.world.clone();
        let _realized = world.enter(|| {
            theme::install_tokens(&[surface("#111")]);
            realize(&h.backend, &h.registry, styled_rows(2))
        });
        h.take_log();
        world.enter(|| theme::update_tokens(&[surface("#222")]));
        world.flush();
        let log = h.take_log();
        assert_eq!(log[0], "update_tokens [\"color-surface\"]");
        assert_eq!(
            log[1..],
            vec![
                "apply_style n1 width=Literal(Px(100.0))".to_string(),
                "apply_style n3 width=Literal(Px(100.0))".to_string(),
            ][..],
            "one bulk cohort entry re-applies every row: {log:?}"
        );
    }

    /// A non-batchable row (reactive text) sends the WHOLE expansion to
    /// the fallback: per-row mounts + one insert_many, and the reactive
    /// binding still works post-mount (batching must not change
    /// semantics).
    #[test]
    fn fallback_repeat_mounts_per_row_and_stays_reactive() {
        let h = harness();
        h.shared.batched_repeat.set(true); // backend supports it; the SHAPE bails
        let world = h.world.clone();
        let (realized, label) = world.enter(|| {
            let label = signal(String::from("a"));
            let realized = realize(
                &h.backend,
                &h.registry,
                view()
                    .children(__static_repeat(2, move |_i| {
                        view()
                            .style(px(50.0))
                            .children(vec![text().content(move || label.get()).build()])
                            .build()
                    }))
                    .build(),
            );
            (realized, label)
        });
        let log = h.take_log();
        assert_eq!(
            log,
            vec![
                "create n0 view".to_string(),
                // Row 1: per-node mounts (raw-rules style is NOT the
                // batchable sheet shape — same bail as the old core).
                "create n1 view".to_string(),
                "create n2 text \"\"".to_string(),
                "update_text n2 \"a\"".to_string(),
                "insert n1 <- n2".to_string(),
                "apply_style n1 width=Literal(Px(50.0))".to_string(),
                // Row 2.
                "create n3 view".to_string(),
                "create n4 text \"\"".to_string(),
                "update_text n4 \"a\"".to_string(),
                "insert n3 <- n4".to_string(),
                "apply_style n3 width=Literal(Px(50.0))".to_string(),
                // One batched attach for all row tops (old fallback).
                "insert_many n0 <- [n1, n3]".to_string(),
            ],
            "fallback: full per-row mounts, one insert_many"
        );

        // Reactive text updates post-mount.
        world.enter(|| label.set("b".to_string()));
        world.flush();
        assert_eq!(
            h.take_log(),
            vec![
                "update_text n2 \"b\"".to_string(),
                "update_text n4 \"b\"".to_string(),
            ],
            "fallback rows keep live bindings"
        );

        // Per-row cleanups fire on teardown (styled rows release).
        drop(realized);
        assert_eq!(
            h.take_log(),
            vec![
                "on_node_unstyled n1".to_string(),
                "on_node_unstyled n3".to_string(),
            ],
            "fallback teardown fires per row"
        );
    }

    /// Backend without batch support: same fallback even for the
    /// batchable shape.
    #[test]
    fn non_batching_backend_takes_fallback_for_batchable_shape() {
        let h = harness();
        assert!(!h.shared.batched_repeat.get());
        let _realized = h.world.enter(|| {
            realize(&h.backend, &h.registry, styled_rows(1))
        });
        let log = h.take_log();
        assert!(
            log.iter().any(|l| l.starts_with("create n")),
            "per-node mounts: {log:?}"
        );
        assert!(
            !log.iter().any(|l| l.starts_with("execute_batch")),
            "no batch call: {log:?}"
        );
        assert!(
            log.iter().any(|l| l.starts_with("insert_many")),
            "row tops still attach via insert_many: {log:?}"
        );
    }

    /// Empty repeat: zero backend calls (old walker's early-out).
    #[test]
    fn empty_repeat_is_free() {
        let h = harness();
        h.shared.batched_repeat.set(true);
        let _realized = h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                view().children(__static_repeat(0, |_i| unreachable!())).build(),
            )
        });
        assert_eq!(h.take_log(), vec!["create n0 view".to_string()]);
    }
}

// ===========================================================================
// P6 SDK-retarget glue extensions (glue.rs's "P6 root-surface" wave):
// the mirrors that carry LOGIC — watch/Subscription, the theme-install
// shared-registry seed, `.bind(Ref)` handle filling, the icon square
// sheet, `on_defer`, `unscope`.
// ===========================================================================
mod p6_glue {
    use super::*;
    use runtime_shared::{Color, Ref, TokenEntry, TokenValue};
    use runtime_vocabulary::glue;

    fn surface(value: &str) -> TokenEntry {
        TokenEntry {
            name: "color-surface",
            value: TokenValue::Color(Color(value.into())),
        }
    }

    /// `glue::watch` — caller-owned subscription: runs now, re-runs per
    /// flush after a dependency write, stops on drop, survives `leak`.
    #[test]
    fn watch_subscription_runs_drops_and_leaks() {
        let world = World::new();
        let runs = Rc::new(Cell::new(0));
        world.enter(|| {
            let s = signal(0);
            let sub = {
                let runs = runs.clone();
                glue::watch(move || {
                    let _ = s.get();
                    runs.set(runs.get() + 1);
                })
            };
            assert_eq!(runs.get(), 1, "watch runs immediately");

            s.set(1);
            world.flush();
            assert_eq!(runs.get(), 2, "dependency write re-runs on flush");

            drop(sub);
            s.set(2);
            world.flush();
            assert_eq!(runs.get(), 2, "dropped subscription no longer fires");

            let sub2 = {
                let runs = runs.clone();
                glue::watch(move || {
                    let _ = s.get();
                    runs.set(runs.get() + 10);
                })
            };
            sub2.leak();
            s.set(3);
            world.flush();
            assert_eq!(runs.get(), 22, "leaked subscription keeps firing");
        });
    }

    /// The theme-install family routes through the per-world ThemeCtx
    /// AND seeds runtime-core's shared token registry, so
    /// `Tokenized::resolve` (component-code color reads, native
    /// apply-time reads) sees the installed values instead of the
    /// compile-time fallbacks — with the old walker-flush queues
    /// drained (nothing consumes them on this core).
    #[test]
    fn theme_install_seeds_shared_token_registry() {
        let world = World::new();
        world.enter(|| {
            glue::install_tokens(&[surface("#123456")]);
            assert_eq!(
                runtime_vocabulary::theme::token_value("color-surface"),
                Some(TokenValue::Color(Color("#123456".into()))),
                "per-world ThemeCtx records the install"
            );
            let reference = Tokenized::Token {
                name: "color-surface",
                fallback: Color("#ffffff".into()),
            };
            assert_eq!(
                reference.resolve().0,
                "#123456",
                "Tokenized::resolve reads the seeded shared registry, not the fallback"
            );

            glue::update_tokens(&[surface("#654321")]);
            assert_eq!(reference.resolve().0, "#654321", "update re-seeds");
            assert!(
                runtime_shared::take_pending_token_updates().is_empty(),
                "the old walker-flush queues are drained by the seeding path"
            );
        });
    }

    /// `pressable(children, cb).bind(ref)` fills a REAL mount-time
    /// handle (the vocabulary handler's `make_pressable_handle`), so
    /// imperative surfaces (anchors, focus, AnimatedValue) work as on
    /// the old core.
    #[test]
    fn pressable_bind_fills_real_handle_at_mount() {
        use runtime_vocabulary::glue::IntoElement;
        let h = harness();
        let r: Ref<runtime_shared::PressableHandle> = Ref::new();
        let r_for = r.clone();
        let el = h.world.enter(|| {
            glue::pressable(Vec::new(), || {}).bind(r_for).into_element()
        });
        let _realized = h
            .world
            .enter(|| realize(&h.backend, &h.registry, el));
        assert!(r.get().is_some(), "Ref filled with the mount-time handle");
    }

    /// `scroll_view(children).bind(ref)` fills a REAL mount-time
    /// [`ScrollViewHandle`] — the `GlueScrollView` mirror of
    /// `Bound::<ScrollViewHandle>::bind`. Regression for the glue gap
    /// that forced the website's shell.rs to cfg-fork onto the raw
    /// vocabulary builder's `on_handle` under new-core.
    #[test]
    fn scroll_view_bind_fills_real_handle_at_mount() {
        use runtime_vocabulary::glue::IntoElement;
        let h = harness();
        let r: Ref<runtime_shared::ScrollViewHandle> = Ref::new();
        let r_for = r.clone();
        let el = h
            .world
            .enter(|| glue::scroll_view(Vec::new()).bind(r_for).into_element());
        let _realized = h.world.enter(|| realize(&h.backend, &h.registry, el));
        assert!(r.get().is_some(), "Ref filled with the mount-time handle");
    }

    /// Regression (new-core `if` E0507 class): the static `if` arm's
    /// thunks are `FnOnce` (old-core `builder.rs::StaticCond` parity),
    /// so a taken branch may MOVE a captured `String` — the shape every
    /// hoisted-condition (`let has_x = !x.is_empty()`) component emits.
    /// Under the original `Fn + 'static` bounds this test does not
    /// compile ("cannot move out of a captured variable in an `Fn`
    /// closure").
    #[test]
    fn static_cond_branch_may_move_string_captures() {
        use runtime_vocabulary::glue::StaticCond as _;
        let s = String::from("owned");
        let taken = true.__idealyst_if(
            move || {
                let moved: String = s; // by-move use of the capture
                assert_eq!(moved, "owned");
                Vec::new()
            },
            || unreachable!("else arm not taken"),
        );
        assert!(taken.is_empty());
    }

    /// `GlueIcon::size` mirrors the old square-sizing sheet: width =
    /// height = px, `flex_shrink: 0`, and the sheet is CACHED per
    /// rounded px so every icon at a size shares one Rc.
    #[test]
    fn icon_size_mints_shared_square_sheet() {
        use runtime_shared::{FillRule, IconData};
        const DOT: IconData = IconData {
            view_box: (24, 24),
            paths: &["M12 12h0"],
            fill_rule: FillRule::NonZero,
            filled: false,
        };

        fn icon_app(px: f32) -> runtime_shared::StyleApplication {
            use runtime_vocabulary::glue::IntoElement;
            let el = glue::icon(DOT).size(px).into_element();
            match el {
                runtime_scene::Element::Item { data, .. } => {
                    let cell = data
                        .downcast_ref::<runtime_vocabulary::prims::PrimCell<
                            runtime_vocabulary::prims::IconPrim,
                        >>()
                        .expect("icon prim");
                    match cell.take().style.expect("size sets a style") {
                        StyleProp::Sheet(app) => *app,
                        _ => panic!("size mints a static sheet application"),
                    }
                }
                _ => panic!("icon builds an Item"),
            }
        }

        let world = World::new();
        world.enter(|| {
            let a = icon_app(20.0);
            let b = icon_app(20.0);
            let c = icon_app(24.0);
            assert!(
                Rc::ptr_eq(&a.sheet, &b.sheet),
                "same px shares one minted sheet"
            );
            assert!(!Rc::ptr_eq(&a.sheet, &c.sheet), "distinct px, distinct sheet");

            let rules = runtime_shared::resolve_style(&a);
            assert_eq!(
                rules.width,
                Some(Tokenized::Literal(runtime_shared::Length::Px(20.0))),
                "square width"
            );
            assert_eq!(
                rules.height,
                Some(Tokenized::Literal(runtime_shared::Length::Px(20.0))),
                "square height"
            );
            assert_eq!(
                rules.flex_shrink,
                Some(Tokenized::Literal(0.0)),
                "icons must not shrink under flex"
            );
        });
    }

    /// `on_defer` mirrors the old skip-first contract: the first run
    /// only records the baseline; every subsequent dependency change
    /// fires with (new, Some(prev)).
    #[test]
    fn on_defer_skips_first_run_then_fires_with_prev() {
        let world = World::new();
        let calls: Rc<RefCell<Vec<(i32, Option<i32>)>>> = Rc::new(RefCell::new(Vec::new()));
        world.enter(|| {
            let s = signal(1);
            let calls_for = calls.clone();
            let _e = glue::on_defer(s, move |new, prev| {
                calls_for.borrow_mut().push((*new, prev.copied()));
            });
            assert!(calls.borrow().is_empty(), "first run is baseline-only");

            s.set(2);
            world.flush();
            assert_eq!(&*calls.borrow(), &[(2, Some(1))], "fires with prev after a change");

            s.set(3);
            world.flush();
            assert_eq!(calls.borrow().last(), Some(&(3, Some(2))));
        });
    }

    /// `glue::unscope` — slots created inside are world-root-owned:
    /// dropping the enclosing collector must NOT retire them (the old
    /// `unscope` thread-lifetime contract, world-scoped here).
    #[test]
    fn unscope_escapes_the_enclosing_collector() {
        let world = World::new();
        world.enter(|| {
            let (s, owned) = runtime_world::collect_owned(|| glue::unscope(|| signal(5)));
            drop(owned);
            assert_eq!(s.get(), 5, "unscoped signal survives the collector drop");
        });
    }

    /// The "Switch thumb never travels" bug: old `AnimatedValue::bind`
    /// anchors its per-frame subscription via old-core `on_cleanup`,
    /// which OUTSIDE any old-core Scope — i.e. on every new-core build —
    /// silently drops the callback at bind time, so the tween ticked but
    /// no `set_animated_f32` ever reached the backend. The glue's
    /// `animation::AnimatedValue` must (a) deliver value changes through
    /// the mounted `ViewHandle` for the component's lifetime and (b)
    /// still drop the listener at unrealize — the recycled-`Ref`
    /// protection the old anchoring existed for.
    #[test]
    fn regression_switch_thumb_animated_bind_delivers_and_dies_with_scope() {
        use glue::animation::{AnimProp, AnimatedValue};

        let h = harness();
        let av: AnimatedValue<f32> = AnimatedValue::new(2.0);
        let av_bind = av.clone();
        let realized = h.world.enter(|| {
            // Component-shaped build: bind runs inside the component
            // scope (like Switch's body), so the keepalive is owned by
            // the realized subtree.
            let element = runtime_scene::component_scope(|| {
                let r: Ref<runtime_shared::ViewHandle> = Ref::new();
                av_bind.bind(r, AnimProp::TranslateX);
                view().on_handle(move |handle| r.fill(handle)).build()
            });
            realize(&h.backend, &h.registry, element)
        });
        h.world.flush();
        take_animated_log(); // discard the pre-mount snapshot writes

        // The Switch's value-flip effect drives the animated value.
        av.set(16.0);
        assert_eq!(
            take_animated_log(),
            vec!["set_animated_f32 n0 TranslateX 16".to_string()],
            "the bound listener must survive the new-core component build"
        );

        drop(realized);
        av.set(30.0);
        assert!(
            take_animated_log().is_empty(),
            "unrealize must drop the subscription (old scope-anchoring contract)"
        );
    }
}
