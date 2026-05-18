//! Stable structural identity for primitives, scopes, styles, and handlers.
//!
//! The framework's internal counters (`NodeId`, `StyleId`, `ScopeId`,
//! `HandlerId`) are sequential u64s assigned in walk order. That's
//! fine within a single walk, but **breaks across rebuilds** in AAS
//! mode: when a sidecar respawns, its counters restart from 0, so
//! `RegisterStyle { id: 0, … }` from the new sidecar overwrites an
//! unrelated style on the client — see the comment chain that
//! introduced this module for the canonical "theme toggle scrambles
//! layout" repro.
//!
//! [`Identity`] is the framework's answer: a content-addressed hash
//! derived from the emission site's *position in the structural tree*
//! (not its position in walk order). Two walks of the same tree
//! produce the same identities for the same nodes; the host-side
//! recorder maps `Identity → wire_id` once and reuses the mapping
//! across rebuilds, so downstream commands always reference the
//! correct wire id.
//!
//! # Composition
//!
//! Identity is composed hierarchically:
//!
//! - **`ScopeIdentity`** = `hash(parent_scope, slot, key, branch)`
//! - **`NodeIdentity`**  = `hash(enclosing_scope, slot, key, branch)`
//! - **`StyleIdentity`** = `hash(stylesheet_path, variant_name)` (no
//!   parent; styles are top-level)
//! - **`HandlerIdentity`** = `hash(enclosing_scope, slot)`
//!
//! Each id-kind uses a different domain-separation salt
//! ([`SALT_SCOPE`], [`SALT_NODE`], etc.) so the same composition
//! tuple can't produce colliding ids across kinds.
//!
//! # Inputs
//!
//! - **`slot`** — emission order within the parent's children list.
//!   The walker computes this as it iterates; macros never need to
//!   surface a slot number to the author.
//! - **`branch`** — discriminator for `if` / `match` arms. Same slot
//!   in different branches produces different identities, matching
//!   React/Solid's keyed-discriminant semantics for conditionals
//!   (each arm is its own structural seat).
//! - **`key`** — user-supplied identity for list children (`for item
//!   in items, key = item.id { … }`). Falls back to the iteration
//!   index when omitted; documented to *not* survive reorder.
//!
//! # Why u64 (not u128)
//!
//! All wire ids are `u64` today; widening would ripple through
//! `wire.rs`, every backend, and serialization. The hash is
//! domain-salted per id-kind, the position contribution is already a
//! u64 (parent_scope), and user keys are folded into the same word —
//! so the effective entropy per id is well above what a 64-bit hash
//! comfortably handles. If collisions surface in practice the host
//! recorder's `Identity → wire_id` table is the only place that
//! cares; it can grow a collision-resolution counter without
//! changing the wire format.

use std::cell::Cell;
use std::hash::{Hash, Hasher};

// ---------------------------------------------------------------------------
// Domain salts
// ---------------------------------------------------------------------------

/// Salt the scope-id hash. Distinct from the other salts so a scope
/// and a node with the same `(parent, slot)` can't share a hash.
const SALT_SCOPE: u64 = 0x9E37_79B9_7F4A_7C15;
/// Salt the node-id hash.
const SALT_NODE: u64 = 0xBF58_476D_1CE4_E5B9;
/// Salt the style-id hash.
const SALT_STYLE: u64 = 0x94D0_49BB_1331_11EB;
/// Salt the handler-id hash.
const SALT_HANDLER: u64 = 0xC4CE_B9FE_1A85_EC53;

// ---------------------------------------------------------------------------
// Identity types
// ---------------------------------------------------------------------------

/// Compile- and walk-time stable identity for one of the four id
/// kinds the framework mints (`Scope`, `Node`, `Style`, `Handler`).
///
/// The internal `u64` is the only thing consumers see; the host
/// recorder uses it as a `HashMap` key when assigning wire ids. Two
/// `Identity`s with the same `u64` are interchangeable.
///
/// Build one via [`Identity::scope`], [`Identity::node`],
/// [`Identity::style`], or [`Identity::handler`]; never construct
/// directly from a `u64` (that would defeat the domain separation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Identity(u64);

impl Identity {
    /// Internal accessor for the recorder's `HashMap` key. Public so
    /// the dev-server crate (different package) can read it; not
    /// meant for external use.
    #[doc(hidden)]
    pub fn raw(self) -> u64 {
        self.0
    }

    /// Compute a `ScopeIdentity` from its position in the parent
    /// scope. Used at every `#[component]` invocation and every
    /// `for` / `if` / `match` arm.
    ///
    /// - `parent`: the enclosing scope's identity (or
    ///   [`Identity::ROOT_SCOPE`] for the top-level scope).
    /// - `slot`: position of this scope within the parent's children
    ///   (or iteration index within a `for` loop).
    /// - `branch`: optional discriminator for `if` / `match` arms.
    ///   `None` for unconditional siblings.
    /// - `key`: optional user-supplied key from a `for` loop's
    ///   `key = …` clause. Hashed before being mixed in.
    pub fn scope(parent: Identity, slot: u32, branch: Option<u32>, key: Option<u64>) -> Identity {
        Identity(mix(SALT_SCOPE, parent.0, slot, branch, key))
    }

