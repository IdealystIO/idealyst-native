//! The PC/SC backend against the machine's real PC/SC service. Needs no
//! reader: it checks the session lifecycle, which every reader setup goes
//! through. Card I/O itself is covered by the `tagproto` unit tests.
#![cfg(any(target_os = "macos", target_os = "windows"))]

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use nfc::{availability, Availability, NfcError, NfcSession, SessionOptions};

fn poll_once<F: Future>(f: F) -> Poll<F::Output> {
    pin!(f).as_mut().poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn session_lifecycle_against_the_system_pcsc_service() {
    // macOS and Windows always run a PC/SC service.
    assert_ne!(availability(), Availability::Unsupported);

    let session = NfcSession::begin(SessionOptions::default()).expect("begin");
    assert!(
        matches!(NfcSession::begin(SessionOptions::default()), Err(NfcError::Busy)),
        "a second session must be refused while one is open"
    );
    if availability() == Availability::Disabled {
        // No reader, so no tag can arrive: the wait stays pending.
        assert!(poll_once(session.next_tag()).is_pending());
    }
    session.close();

    // Closing frees the slot for the next session.
    let again = NfcSession::begin(SessionOptions::default()).expect("begin after close");
    again.close();
}
