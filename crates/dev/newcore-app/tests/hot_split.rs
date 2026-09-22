//! The `#[component]` hot-reload split, proved on a real authored tree.
//!
//! `src/app.rs` is the same app `e2e.rs` mounts — nested components with
//! static / reactive / defaulted / children props, a `#[method]`-bearing
//! component, a keyed `for` with row-local state, primitives, a
//! `stylesheet!`. Compiled with `runtime-core/hot-reload` it all goes
//! through the split. Two things have to hold, and neither is visible
//! from the macro's token output alone:
//!
//! 1. **The split renders identically.** Every component body now runs
//!    behind `dev_hot::call(fn_ptr, args)`. With no jump table installed
//!    that is a direct call, so the recorded op log must match the
//!    unsplit mount exactly. This suite re-asserts the load-bearing
//!    facts `e2e.rs` pins, on the split build.
//!
//! 2. **`__*_hot_impl` survives as a real symbol.** The patch pipeline
//!    pairs those symbols between the running binary and the patch
//!    dylib; a body the optimizer inlined away, or the linker dropped,
//!    has no address to rebind and the patch silently does nothing.
//!    Only the linked artifact can answer that, so the test reads its
//!    own symbol table.
//!
//! Run it explicitly — the feature is opt-in because `runtime-macros` is
//! a proc-macro crate and its gates unify across a whole cargo
//! invocation:
//!
//! ```text
//! cargo test -p newcore-app --features hot-reload --test hot_split
//! ```

#![cfg(feature = "hot-reload")]

use std::cell::RefCell;
use std::rc::Rc;

use newcore_app::app::build_app;
use runtime_scene::{realize, Registry};
use runtime_world::World;
use scene_parity::full::FullRecorder;
use scene_parity::{Mode, Recorder};

type Bridged = FullRecorder;

/// Mount the authored app against the mock host and return the op log.
fn mount_ops() -> Vec<String> {
    let rec = Recorder::default();
    let backend = Rc::new(RefCell::new(FullRecorder::new(rec.clone(), Mode::Spliced)));
    let mut registry: Registry<Bridged> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let registry = Rc::new(registry);
    let world = World::new();
    let (root, _handle) = world.enter(build_app);
    // Held to the end of the fn: dropping the `Realized` tears the tree
    // down and would empty the recorder's view of it.
    let _realized = world.enter(|| realize(&backend, &registry, root));
    rec.take_ops()
}

/// The split is dispatch-only: with no jump table installed,
/// `dev_hot::call` is a direct call and the mounted tree is the same
/// tree. Pins the same structural facts `e2e.rs` asserts on the unsplit
/// build — if the split reordered, dropped or duplicated a body, one of
/// these moves.
#[test]
fn split_build_mounts_the_same_tree() {
    let ops = mount_ops();
    let log = ops.join("\n");

    // Reactive prop rendered through a closure, inside a component that
    // is itself called from another component's `ui!` body.
    assert!(log.contains("\"Todos\""), "section title missing:\n{log}");
    // A defaulted `#[prop(default = ...)]` override — exactly one
    // component in the tree sets `highlighted`. A split that ran a body
    // twice (both halves emitted) would show two.
    assert_eq!(
        log.matches("text \"*\"").count(),
        1,
        "defaulted prop arrived twice or not at all:\n{log}"
    );
    // A `children:` prop forwarded through a container component.
    assert!(log.contains("text \"static footer\""), "children prop lost:\n{log}");
    // A memo read inside an f-string.
    assert!(log.contains("\"1 left\""), "memo f-string lost:\n{log}");
    // The keyed `for` rows, each a component with row-local state.
    assert!(log.contains("\"[ ] write tests\""), "open row lost:\n{log}");
    assert!(log.contains("\"[x] ship it\""), "done row lost:\n{log}");
    // The `#[method]`-bearing component still builds and binds.
    assert!(log.contains("\"tally: 5\""), "method component lost:\n{log}");
    // The untaken branch of a static `if` stays untaken.
    assert!(!log.contains("nothing to do"), "empty state wrongly mounted:\n{log}");
}

/// Every component in the authored tree has a `__<Name>_hot_impl`
/// symbol in this test binary.
///
/// This is the property the whole patch pipeline rests on
/// (`build-runtime-server`'s `hotpatch::jumptable::build` pairs symbols
/// whose name contains `_hot_impl`). It is NOT implied by the macro
/// emitting the split: `[profile.dev] opt-level = "z"` is aggressive,
/// and a function reached only as a fn pointer is exactly the shape an
/// optimizer likes to fold away. `#[inline(never)]` on the inner half
/// is what prevents it, and this test is what proves the attribute is
/// doing its job.
#[test]
fn every_component_keeps_a_hot_impl_symbol_in_the_linked_binary() {
    use object::{Object, ObjectSymbol};

    let exe = std::env::current_exe().expect("current_exe");
    let data = std::fs::read(&exe).expect("read own binary");
    let obj = object::File::parse(&*data).expect("parse own binary");

    let mut names: Vec<String> = Vec::new();
    for sym in obj.symbols() {
        if let Ok(n) = sym.name() {
            if n.contains("_hot_impl") {
                names.push(n.to_string());
            }
        }
    }
    assert!(
        !names.is_empty(),
        "no `_hot_impl` symbol at all in {} — the split did not reach the linked artifact",
        exe.display()
    );

    // Each component declared in src/app.rs, by name. A mangled symbol
    // embeds the ident, so a substring test is exact enough and survives
    // both legacy and v0 mangling.
    for component in [
        "StyledCard",
        "StaticIfMovedString",
        "Section",
        "TodoRow",
        "PrimsDemo",
        "MethodTally",
        "TodoApp",
    ] {
        let want = format!("__{component}_hot_impl");
        assert!(
            names.iter().any(|n| n.contains(&want)),
            "`{want}` is not in the binary's symbol table — that component cannot be \
             hot-patched. Symbols found: {names:#?}"
        );
    }
}
