//! `form(FormProps { .. }).with_style(…).bind(r)` and the `Form` `ui!`
//! tag shape, mounted through the scene registry on the shared
//! `host-mock` recording substrate.
//!
//! The host target exercises the NON-web arm of [`form::register`]: the
//! External-placeholder path extended with children (create → children
//! → style → ref fill → teardown). The load-bearing contract is that
//! the form's children realize INTO the external node (on web that
//! containment is what makes autofill + submit-on-enter work). The web
//! (`<form>` + submit listener) handler is `WebBackend`-concrete and is
//! covered by the wasm32 check gate.

use std::cell::Cell;
use std::rc::Rc;

use form::prelude::*;
use host_mock::Harness;
use runtime_shared::{Ref, StyleRules, Tokenized};
use runtime_scene::Realized;
use runtime_vocabulary::builders;
use runtime_vocabulary::glue::{BuildElement, IntoElement};

fn harness() -> Harness {
    // The SDK's boot registration seam — the same fn an app passes to
    // `backend_web::newcore::start_in` / `backend_ssr::newcore::
    // render_path_with`.
    let h = Harness::with_registry(|r| form::register(r));
    // The exact-log expectations below predate the update_text /
    // on_node_unstyled / mark_container op families (the codeblock
    // suite's convention).
    h.mute(&["update_text", "on_node_unstyled", "mark_container"]);
    h
}

fn two_field_form() -> FormProps {
    FormProps {
        on_submit: None,
        children: vec![
            builders::text().content("email").build(),
            builders::text().content("submit").build(),
        ],
    }
}

/// The load-bearing containment assertion: the form mounts as ONE
/// external node and its children realize INTO it (`insert n0 <- …`) —
/// the create → children order. If children mounted beside the external
/// instead, the web `<form>` would lose autofill + submit-on-enter.
#[test]
fn children_realize_into_the_external_form_node() {
    let h = harness();
    let el = form(two_field_form()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops().join("\n");
    assert_eq!(
        log,
        "create n0 external form::FormProps\n\
         create n1 text \"email\"\n\
         insert n0 <- n1\n\
         create n2 text \"submit\"\n\
         insert n0 <- n2",
        "form mount shape drifted from the External-with-children path"
    );
}

/// Author style attaches to the external node AFTER the children
/// (create → children → style).
#[test]
fn author_style_lands_on_the_form_node_after_children() {
    let h = harness();
    let mut author = StyleRules::default();
    author.width = Some(Tokenized::Literal(runtime_shared::Length::Px(320.0)));
    let el = form(two_field_form()).with_style(author).into_element();
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops();
    let style_at = log
        .iter()
        .position(|l| l.starts_with("apply_style n0"))
        .expect("author style must attach to the external form node");
    let last_insert_at = log
        .iter()
        .rposition(|l| l.starts_with("insert n0"))
        .expect("children must insert into the form node");
    assert!(
        style_at > last_insert_at,
        "author style must attach AFTER the children (mount order): {log:?}"
    );
}

/// `bind` fills the ref at mount; the host fallback ops make
/// `submit()` a documented no-op
/// (`UnsupportedOps` — there is no form machinery to drive off-web).
#[test]
fn bind_fills_the_ref_at_mount_with_the_fallback_ops() {
    let h = harness();
    let r: Ref<FormHandle> = Ref::new();
    let el = form(FormProps::default()).bind(r.clone()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    let observed = r.with(|handle| {
        handle.submit(); // fallback ops: must be a silent no-op
        true
    });
    assert_eq!(
        observed,
        Some(true),
        "ref must be filled at mount; fallback submit must not panic"
    );
    assert!(
        !h.ops().iter().any(|l| l.contains("submit")),
        "fallback submit drives no backend op"
    );
}

/// Unmount releases the external node (teardown-guard contract).
#[test]
fn teardown_releases_the_external_node() {
    let h = harness();
    let el = form(FormProps::default()).into_element();
    let realized: Realized<u32> = h.mount(el);
    let _ = h.take_log();

    drop(realized);
    let log = h.ops();
    assert!(
        log.iter().any(|l| l == "release_external n0"),
        "unmount must release the external node: {log:?}"
    );
}

/// The `ui!`-tag contract: `ui! { Form(on_submit = ..) { .. } }`
/// compiles to `BuildElement::build(Form { .., ..defaults() })` — the
/// manual impl must route through `form(..)` and yield the same item,
/// children flowing into the external node.
///
/// The author callback is not observable post-mount on the host
/// placeholder (no submit listener exists off-web — firing it is the
/// wasm32 leg's behavior), so this asserts the tag path's structure:
/// payload dispatch + children containment. The callback closure is
/// still exercised for reachability before the build consumes it.
#[test]
fn form_tag_builds_through_build_element_with_children_and_callback() {
    let fired = Rc::new(Cell::new(false));
    let on_submit: Rc<dyn Fn()> = {
        let fired = fired.clone();
        Rc::new(move || fired.set(true))
    };
    // The closure the tag carries is the author's: invoking the same Rc
    // the props hold runs it (reachability — the wasm32 submit listener
    // clones exactly this Rc).
    let props = Form {
        on_submit: Some(on_submit.clone()),
        children: vec![builders::text().content("email").build()],
    };
    (props.on_submit.as_ref().expect("wired"))();
    assert!(fired.get(), "the tag's on_submit Rc is the author closure");

    let h = harness();
    let el = BuildElement::build(props);
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops().join("\n");
    assert_eq!(
        log,
        "create n0 external form::FormProps\n\
         create n1 text \"email\"\n\
         insert n0 <- n1",
        "the Form tag's BuildElement path must mount identically to form(..)"
    );
}

/// `FormHandle: PartialEq` by node identity — required because
/// `Signal<T>` is bounded on `T: PartialEq` at creation and `get`, so an
/// author who parks a bound handle in state (to submit from a toolbar
/// button, say) cannot otherwise compile. The orphan rule means only the
/// `form` crate can supply the impl.
///
/// Both directions matter: clones of ONE handle must compare equal (a
/// guarded `set` of the same handle must not notify), and handles onto
/// two DIFFERENT mounted forms must compare unequal (swapping which form
/// you drive must notify).
#[test]
fn form_handles_compare_by_mounted_node_identity() {
    let h = harness();

    let a: Ref<FormHandle> = Ref::new();
    let b: Ref<FormHandle> = Ref::new();
    let _ra: Realized<u32> = h.mount(form(FormProps::default()).bind(a.clone()).into_element());
    let _rb: Realized<u32> = h.mount(form(FormProps::default()).bind(b.clone()).into_element());

    let ha = a.with(|handle| handle.clone()).expect("form a mounted");
    let hb = b.with(|handle| handle.clone()).expect("form b mounted");

    assert!(
        ha == ha.clone(),
        "clones of one handle name the same form and must compare equal"
    );
    assert!(
        ha != hb,
        "handles onto two different mounted forms must compare unequal — \
         the `ops` vtable they share must not make them look identical"
    );
}
