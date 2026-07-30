//! Wire serde registry for third-party primitive payloads.
//!
//! A third-party primitive carries a type-erased `Rc<dyn Any>` payload.
//! In a single process that is fine — the backend's
//! [`runtime_scene::Registry`] downcasts it. But over the runtime-server
//! wire the payload must travel from the recorder process to the device,
//! and `Rc<dyn Any>` cannot be serialized generically. So an SDK
//! registers a (serialize, deserialize) pair keyed by the payload's
//! `type_name` — a stable string across processes, unlike `TypeId`,
//! which differs per binary. The recorder serializes the payload into
//! [`Command::CreateExternal`](crate::Command::CreateExternal); the
//! client deserializes it back to a concrete `Rc<dyn Any>` and
//! dispatches to its own registry.
//!
//! ## Why it lives here
//!
//! This registry used to sit in `runtime-core/src/external.rs`, next to
//! the `Element::External` primitive it served. That primitive is gone;
//! the *wire* contract it encoded is not. The registry is plain data
//! plumbing — a thread-local map of closures, no runtime types in the
//! signatures — and its only two consumers are `dev-server` (recorder
//! side) and `dev-client` (replay side), both of which already depend on
//! this crate. Keeping it in the protocol crate means an SDK that wants
//! its payload to cross the wire depends on the wire, not on a runtime
//! core.
//!
//! The registry stores plain closures, so this crate still takes no
//! serde-format dependency: the SDK's closure owns the format choice.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

type Serializer = Rc<dyn Fn(&dyn Any) -> Option<Vec<u8>>>;
type Deserializer = Rc<dyn Fn(&[u8]) -> Option<Rc<dyn Any>>>;

thread_local! {
    static EXTERNAL_SERDE: RefCell<HashMap<&'static str, (Serializer, Deserializer)>> =
        RefCell::new(HashMap::new());
}

/// Register the wire (serialize, deserialize) pair for a third-party
/// payload type, keyed by `type_name` (use `std::any::type_name::<T>()`
/// — the same string the recorder stamps into
/// [`Command::CreateExternal`](crate::Command::CreateExternal)).
/// Idempotent — last write wins.
///
/// - `serialize`: downcast the `&dyn Any` to the payload type, encode to
///   bytes (`None` → the client falls back to the not-available
///   placeholder).
/// - `deserialize`: decode bytes back to a concrete `Rc<dyn Any>` whose
///   `TypeId` matches the client's registry entry.
pub fn register_external_serde(
    type_name: &'static str,
    serialize: impl Fn(&dyn Any) -> Option<Vec<u8>> + 'static,
    deserialize: impl Fn(&[u8]) -> Option<Rc<dyn Any>> + 'static,
) {
    EXTERNAL_SERDE.with(|c| {
        c.borrow_mut()
            .insert(type_name, (Rc::new(serialize), Rc::new(deserialize)));
    });
}

/// Serialize a payload for the wire. `None` when no serde is registered
/// for `type_name` (sentinel payloads carry no data) or the serializer
/// declines.
pub fn serialize_external_payload(type_name: &str, payload: &dyn Any) -> Option<Vec<u8>> {
    // Clone the closure out before invoking so the SDK closure can't
    // re-enter the registry borrow.
    let ser = EXTERNAL_SERDE.with(|c| c.borrow().get(type_name).map(|(s, _)| s.clone()));
    ser.and_then(|s| s(payload))
}

/// Deserialize a payload received over the wire. `None` when no serde is
/// registered for `type_name` (→ caller renders the placeholder) or the
/// bytes don't decode.
pub fn deserialize_external_payload(type_name: &str, bytes: &[u8]) -> Option<Rc<dyn Any>> {
    let de = EXTERNAL_SERDE.with(|c| c.borrow().get(type_name).map(|(_, d)| d.clone()));
    de.and_then(|d| d(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Payload {
        code: String,
    }

    fn register() {
        register_external_serde(
            "wire::payload_serde::tests::Payload",
            |any| {
                any.downcast_ref::<Payload>()
                    .map(|p| p.code.as_bytes().to_vec())
            },
            |bytes| {
                Some(Rc::new(Payload {
                    code: String::from_utf8(bytes.to_vec()).ok()?,
                }) as Rc<dyn Any>)
            },
        );
    }

    #[test]
    fn round_trips_a_registered_payload() {
        register();
        let payload = Payload {
            code: "fn main() {}".into(),
        };
        let bytes = serialize_external_payload("wire::payload_serde::tests::Payload", &payload)
            .expect("registered serializer");
        let back = deserialize_external_payload("wire::payload_serde::tests::Payload", &bytes)
            .expect("registered deserializer");
        assert_eq!(
            back.downcast_ref::<Payload>().expect("payload type"),
            &payload
        );
    }

    #[test]
    fn unregistered_type_name_is_none_on_both_halves() {
        assert!(serialize_external_payload("nope::Unregistered", &1u32).is_none());
        assert!(deserialize_external_payload("nope::Unregistered", b"x").is_none());
    }
}
