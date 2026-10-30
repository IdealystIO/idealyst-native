//! Android permission requests waiting for their `onRequestPermissionsResult`,
//! keyed by the request code passed to `Activity.requestPermissions`.
//!
//! Platform-free so the park → answer → resolve path runs under host
//! `cargo test`; `android.rs` owns the JNI on either side of it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

use crate::oneshot;
use crate::PermissionStatus;

/// How the OS answered one request. The integer codes are the wire format
/// of `RustPermissions.nativeOnPermissionsResult` (the `RESULT_*` constants
/// in `runtime/kotlin/io/idealyst/permissions/RustPermissions.kt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Granted,
    Denied,
    /// The prompt was interrupted without an answer (empty result arrays).
    Cancelled,
}

impl Outcome {
    pub(crate) fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Outcome::Granted),
            1 => Some(Outcome::Denied),
            2 => Some(Outcome::Cancelled),
            _ => None,
        }
    }

    fn status(self) -> PermissionStatus {
        match self {
            Outcome::Granted => PermissionStatus::Granted,
            Outcome::Denied => PermissionStatus::Denied,
            // Nothing was decided, so a later request prompts again.
            Outcome::Cancelled => PermissionStatus::Undetermined,
        }
    }
}

static PENDING: Mutex<Option<HashMap<i32, oneshot::Sender<PermissionStatus>>>> = Mutex::new(None);

static NEXT_CODE: AtomicI32 = AtomicI32::new(1);

/// A fresh request code. `FragmentActivity.requestPermissions` rejects codes
/// outside the low 16 bits (it uses the high bits to route fragment
/// requests), so codes wrap within `1..=0xFFFF`.
fn next_code() -> i32 {
    loop {
        if let Some(code) = request_code(NEXT_CODE.fetch_add(1, Ordering::Relaxed)) {
            return code;
        }
    }
}

/// The request code for one counter value; `None` for the 0 the counter
/// passes through on each wrap.
fn request_code(counter: i32) -> Option<i32> {
    let v = counter & 0xFFFF;
    (v != 0).then_some(v)
}

/// Park a new request. Returns its code and the receiver that resolves when
/// [`complete`] is called with that code.
pub(crate) fn park() -> (i32, oneshot::Receiver<PermissionStatus>) {
    let code = next_code();
    let (tx, rx) = oneshot::channel(PermissionStatus::Undetermined);
    PENDING
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(code, tx);
    (code, rx)
}

/// Drop a parked request whose dialog never launched. Its receiver resolves
/// to `Undetermined`.
pub(crate) fn abandon(code: i32) {
    let tx = PENDING.lock().unwrap().as_mut().and_then(|m| m.remove(&code));
    drop(tx);
}

/// Resolve the request parked under `code`. A no-op for a code this crate
/// didn't park.
pub(crate) fn complete(code: i32, outcome: Outcome) {
    // Take the sender out before sending so the waker never runs under the lock.
    let tx = PENDING.lock().unwrap().as_mut().and_then(|m| m.remove(&code));
    if let Some(tx) = tx {
        tx.send(outcome.status());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reported hang: the first `request` parked here and nothing ever
    /// completed it. An answer delivered by code must resolve the waiter.
    #[tokio::test]
    async fn regression_parked_request_resolves_when_the_answer_arrives() {
        let (code, rx) = park();
        let waiter = tokio::spawn(rx);
        tokio::task::yield_now().await;
        complete(code, Outcome::Granted);
        assert_eq!(waiter.await.unwrap(), PermissionStatus::Granted);
    }

    #[tokio::test]
    async fn outcomes_map_to_statuses() {
        for (outcome, status) in [
            (Outcome::Granted, PermissionStatus::Granted),
            (Outcome::Denied, PermissionStatus::Denied),
            (Outcome::Cancelled, PermissionStatus::Undetermined),
        ] {
            let (code, rx) = park();
            complete(code, outcome);
            assert_eq!(rx.await, status);
        }
    }

    /// The Kotlin shim sends integer outcomes; an unknown one is rejected
    /// rather than read as a grant.
    #[test]
    fn outcome_wire_codes_match_the_kotlin_shim() {
        let kotlin = include_str!("../runtime/kotlin/io/idealyst/permissions/RustPermissions.kt");
        for (name, code) in [("RESULT_GRANTED", 0), ("RESULT_DENIED", 1), ("RESULT_CANCELLED", 2)] {
            assert!(
                kotlin.contains(&format!("const val {name} = {code}")),
                "{name} must be {code} in RustPermissions.kt"
            );
        }
        assert_eq!(Outcome::from_code(0), Some(Outcome::Granted));
        assert_eq!(Outcome::from_code(1), Some(Outcome::Denied));
        assert_eq!(Outcome::from_code(2), Some(Outcome::Cancelled));
        assert_eq!(Outcome::from_code(3), None);
    }

    #[tokio::test]
    async fn completing_one_code_leaves_the_others_parked() {
        let (a, rx_a) = park();
        let (b, rx_b) = park();
        assert_ne!(a, b);
        complete(b, Outcome::Denied);
        assert_eq!(rx_b.await, PermissionStatus::Denied);
        // A stray or repeated answer for an unknown code is ignored.
        complete(b, Outcome::Granted);
        complete(a, Outcome::Granted);
        assert_eq!(rx_a.await, PermissionStatus::Granted);
    }

    #[tokio::test]
    async fn abandoned_request_resolves_undetermined() {
        let (code, rx) = park();
        abandon(code);
        assert_eq!(rx.await, PermissionStatus::Undetermined);
    }

    #[test]
    fn codes_stay_in_the_fragment_activity_range() {
        assert_eq!(request_code(0xFFFF), Some(0xFFFF));
        assert_eq!(request_code(0x1_0000), None, "0 is skipped when the counter wraps");
        assert_eq!(request_code(0x1_0001), Some(1));
        assert_eq!(request_code(i32::MAX), Some(0xFFFF));
    }
}
