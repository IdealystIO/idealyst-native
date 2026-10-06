//! Signed bundles over the real loader: a release bundle (metadata +
//! signature appended as custom sections) still loads and mounts, and an app
//! that requires a signature refuses everything else — at install and at
//! reload, before the module is parsed.

use host_mock::Harness;
use remote_bundle::{sign, with_metadata, Metadata, SigningKey};
use remote_host::remote::{install_with_options, Options, Trust};
use runtime_core::ui;
use runtime_world::signal;
use spike_remoteattr::Greeting;
use stream_spike::REMOTE_ATTR_WASM;

fn camera() -> Vec<runtime_vocabulary::remote::HostFnDef> {
    vec![
        spike_camera::battery_level::export(),
        spike_camera::take_photo::export(),
        spike_remoteattr::mount_nested::export(),
        spike_remoteattr::mood_says::export(),
    ]
}

fn release(key: &SigningKey) -> Vec<u8> {
    let meta = Metadata { name: "greeting".into(), package: "spike-remoteattr".into(), version: "0.1.0".into(), codec: 2 };
    sign(&with_metadata(REMOTE_ATTR_WASM, &meta).unwrap(), key).unwrap()
}

fn strict(key: &SigningKey) -> Options {
    Options { host_fns: camera(), trust: Trust::default().key(key.public()).require_signature() }
}

/// The custom sections a release adds don't change what the module does:
/// it loads under a policy that requires them, and its component mounts.
#[test]
fn a_signed_release_bundle_loads_and_mounts() {
    let key = SigningKey::generate().unwrap();
    let _remote = install_with_options(&release(&key), strict(&key)).expect("a signed bundle loads");
    let h = Harness::new();
    let (count, likes) = h.world.enter(|| (signal(1i64), signal(0i64)));
    let realized = h.mount(h.world.enter(|| ui! { Greeting(name = "ada".to_string(), count = count.read_only(), likes = likes) }));
    h.flush();
    let text = realized.collect_nodes().iter().map(|n| h.live_tree(*n)).collect::<Vec<_>>().join("\n");
    assert!(text.contains("hello ada"), "{text}");
}

#[test]
fn a_required_signature_refuses_unsigned_foreign_and_tampered_bundles() {
    let (ours, theirs) = (SigningKey::generate().unwrap(), SigningKey::generate().unwrap());
    let err = |wasm: &[u8]| install_with_options(wasm, strict(&ours)).err().expect("refused");

    assert!(err(REMOTE_ATTR_WASM).contains("not signed"), "{}", err(REMOTE_ATTR_WASM));
    let foreign = release(&theirs);
    assert!(err(&foreign).contains(&theirs.public().id().to_string()), "{}", err(&foreign));
    // One byte of the module changed after signing (inside the code, past
    // the header): refused as tampered, not as a wasm error — the check
    // runs before the module is parsed.
    let mut tampered = release(&ours);
    let at = tampered.len() / 2;
    tampered[at] ^= 0x01;
    assert!(err(&tampered).contains("changed after signing"), "{}", err(&tampered));
}

/// A reload is held to the install's policy, and a refused one leaves the
/// current bundle in place.
#[test]
fn a_reload_is_held_to_the_same_trust() {
    let key = SigningKey::generate().unwrap();
    let remote = install_with_options(&release(&key), strict(&key)).expect("loads");
    let before = remote.__generation();
    let err = remote.reload(REMOTE_ATTR_WASM).err().expect("an unsigned reload is refused");
    assert!(err.contains("not signed"), "{err}");
    assert_eq!(remote.__generation(), before, "nothing remounted");
    remote.reload(&release(&key)).expect("a signed reload loads");
}

/// The default trusts nothing and requires nothing: an unsigned
/// development bundle loads, as before signing existed.
#[test]
fn by_default_any_bundle_loads() {
    install_with_options(REMOTE_ATTR_WASM, Options { host_fns: camera(), ..Options::default() }).expect("unsigned loads");
}