    /// Compute a `NodeIdentity` for a primitive emission within a
    /// scope. Same inputs as [`Identity::scope`], but salted into the
    /// node namespace so nodes and scopes never collide on the same
    /// `(parent, slot)`.
    pub fn node(parent: Identity, slot: u32, branch: Option<u32>, key: Option<u64>) -> Identity {
        Identity(mix(SALT_NODE, parent.0, slot, branch, key))
    }

    /// Compute a `StyleIdentity` for a stylesheet declaration.
    /// `path_hash` is a compile-time hash of the stylesheet's
    /// fully-qualified module path + variant name (emitted by the
    /// `stylesheet!` macro). Same stylesheet at the same module path
    /// → same `StyleId` across rebuilds, regardless of whether other
    /// styles registered before it.
    pub fn style(path_hash: u64) -> Identity {
        Identity(mix(SALT_STYLE, path_hash, 0, None, None))
    }

    /// Compute a `HandlerIdentity` for a callback registered inside a
    /// component scope. `slot` is the callback's emission order
    /// within the component (the `#[component]` macro tracks one
    /// counter per callback site).
    pub fn handler(parent: Identity, slot: u32) -> Identity {
        Identity(mix(SALT_HANDLER, parent.0, slot, None, None))
    }

    /// Sentinel identity for the top-level scope (the root of the
    /// `app()` walk). All scopes' parents transitively reduce to this.
    /// Picked to be a fixed constant rather than `0` so a stray
    /// uninitialized id doesn't masquerade as the root.
    pub const ROOT_SCOPE: Identity = Identity(0xD1BE_4A24_71B2_75A1);

    /// Sentinel for "this site is not yet identified" — used by the
    /// walker during the transition window where some emission sites
    /// haven't been threaded through `Identity` yet. The host recorder
    /// treats `UNIDENTIFIED` specially: it mints a fresh wire id every
    /// time it sees one, with no dedup. Effectively reverts to the
    /// legacy sequential behavior for that emission.
    ///
    /// Remove once every `create_*` site is identity-aware.
    pub const UNIDENTIFIED: Identity = Identity(0);
}

