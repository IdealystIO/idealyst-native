//! What the showcase's bundle requires of an app, and what the showcase app
//! provides, computed for real on each side: the bundle's list read from
//! its wasm the way `idealyst build --remote` reads it, the app's from what
//! it registered. A bundle and the app built from the same source must
//! fit; a bundle needing more must be refused at load, naming what.

#![cfg(not(feature = "inline"))]

use remote_bundle::Problem;
use remote_host::remote::{install_with, provides};
use remote_showcase::{host_fns, BUILT_IN};

const BUTTON: &str = "idea_ui::components::button::Button";
const CODEC: u32 = 2;

fn requires() -> remote_bundle::Requires {
    build_remote::requires::requires(BUILT_IN).expect("the bundle's requirements")
}

#[test]
fn the_bundle_fits_the_app_built_from_the_same_source() {
    let req = requires();
    let app = provides(&host_fns());
    let errors: Vec<Problem> = remote_bundle::check(&req, CODEC, &app).into_iter().filter(Problem::is_error).collect();
    assert!(errors.is_empty(), "{errors:#?}");

    // Only the props the bundle's call sites set, against all of the app's.
    let used: Vec<&str> = req.components[BUTTON].keys().map(String::as_str).collect();
    assert_eq!(used, ["label", "on_click", "tone"]);
    assert!(app.components[BUTTON].len() > used.len(), "the app's Button has more props than the bundle sets");
    // Only the app components the bundle reaches: idea-ui has dozens.
    assert!(req.components.len() < app.components.len());
    assert!(!req.components.contains_key("idea_ui::components::modal::Modal"));

    // The two sides spell a derived type's structure the same way: the app
    // computes it natively, the bundle in the interpreter.
    let invoice = req.host_fns.keys().find(|k| k.starts_with("remote_showcase::tools::invoice_total#")).expect("invoice_total");
    assert_eq!(
        req.host_fns[invoice],
        "fn(Invoice{lines:list<InvoiceLine{item:str,cents:u64,qty:u32}>,tax_percent:u32})->result<u64,InvoiceError[Empty,OverLimit{cents:u64}]>"
    );
    assert_eq!(app.host_fns[invoice], req.host_fns[invoice]);
    assert_eq!(req.remote["remote_showcase::ShopNavigator"], app.remote["remote_showcase::ShopNavigator"]);
}

/// A bundle built against a newer app (a prop, a host function this one
/// lacks, a struct that changed shape) is refused before it runs, with
/// every problem named — rather than failing when the screen mounts, or
/// misreading a value.
#[test]
fn the_loader_refuses_a_bundle_that_needs_more_than_the_app_has() {
    let fits = remote_bundle::with_requires(BUILT_IN, &requires()).unwrap();
    assert!(install_with(&fits, host_fns()).is_ok(), "the bundle's own requirements load");

    let mut newer = requires();
    newer.components.get_mut(BUTTON).unwrap().insert("glow".into(), "bool".into());
    let invoice = newer.host_fns.keys().find(|k| k.contains("invoice_total#")).unwrap().clone();
    newer.host_fns.insert(invoice, "fn(Invoice{lines:list<InvoiceLine{item:str,cents:u64,qty:u32}>,tax_percent:u32,note:str})->u64".into());
    let wasm = remote_bundle::with_requires(BUILT_IN, &newer).unwrap();
    let err = install_with(&wasm, host_fns()).err().expect("refused");
    assert!(err.contains("`idea_ui::components::button::Button` has no prop `glow` in the app"), "{err}");
    assert!(err.contains("host function `remote_showcase::tools::invoice_total`: the bundle calls"), "{err}");
}

/// The release build strips the shape functions after reading them
/// (`build_remote::requires::strip_shapes`, a dead-code pass over the
/// whole module): the result must still be the same working bundle — it
/// loads under its requirements, and its screens run, host functions and
/// app components included.
#[test]
fn the_stripped_release_bundle_runs() {
    use host_mock::{pump, Harness};
    use runtime_core::ui;

    let spec = build_remote::BundleSpec { name: "showcase".into(), package: "remote-showcase".into() };
    let (release, manifest) = build_remote::finish(BUILT_IN, &spec, "0.1.0", None).expect("a release");
    assert!(release.len() < BUILT_IN.len(), "stripped: {} -> {} bytes", BUILT_IN.len(), release.len());
    assert_eq!(remote_bundle::requires(&release).unwrap().as_ref(), Some(&manifest.requires));

    pump::install_executor();
    pump::install_scheduler();
    remote_showcase::install_from(&release).expect("the release loads");
    let h = Harness::new();
    let tree = h.world.enter(|| ui! { remote_showcase::App() });
    let _realized = h.mount(tree);
    h.flush();
    let screen = h.live_roots().iter().map(|n| h.live_tree(*n)).collect::<Vec<_>>().join("\n");
    assert!(screen.contains("Feed — rendered by the bundle"), "{screen}");
    assert!(screen.contains(&format!("running on {}", std::env::consts::OS)), "a host function answered:\n{screen}");
    assert!(runtime_vocabulary::remote::host::live_trees() > 0);
}
