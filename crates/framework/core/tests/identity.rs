//! Identity suite — `Identity` tree positions, `use_id`, `use_id_keyed`.
//!
//! Identity is the framework's stable tree-position abstraction.
//! `current_identity()` returns the identity the walker is emitting
//! under; `with_current_identity(id, f)` runs `f` with `id` active.
//! `use_id()` derives a stable string from the current identity.
//!
//! These tests cover the public API of identity directly; the
//! identity-stability-across-renders behavior (when a list item's
//! position is preserved by a stable key vs. when it moves) is
//! covered by the walker suite, since it depends on `for`/`Repeat`
//! key handling.

#[path = "common/mod.rs"]
mod common;

use framework_core::{current_identity, hash_key, use_id, use_id_keyed, with_current_identity, Identity};

/// `current_identity()` outside any `with_current_identity` returns
/// `Identity::UNIDENTIFIED`.
#[test]
fn current_identity_default() {
    let id = current_identity();
    assert_eq!(id, Identity::UNIDENTIFIED);
}

/// `with_current_identity(id, f)` makes `id` visible to `current_identity()`
/// inside `f`, then restores on return.
#[test]
fn with_current_identity_replaces_then_restores() {
    let outer = current_identity();
    let custom = Identity::node(Identity::ROOT_SCOPE, 7, None, None);

    let inside = with_current_identity(custom, current_identity);
    assert_eq!(inside, custom);

    let after = current_identity();
    assert_eq!(after, outer, "outer identity restored after scope ends");
}

/// `with_current_identity` nests correctly — inner scope's identity
/// is active inside, outer's identity is restored when inner returns.
#[test]
fn nested_with_current_identity() {
    let outer = Identity::node(Identity::ROOT_SCOPE, 1, None, None);
    let inner = Identity::node(Identity::ROOT_SCOPE, 2, None, None);

    let (in_outer, in_inner, in_outer_again) = with_current_identity(outer, || {
        let o = current_identity();
        let i = with_current_identity(inner, current_identity);
        let o2 = current_identity();
        (o, i, o2)
    });

    assert_eq!(in_outer, outer);
    assert_eq!(in_inner, inner);
    assert_eq!(in_outer_again, outer);
}

/// `use_id()` outside any `with_current_identity` derives from
/// `Identity::UNIDENTIFIED` — stable across calls in that context.
#[test]
fn use_id_outside_scope_is_stable() {
    let a = use_id();
    let b = use_id();
    assert_eq!(a, b, "same context → same id");
}

/// `use_id()` inside a `with_current_identity` differs per identity.
#[test]
fn use_id_changes_with_identity() {
    let id_a = Identity::node(Identity::ROOT_SCOPE, 1, None, None);
    let id_b = Identity::node(Identity::ROOT_SCOPE, 2, None, None);

    let a = with_current_identity(id_a, use_id);
    let b = with_current_identity(id_b, use_id);
    assert_ne!(a, b, "different identities → different ids");
}

/// `use_id()` is deterministic — same identity → same id, repeatedly.
#[test]
fn use_id_is_deterministic() {
    let id = Identity::node(Identity::ROOT_SCOPE, 42, Some(3), None);
    let first = with_current_identity(id, use_id);
    let second = with_current_identity(id, use_id);
    let third = with_current_identity(id, use_id);
    assert_eq!(first, second);
    assert_eq!(second, third);
}

/// `use_id_keyed(key)` differs for different keys under the same
/// identity.
#[test]
fn use_id_keyed_differs_per_key() {
    let id = Identity::node(Identity::ROOT_SCOPE, 1, None, None);
    let (a, b, c) = with_current_identity(id, || {
        (use_id_keyed("foo"), use_id_keyed("bar"), use_id_keyed("foo"))
    });

    assert_ne!(a, b, "different keys → different ids");
    assert_eq!(a, c, "same key → same id");
}

/// `use_id` format: "ui-" prefix + 16 hex digits = 19 chars total.
#[test]
fn use_id_format_is_correct() {
    let id = use_id();
    assert!(id.starts_with("ui-"), "starts with 'ui-': got '{id}'");
    assert_eq!(id.len(), 19, "length 19 (3 prefix + 16 hex): got {}", id.len());
    let hex_part = &id["ui-".len()..];
    assert!(
        hex_part.chars().all(|c| c.is_ascii_hexdigit() && (c.is_ascii_digit() || c.is_ascii_lowercase())),
        "hex portion is lowercase hex: '{hex_part}'"
    );
}

/// `hash_key` is a deterministic projection — same input → same hash.
#[test]
fn hash_key_is_deterministic() {
    assert_eq!(hash_key("foo"), hash_key("foo"));
    assert_eq!(hash_key(&42u32), hash_key(&42u32));
    assert_ne!(hash_key("foo"), hash_key("bar"));
}

/// `Identity::UNIDENTIFIED` is distinct from explicit identities.
#[test]
fn unidentified_is_distinct() {
    let id = Identity::node(Identity::ROOT_SCOPE, 0, None, None);
    assert_ne!(id, Identity::UNIDENTIFIED);
}

/// `Identity::scope(...)` and `Identity::node(...)` produce different
/// identities for the same arguments — they're different identity
/// kinds.
#[test]
fn scope_and_node_identities_differ() {
    let scope = Identity::scope(Identity::ROOT_SCOPE, 2, None, None);
    let node = Identity::node(Identity::ROOT_SCOPE, 2, None, None);
    assert_ne!(scope, node, "scope and node identities are different shapes");
}

/// `use_id_keyed` with the same key under different identities still
/// produces different ids (identity is mixed into the hash).
#[test]
fn use_id_keyed_changes_with_identity_too() {
    let id_a = Identity::node(Identity::ROOT_SCOPE, 1, None, None);
    let id_b = Identity::node(Identity::ROOT_SCOPE, 2, None, None);

    let a = with_current_identity(id_a, || use_id_keyed("same-key"));
    let b = with_current_identity(id_b, || use_id_keyed("same-key"));
    assert_ne!(a, b, "same key under different identities → different ids");
}
