//! The interning budget (`remote::INTERN_BUDGET_BYTES`): its own test
//! binary, because the budget is process-global and this lowers it.
//!
//! ```sh
//! cargo test -p runtime-vocabulary --features remote-loopback --test remote_intern_budget
//! ```

#![cfg(feature = "remote-loopback")]

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_shared::assets::{kinds, Asset, AssetId, AssetSource};
use runtime_vocabulary::builders::{image, view};
use runtime_vocabulary::remote::host::{decode, Link};
use runtime_vocabulary::remote::{bundle, to_bytes, Cb};

#[derive(Default)]
struct Recording {
    fails: RefCell<Vec<String>>,
}

impl Link for Recording {
    fn call(&self, cb: Cb, args: &[u8]) -> Option<Vec<u8>> {
        Some(bundle::invoke(cb, args))
    }
    fn release(&self, cb: Cb) {
        bundle::release(cb)
    }
    fn fail(&self, msg: String) {
        self.fails.borrow_mut().push(msg);
    }
}

fn embedded(id: u64, bytes: &'static [u8]) -> Asset<kinds::Image> {
    Asset::new(AssetId(id), AssetSource::Embedded { bytes, extension: "png" })
}

/// Regression: the app leaked every DISTINCT name, icon and asset blob a
/// bundle sent (the prims hold them `&'static`) with no bound, so a bundle
/// sending new bytes on every render grew the app without limit. Past the
/// budget the bundle is stopped; within it, distinct values intern and
/// repeats cost nothing.
#[test]
fn regression_a_bundle_past_the_interning_budget_is_stopped() {
    runtime_vocabulary::remote::__set_intern_budget(4096);
    let h = Harness::new();
    let mount = |asset: Asset<kinds::Image>| {
        let link = Rc::new(Recording::default());
        let tree = h.world.enter(|| view().child(image().asset(asset)).build());
        let realized = h.mount(decode(link.clone(), &to_bytes(&bundle::tree(tree))).expect("decodes"));
        h.flush();
        let fails = link.fails.borrow().clone();
        drop(realized);
        fails
    };
    static SMALL: [u8; 1024] = [1; 1024];
    assert!(mount(embedded(1, &SMALL)).is_empty(), "within the budget");
    assert!(mount(embedded(2, &SMALL)).is_empty(), "the same bytes again cost nothing");
    static BIG: [u8; 8192] = [2; 8192];
    let fails = mount(embedded(3, &BIG));
    assert!(fails.iter().any(|m| m.contains("interning budget")), "{fails:?}");
}