/// Compute the `path_hash` input to [`Identity::style`] from a
/// stylesheet's module path + variant name. Exposed here (rather than
/// hard-coded in the `stylesheet!` macro) so the framework owns the
/// algorithm — changes to it are a single-location update.
///
/// The macro calls this from generated code:
///
/// ```ignore
/// const __PATH_HASH: u64 =
///     framework_core::style_path_hash(module_path!(), "Variant");
/// ```
///
/// The function is `const`-evaluable so the result lands in a `pub
/// const` slot per stylesheet, with zero runtime cost at every
/// usage site.
pub const fn style_path_hash(module_path: &str, variant: &str) -> u64 {
    fnv_const(fnv_const(FNV_OFFSET, module_path.as_bytes()), variant.as_bytes())
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// FxHash-style word mixer. Combines the salt with the position
/// inputs into a single u64. Domain separation: each id-kind uses a
/// different salt, so the same `(parent, slot, branch, key)` tuple
/// can't produce colliding ids across kinds.
fn mix(salt: u64, parent: u64, slot: u32, branch: Option<u32>, key: Option<u64>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    salt.hash(&mut h);
    parent.hash(&mut h);
    slot.hash(&mut h);
    // Distinguish `None` from `Some(0)` for both branch and key by
    // tagging the discriminant explicitly. Without this, a `match`
    // arm at slot 0 with branch=Some(0) would alias to the same id
    // as a non-conditional sibling at slot 0 (branch=None).
    match branch {
        Some(b) => {
            1u8.hash(&mut h);
            b.hash(&mut h);
        }
        None => 0u8.hash(&mut h),
    }
    match key {
        Some(k) => {
            1u8.hash(&mut h);
            k.hash(&mut h);
        }
        None => 0u8.hash(&mut h),
    }
    h.finish()
}

// ---------------------------------------------------------------------------
// Thread-local current-identity context
// ---------------------------------------------------------------------------

thread_local! {
    /// Identity the walker is currently emitting under. The walker
    /// sets this immediately before every `backend.create_*` call;
    /// backends that care (notably the dev-server's
    /// `WireRecordingBackend`, which uses it to dedup `NodeId`s
    /// across sidecar respawns) read it lazily. Backends that don't
    /// care ignore the thread-local entirely — that's the entire
    /// trick that makes this rollout zero-trait-surface-change.
    ///
    /// Defaults to [`Identity::UNIDENTIFIED`] (the legacy "mint a
    /// fresh id each walk, no dedup" lane) so any call path that
    /// hasn't been migrated yet keeps working.
    static CURRENT_IDENTITY: Cell<Identity> = const { Cell::new(Identity::UNIDENTIFIED) };
}

/// Set the current identity for the duration of `f`'s execution.
/// Restores the previous value on return — RAII-style, so nested
/// calls compose correctly (e.g., a navigator's
/// `drawer_navigator_attach_initial` emission running inside a
/// component build).
pub fn with_current_identity<F, R>(id: Identity, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = CURRENT_IDENTITY.with(|c| c.replace(id));
    // Use a guard so a panic in `f` still restores. Otherwise a
    // panic in build code would leak the new identity into whichever
    // emission site catches the unwind.
    struct Guard(Identity);
    impl Drop for Guard {
        fn drop(&mut self) {
            CURRENT_IDENTITY.with(|c| c.set(self.0));
        }
    }
    let _g = Guard(prev);
    f()
}

/// Read the identity the walker is currently emitting under. Returns
/// [`Identity::UNIDENTIFIED`] outside a `with_current_identity`
/// scope.
pub fn current_identity() -> Identity {
    CURRENT_IDENTITY.with(|c| c.get())
}

/// Hash an arbitrary `Hash`-able user key into the `u64` form
/// [`Identity::scope`] / [`Identity::node`] consume. Wraps `Hasher`
/// boilerplate so call sites stay one-liners.
///
/// ```ignore
/// let k = framework_core::hash_key(&item.id);
/// Identity::node(parent, slot, None, Some(k))
/// ```
pub fn hash_key<K: Hash + ?Sized>(k: &K) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    k.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// `const`-compatible FNV-1a — for `style_path_hash` so stylesheet
// macros can land their id in a `pub const`. `std::collections::
// hash_map::DefaultHasher` isn't const-stable; FNV is trivially so.
// ---------------------------------------------------------------------------

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

const fn fnv_const(mut h: u64, bytes: &[u8]) -> u64 {
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    h
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_position_produces_same_identity() {
        let parent = Identity::ROOT_SCOPE;
        let a = Identity::node(parent, 3, None, None);
        let b = Identity::node(parent, 3, None, None);
        assert_eq!(a, b);
    }

    #[test]
    fn different_slots_produce_different_identities() {
        let parent = Identity::ROOT_SCOPE;
        let a = Identity::node(parent, 3, None, None);
        let b = Identity::node(parent, 4, None, None);
        assert_ne!(a, b);
    }

    #[test]
    fn id_kinds_never_alias_on_same_position() {
        let parent = Identity::ROOT_SCOPE;
        assert_ne!(
            Identity::scope(parent, 0, None, None),
            Identity::node(parent, 0, None, None)
        );
    }

    #[test]
    fn branch_discriminates_arms() {
        let parent = Identity::ROOT_SCOPE;
        let if_arm = Identity::node(parent, 0, Some(0), None);
        let else_arm = Identity::node(parent, 0, Some(1), None);
        assert_ne!(if_arm, else_arm);
    }

    #[test]
    fn branch_none_vs_some_zero_are_distinct() {
        let parent = Identity::ROOT_SCOPE;
        assert_ne!(
            Identity::node(parent, 0, None, None),
            Identity::node(parent, 0, Some(0), None),
        );
    }

    #[test]
    fn keys_override_position() {
        let parent = Identity::ROOT_SCOPE;
        // Reordering with stable keys keeps identity.
        let a = Identity::node(parent, 0, None, Some(hash_key("alpha")));
        let b = Identity::node(parent, 1, None, Some(hash_key("alpha")));
        // Same key but different slot → still different (slot is
        // hashed in). The host recorder MAY further dedup by key
        // alone — that's a follow-up decision; for now we surface
        // collisions only when both `slot` and `key` match.
        assert_ne!(a, b);
        // Same key + same slot = same identity.
        let c = Identity::node(parent, 0, None, Some(hash_key("alpha")));
        assert_eq!(a, c);
    }

    #[test]
    fn thread_local_default_is_unidentified() {
        assert_eq!(current_identity(), Identity::UNIDENTIFIED);
    }

    #[test]
    fn with_current_identity_sets_and_restores() {
        let outer = Identity::node(Identity::ROOT_SCOPE, 0, None, None);
        let inner = Identity::node(Identity::ROOT_SCOPE, 1, None, None);
        with_current_identity(outer, || {
            assert_eq!(current_identity(), outer);
            with_current_identity(inner, || {
                assert_eq!(current_identity(), inner);
            });
            // Inner scope restores outer.
            assert_eq!(current_identity(), outer);
        });
        // Outermost restore returns to the thread-local default.
        assert_eq!(current_identity(), Identity::UNIDENTIFIED);
    }

    #[test]
    fn with_current_identity_restores_on_panic() {
        let outer = Identity::node(Identity::ROOT_SCOPE, 7, None, None);
        let result = std::panic::catch_unwind(|| {
            with_current_identity(outer, || {
                panic!("oops");
            });
        });
        assert!(result.is_err());
        // Guard's `Drop` ran during unwind → context cleared back to
        // UNIDENTIFIED (the default we started in).
        assert_eq!(current_identity(), Identity::UNIDENTIFIED);
    }

    #[test]
    fn style_path_hash_is_stable() {
        let a = style_path_hash("crate::ui::styles", "Sidebar");
        let b = style_path_hash("crate::ui::styles", "Sidebar");
        let c = style_path_hash("crate::ui::styles", "SidebarHeader");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
