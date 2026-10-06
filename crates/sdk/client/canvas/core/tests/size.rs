//! The canvas-size seam end to end, through the scene registry: a renderer
//! reports its laid-out size with `CanvasPrim::size_reporter()`, the write is
//! committed by a flush, and the painter — run by the renderer through
//! `CanvasPrim::paint()` inside its effect — re-runs and reads it as
//! `Scene::size()`.
//!
//! The renderer here is a minimal test handler on the host-mock substrate that
//! does exactly what every real renderer does: paint through `prim.paint()` in
//! a subtree-owned effect, and report the size it learns. (Without an
//! installed scheduler `after_ms` runs its callback synchronously on native,
//! so the deferred write is staged at `report` time and committed by the
//! test's `flush`.)

use std::cell::RefCell;
use std::rc::Rc;

use canvas_core::{Canvas, CanvasProps, Scene, SizeReporter};
use host_mock::Harness;
use runtime_scene::{MountCx, Realized};
use runtime_vocabulary::caps::DocumentOps;
use runtime_vocabulary::glue::IntoElement;

thread_local! {
    static REPORTER: RefCell<Option<SizeReporter>> = const { RefCell::new(None) };
}

fn harness() -> Harness {
    Harness::with_registry(|r| {
        r.register::<canvas_core::CanvasPrim, _>(|cx: &mut MountCx<'_, _>, prim, _children| {
            let node = cx.backend().borrow_mut().create_element("canvas");
            REPORTER.with(|s| *s.borrow_mut() = Some(prim.size_reporter()));
            let prim = prim.clone();
            runtime_world::effect(move || {
                let _scene = prim.paint();
            });
            node
        });
    })
}

fn reporter() -> SizeReporter {
    REPORTER.with(|s| s.borrow().clone()).expect("the test renderer stored a reporter")
}

/// A canvas whose painter records every size it is painted at.
fn recording_canvas(seen: Rc<RefCell<Vec<(f32, f32)>>>) -> runtime_scene::Element {
    Canvas(CanvasProps {
        draw: canvas_core::draw(move |s: &mut Scene| seen.borrow_mut().push(s.size())),
        ..Default::default()
    })
    .into_element()
}

#[test]
fn a_reported_size_reaches_the_painter_and_repaints() {
    let h = harness();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let el = h.world.enter(|| recording_canvas(seen.clone()));
    let _realized: Realized<u32> = h.mount(el);
    h.flush();
    assert_eq!(*seen.borrow(), vec![(0.0, 0.0)], "first paint runs before layout");

    reporter().report(120.0, 80.0);
    h.flush();
    assert_eq!(seen.borrow().last(), Some(&(120.0, 80.0)), "the painter sees the reported size");

    reporter().report(300.0, 80.0);
    h.flush();
    assert_eq!(seen.borrow().last(), Some(&(300.0, 80.0)), "a resize re-runs the painter");
}

/// Renderers report on every layout pass; an unchanged size must not repaint.
#[test]
fn reporting_the_same_size_does_not_repaint() {
    let h = harness();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let el = h.world.enter(|| recording_canvas(seen.clone()));
    let _realized: Realized<u32> = h.mount(el);
    h.flush();

    reporter().report(64.0, 64.0);
    h.flush();
    let paints = seen.borrow().len();
    reporter().report(64.0, 64.0);
    reporter().report(64.004, 63.996); // sub-epsilon layout noise
    h.flush();
    assert_eq!(seen.borrow().len(), paints, "no repaint for an unchanged size");
}

/// A canvas built outside a world (a test, a wire snapshot) has no size
/// signal: reporting is a no-op and the size reads as zero.
#[test]
fn a_canvas_outside_a_world_reports_nothing_and_paints_at_zero() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let props = CanvasProps {
        draw: canvas_core::draw({
            let seen = seen.clone();
            move |s: &mut Scene| seen.borrow_mut().push(s.size())
        }),
        ..Default::default()
    };
    let scene = canvas_core::paint_scene(&props);
    assert_eq!(scene.size(), (0.0, 0.0));
    assert_eq!(*seen.borrow(), vec![(0.0, 0.0)]);
}

/// The reporter can outlive the canvas (a late resize callback during
/// teardown). Its deferred write must be skipped once the canvas's scope is
/// gone — writing a freed signal panics with `stale-signal-handle`, and on a
/// real backend that panic is raised in a non-unwinding platform callback,
/// which aborts.
///
/// The canvas is built inside a reactive hole so it has a scope of its own
/// that unmount really disposes while the world lives on — the shape of a
/// route change. Built at the world root instead, its size signal is never
/// freed and the test would pass with the guard removed.
#[test]
fn regression_a_report_after_unmount_is_ignored() {
    use runtime_vocabulary::builders::view;
    use runtime_world::signal;

    let h = harness();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let shown_slot = Rc::new(RefCell::new(None));
    let el = h.world.enter(|| {
        let shown = signal(true);
        *shown_slot.borrow_mut() = Some(shown);
        let seen = seen.clone();
        view()
            .child(move || {
                if shown.get() {
                    recording_canvas(seen.clone())
                } else {
                    view().build()
                }
            })
            .build()
    });
    let _realized: Realized<u32> = h.mount(el);
    h.flush();
    let shown = shown_slot.borrow().expect("hole built");

    // Sanity: while mounted the reporter really writes, so the silence
    // below means "guarded", not "never wired".
    let late = reporter();
    late.report(40.0, 40.0);
    h.flush();
    assert_eq!(seen.borrow().last(), Some(&(40.0, 40.0)), "reports land while mounted");

    // Unmount only the canvas's scope; the world survives.
    shown.set(false);
    h.flush();
    let paints = seen.borrow().len();

    late.report(50.0, 50.0); // must not write the freed size signal
    h.flush();
    assert_eq!(seen.borrow().len(), paints, "an unmounted canvas never repaints");
}
