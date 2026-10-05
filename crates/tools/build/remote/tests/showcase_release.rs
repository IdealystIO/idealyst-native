//! A real release build: the showcase declares its own library as a bundle
//! (`[package.metadata.idealyst.remote]`), and `build` compiles it to wasm,
//! checks it, stamps it and signs it. The result must be a bundle the loader
//! accepts under a policy that requires the signature — `remote-showcase`'s
//! own tests load the same source; this pins that the CLI's release path
//! produces it.
//!
//! Slow on a cold cache (a full wasm32 release build of the framework into
//! `target/idealyst/remote-build`), fast after.

use std::path::PathBuf;

use build_remote::{build, Options};
use remote_bundle::SigningKey;

#[test]
fn the_showcase_builds_as_a_signed_release_bundle() {
    let app = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../streaming/showcase/app");
    let out = tempfile::tempdir().unwrap();
    let key = SigningKey::generate().unwrap();
    let built = build(&app, &Options { out_dir: Some(out.path().to_path_buf()), sign: Some(key), only: vec![] }).expect("builds");
    assert_eq!(built.len(), 1);
    let b = &built[0];
    assert_eq!(b.manifest.name, "showcase");
    assert_eq!(b.manifest.package, "remote-showcase");
    assert_eq!(b.wasm, out.path().join("showcase.wasm"));
    let wasm = std::fs::read(&b.wasm).unwrap();
    let meta = remote_bundle::metadata(&wasm).unwrap().expect("metadata");
    assert_eq!((meta.name.as_str(), meta.codec), ("showcase", 2));
    assert_eq!(b.manifest.sha256, remote_bundle::content_hash(&wasm));
    let json: build_remote::Manifest = serde_json::from_slice(&std::fs::read(out.path().join("showcase.json")).unwrap()).unwrap();
    assert_eq!(&json, &b.manifest);
    assert!(remote_bundle::signature(&wasm).unwrap().is_some());
    // Imports only what the loader defines (the build checked; this is the
    // file it wrote).
    assert!(build_remote::foreign_imports(&wasm).unwrap().is_empty());
}
