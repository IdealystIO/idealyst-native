//! Removing the backend's own tagged sublayers from a `CALayer`.
//!
//! `-[CALayer sublayers]` returns the layer's *live* `CALayerArray`, not a
//! snapshot: `removeFromSuperlayer` shrinks it in place. A walk that caches
//! `count` and removes while indexing runs past the new end, and
//! `-[CALayerArray objectAtIndex:]` aborts the process. Every re-apply of a
//! tagged layer (a theme re-render re-installing a gradient) hits that walk,
//! so removal is two-phase: collect every match in a read-only pass, then
//! detach them.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2_foundation::NSString;

/// Detach every direct sublayer of `layer` whose `name` equals `name`.
/// Returns how many were removed. Safe on a layer with no sublayers.
pub fn remove_named_sublayers(layer: &NSObject, name: &str) -> usize {
    let doomed: Vec<Retained<NSObject>> = unsafe {
        let sublayers: *mut NSObject = msg_send![layer, sublayers];
        if sublayers.is_null() {
            return 0;
        }
        let count: usize = msg_send![sublayers, count];
        let mut doomed = Vec::new();
        for i in 0..count {
            let sub: *mut NSObject = msg_send![sublayers, objectAtIndex: i];
            if sub.is_null() {
                continue;
            }
            let sub_name: *mut NSString = msg_send![sub, name];
            if sub_name.is_null() || (*sub_name).to_string() != name {
                continue;
            }
            // Retained: the array is the layer's only owner, and detaching an
            // earlier match must not free a later one we still hold.
            if let Some(sub) = Retained::retain(sub) {
                doomed.push(sub);
            }
        }
        doomed
    };
    for sub in &doomed {
        let _: () = unsafe { msg_send![&**sub, removeFromSuperlayer] };
    }
    doomed.len()
}
