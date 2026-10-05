//! SSR's `<head>` for a sheet with breakpoint and state overlays, through
//! the real vocabulary path (`realize` → `attach_style` → the SSR
//! backend). Each overlay rule must hold only the properties its own
//! block sets — the layering contract `StyleSheet::resolve` defines
//! (base < breakpoints < containers < author axes < states).

use std::cell::RefCell;
use std::rc::Rc;

use backend_ssr::SsrBackend;
use runtime_scene::{realize, Registry};
use runtime_shared::{Color, Length, StyleApplication, StyleRules, StyleSheet, Tokenized};
use runtime_vocabulary::builders::view;
use runtime_vocabulary::caps::LifecycleOps;
use runtime_vocabulary::register_builtins;
use runtime_world::World;

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

/// Mount one sheet-styled view and return `(class, head_css)`.
fn render(app: StyleApplication) -> (String, String) {
    let backend = Rc::new(RefCell::new(SsrBackend::new()));
    let mut registry: Registry<SsrBackend> = Registry::new();
    register_builtins(&mut registry);
    let registry = Rc::new(registry);
    let world = World::new();
    let realized = world.enter(|| realize(&backend, &registry, view().style(app).build()));
    let root = realized.collect_nodes().pop().expect("one root");
    LifecycleOps::finish(&mut *backend.borrow_mut(), root);
    world.flush();
    let html = backend.borrow().into_html();
    let class = html
        .split("class=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("the view wears a minted class")
        .to_string();
    let head = backend.borrow().head_css();
    (class, head)
}

/// The rule `head_css` emits for `selector` (with `.{class}` already in it).
fn rule_body<'a>(head: &'a str, selector: &str) -> &'a str {
    let at = head.find(selector).unwrap_or_else(|| panic!("no `{selector}` in {head}"));
    let rest = &head[at + selector.len()..];
    let open = rest.find('{').expect("rule body");
    let close = rest.find('}').expect("rule end");
    &rest[open + 1..close]
}

/// Regression (CrewForge want_c5b3c05a): `base { min_height: 44 }`,
/// `breakpoint sm { min_height: 0 }`, `breakpoint md { padding_top: 8 }`
/// and `state hovered { background }`. SSR emitted every overlay as a
/// FULL resolution, so the (0,2,0) `:hover` rule re-declared the base
/// `min-height: 44px` over the active `sm` rule (a hovered chip at 1440px
/// grew from 31px to 44px), and the `md` rule re-declared it over `sm`'s.
#[test]
fn regression_state_overlay_erases_breakpoint_arm_ssr_head() {
    let sheet = Rc::new(
        StyleSheet::new(|_| StyleRules { min_height: px(44.0), padding_top: px(4.0), ..Default::default() })
            .variant("__bp_sm", "on", |_| StyleRules { min_height: px(0.0), ..Default::default() })
            .variant("__bp_md", "on", |_| StyleRules { padding_top: px(8.0), ..Default::default() })
            .variant("__state_hovered", "on", |_| StyleRules {
                background: Some(Tokenized::Literal(Color("#333333".into()))),
                ..Default::default()
            }),
    );
    let (class, head) = render(StyleApplication::new(sheet));

    let hover = rule_body(&head, &format!(".{class}:hover"));
    assert!(hover.contains("background"), "the state's own property: {head}");
    assert!(!hover.contains("min-height"), "the state must not re-declare the base min-height: {head}");

    let sm = rule_body(&head, &format!("(min-width: 640px) {{ .{class} "));
    assert!(sm.contains("min-height: 0px"), "{head}");
    let md = rule_body(&head, &format!("(min-width: 768px) {{ .{class} "));
    assert!(md.contains("padding-top: 8px"), "{head}");
    assert!(!md.contains("min-height"), "md must not re-declare the base min-height over sm's: {head}");
}
