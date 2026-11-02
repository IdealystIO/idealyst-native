//! Platforms with no NFC API: every session fails with
//! [`NfcError::Unsupported`](crate::NfcError::Unsupported).

use crate::{Availability, NdefMessage, NfcError, SessionOptions, Tag};

const WHY: &str = "no NFC backend for this platform (Linux needs the `linux-pcsc` feature)";

pub(crate) fn availability() -> Availability {
    Availability::Unsupported
}

pub(crate) struct Session;

impl Session {
    pub(crate) fn begin(_options: &SessionOptions) -> Result<Session, NfcError> {
        Err(NfcError::Unsupported(WHY.into()))
    }

    pub(crate) async fn next_tag(&self) -> Result<Tag, NfcError> {
        Err(NfcError::Unsupported(WHY.into()))
    }

    pub(crate) fn set_prompt(&self, _text: &str) {}

    pub(crate) fn close(&self, _error_message: Option<&str>) {}
}

pub(crate) struct TagHandle;

impl TagHandle {
    pub(crate) async fn read_ndef(&self) -> Result<Option<NdefMessage>, NfcError> {
        Err(NfcError::Unsupported(WHY.into()))
    }

    pub(crate) async fn write_ndef(&self, _message: &NdefMessage) -> Result<(), NfcError> {
        Err(NfcError::Unsupported(WHY.into()))
    }

    pub(crate) async fn transceive(&self, _command: &[u8]) -> Result<Vec<u8>, NfcError> {
        Err(NfcError::Unsupported(WHY.into()))
    }
}
