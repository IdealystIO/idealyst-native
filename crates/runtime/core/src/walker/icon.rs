//! `Element::Icon` build path. Reactive `color` and `stroke`
//! progress install per-axis Effects; an optional `draw_in`
//! mount animation snaps to `from` and schedules an
//! `animate_icon_stroke` on the next microtask.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::primitives;
use crate::reactive::Effect;
use crate::scheduling::schedule_microtask;
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    data: primitives::icon::IconData,
    data_fn: Option<Box<dyn Fn() -> primitives::icon::IconData>>,
    color: Option<Box<dyn Fn() -> crate::style::Color>>,
    stroke: Option<Box<dyn Fn() -> f32>>,
    draw_in: Option<primitives::icon::StrokeAnimation>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let initial_color = color.as_ref().map(|f| f());
    let n = time_backend_create(pkind!(Icon), || {
        backend.borrow_mut().create_icon(&data, initial_color.as_ref(), &a11y)
    });
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    // Reactive icon geometry: swap the rendered glyph in place when the
    // source changes (no node rebuild).
    if let Some(f) = data_fn {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let d = f();
            backend.borrow_mut().update_icon_data(&node, &d);
        });
    }
    // Reactive color effect.
    if let Some(f) = color {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let c = f();
            backend.borrow_mut().update_icon_color(&node, &c);
        });
    }
    // Reactive stroke progress effect.
    if let Some(f) = stroke {
        let initial = f();
        backend.borrow_mut().update_icon_stroke(&n, initial);
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let progress = f();
            backend.borrow_mut().update_icon_stroke(&node, progress);
        });
    }
    // Mount draw-in animation: snap to `from`, then animate
    // to `to` on the next microtask.
    if let Some(anim) = draw_in {
        backend.borrow_mut().update_icon_stroke(&n, anim.from);
        let backend = backend.clone();
        let node = n.clone();
        let autoreverses = anim.autoreverses;
        schedule_microtask(move || {
            backend.borrow_mut().animate_icon_stroke(
                &node,
                anim.from,
                anim.to,
                anim.duration_ms,
                anim.easing,
                anim.infinite,
                autoreverses,
            );
        });
    }
    if let Some(RefFill::Icon(fill)) = ref_fill {
        let handle = backend.borrow().make_icon_handle(&n);
        fill(handle);
    }
    n
}
