//! Ref binding lifecycle: `.bind(ref)` produces a Ref that's filled
//! at mount and cleared at unmount.

use runtime_core::{button, text, view, ButtonHandle, Ref, TextHandle, ViewHandle};

use crate::common::TestRuntime;

#[test]
fn button_ref_filled_at_mount() {
    let r: Ref<ButtonHandle> = Ref::new();
    let r_for_render = r;

    let rt = TestRuntime::new();
    let _owner = rt.render(button("click", || {}).bind(r_for_render).into());

    assert!(r.get().is_some(), "button ref filled at mount");
}

#[test]
fn view_ref_filled_at_mount() {
    let r: Ref<ViewHandle> = Ref::new();

    let rt = TestRuntime::new();
    let _owner = rt.render(view(Vec::new()).bind(r).into());

    assert!(r.get().is_some(), "view ref filled at mount");
}

#[test]
fn text_ref_filled_at_mount() {
    let r: Ref<TextHandle> = Ref::new();

    let rt = TestRuntime::new();
    let _owner = rt.render(text("hello").bind(r).into());

    assert!(r.get().is_some(), "text ref filled at mount");
}

#[test]
fn unfilled_ref_returns_none() {
    let r: Ref<ButtonHandle> = Ref::new();
    assert!(r.get().is_none(), "ref before mount is None");
}

#[test]
fn ref_filled_inside_nested_tree() {
    let r: Ref<TextHandle> = Ref::new();

    let rt = TestRuntime::new();
    let _owner = rt.render(
        view(vec![
            view(vec![text("nested").bind(r).into()]).into(),
        ])
        .into(),
    );

    assert!(r.get().is_some(), "ref deep in the tree is filled");
}
