//! Live-CALayer test for `sublayers::remove_named_sublayers`.
//!
//! Real `CALayer`s, so the property that crashed is the real one:
//! `-[CALayer sublayers]` is a live array that shrinks as sublayers detach.
//! Reaching the end of `run` without an `objectAtIndex:` abort is part of the
//! assertion.
//!
//! Run: `cargo test -p backend-apple-core --test sublayers_calayer`

use backend_apple_core::sublayers::remove_named_sublayers;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::NSString;

#[link(name = "QuartzCore", kind = "framework")]
extern "C" {}

const GRADIENT: &str = "idealyst_gradient";

fn layer(name: Option<&str>) -> Retained<NSObject> {
    let l: Retained<NSObject> = unsafe { msg_send_id![class!(CALayer), layer] };
    if let Some(n) = name {
        let n = NSString::from_str(n);
        let _: () = unsafe { msg_send![&l, setName: &*n] };
    }
    l
}

fn add(parent: &NSObject, child: &NSObject) {
    let _: () = unsafe { msg_send![parent, addSublayer: child] };
}

fn names(parent: &NSObject) -> Vec<Option<String>> {
    unsafe {
        let subs: *mut NSObject = msg_send![parent, sublayers];
        if subs.is_null() {
            return Vec::new();
        }
        let count: usize = msg_send![subs, count];
        (0..count)
            .map(|i| {
                let sub: *mut NSObject = msg_send![subs, objectAtIndex: i];
                let n: *mut NSString = msg_send![sub, name];
                (!n.is_null()).then(|| (*n).to_string())
            })
            .collect()
    }
}

/// Regression: re-applying a gradient on iOS aborted with
/// `-[CALayerArray objectAtIndex:]` out of bounds (idea-ui-docs crashed at
/// launch on iPad when the theme driver re-applied a codeblock sheet). The
/// removal walk detached the old `idealyst_gradient` sublayer while indexing
/// the live `sublayers` array. A gradient FIRST with siblings after it is the
/// shape the backend builds (`insertSublayer:atIndex:0`).
fn regression_ios_gradient_reapply_out_of_bounds() {
    let parent = layer(None);
    add(&parent, &layer(Some(GRADIENT)));
    add(&parent, &layer(Some("author_content")));
    add(&parent, &layer(None));

    assert_eq!(remove_named_sublayers(&parent, GRADIENT), 1);
    assert_eq!(names(&parent), vec![Some("author_content".to_string()), None]);
}

/// Every match goes, including adjacent ones the shrinking array would shift
/// under a one-phase walk; unmatched layers keep their order.
fn removes_every_match_and_keeps_the_rest() {
    let parent = layer(None);
    add(&parent, &layer(Some(GRADIENT)));
    add(&parent, &layer(Some(GRADIENT)));
    add(&parent, &layer(Some("a")));
    add(&parent, &layer(Some(GRADIENT)));
    add(&parent, &layer(Some("b")));

    assert_eq!(remove_named_sublayers(&parent, GRADIENT), 3);
    assert_eq!(names(&parent), vec![Some("a".to_string()), Some("b".to_string())]);
}

/// A layer with no sublayers (`sublayers` is nil) is a no-op.
fn no_sublayers_is_a_noop() {
    assert_eq!(remove_named_sublayers(&layer(None), GRADIENT), 0);
}

pub fn run() {
    regression_ios_gradient_reapply_out_of_bounds();
    removes_every_match_and_keeps_the_rest();
    no_sublayers_is_a_noop();
    println!("sublayers_calayer: 3 passed");
}
