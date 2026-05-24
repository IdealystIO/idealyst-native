//! `Primitive::Video` build path. Reactive `src` re-fires an
//! `update_video_src` call on every signal change inside the closure.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::reactive::Effect;
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    src: Box<dyn Fn() -> String>,
    autoplay: bool,
    controls: bool,
    loop_playback: bool,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let initial = src();
    let n = time_backend_create(pkind!(Video), || {
        backend.borrow_mut().create_video(&initial, autoplay, controls, loop_playback, &a11y)
    });
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    {
        let backend = backend.clone();
        let node = n.clone();
        let _e = Effect::new(move || {
            let s = src();
            backend.borrow_mut().update_video_src(&node, &s);
        });
    }
    if let Some(RefFill::Video(fill)) = ref_fill {
        let handle = backend.borrow().make_video_handle(&n);
        fill(handle);
    }
    n
}
