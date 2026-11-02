//! NDEF — the NFC Forum Data Exchange Format — as plain Rust values.
//!
//! An [`NdefMessage`] is a list of [`NdefRecord`]s; a record is a type name
//! format ([`Tnf`]), a type, an optional id and a payload. This module
//! encodes and decodes the binary form (NFC Forum *NDEF 1.0*) and builds /
//! reads the two record types nearly every tag carries: Text (`T`) and URI
//! (`U`) from the NFC Forum RTD specs. It is pure Rust with no platform code,
//! so the same values cross every backend and work in tests and on servers.
//!
//! Records stay raw on purpose: [`NdefRecord::as_text`] / [`as_uri`] read the
//! common types, and anything else (smart posters, Wi-Fi handover, app
//! payloads) is left to the caller to interpret from `record_type` +
//! `payload`.
//!
//! [`as_uri`]: NdefRecord::as_uri

use std::fmt;

/// The 3-bit Type Name Format of a record: how to read its `record_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tnf {
    /// No type, id or payload. Used to mark an empty message.
    Empty,
    /// An NFC Forum well-known type (RTD): `T` (text), `U` (URI), `Sp`
    /// (smart poster), …
    WellKnown,
    /// A MIME media type (RFC 2046), e.g. `application/json`.
    Media,
    /// An absolute URI (RFC 3986) naming the type.
    AbsoluteUri,
    /// An NFC Forum external type: `domain:type`, e.g. `example.com:badge`.
    External,
    /// Payload of unknown type; `record_type` must be empty.
    Unknown,
    /// Reserved by the spec (TNF 7). Decoded so a message round-trips.
    Reserved,
}

impl Tnf {
    /// The 3-bit wire value.
    pub fn to_bits(self) -> u8 {
        match self {
            Tnf::Empty => 0x00,
            Tnf::WellKnown => 0x01,
            Tnf::Media => 0x02,
            Tnf::AbsoluteUri => 0x03,
            Tnf::External => 0x04,
            Tnf::Unknown => 0x05,
            Tnf::Reserved => 0x07,
        }
    }

    /// Read a 3-bit wire value. `0x06` (Unchanged) only appears inside a
    /// chunked record and is not a record's own TNF, so it is `None`.
    pub fn from_bits(bits: u8) -> Option<Tnf> {
        Some(match bits & 0x07 {
            0x00 => Tnf::Empty,
            0x01 => Tnf::WellKnown,
            0x02 => Tnf::Media,
            0x03 => Tnf::AbsoluteUri,
            0x04 => Tnf::External,
            0x05 => Tnf::Unknown,
            0x07 => Tnf::Reserved,
            _ => return None,
        })
    }
}

/// Why bytes could not be decoded as NDEF, or a record could not be encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NdefError {
    /// The input ended inside a record.
    Truncated,
    /// The first record lacks the Message Begin flag, or a later one sets it.
    MessageBegin,
    /// No record carried the Message End flag.
    MissingMessageEnd,
    /// Bytes remain after the record carrying Message End.
    TrailingBytes,
    /// A chunked record was malformed (a middle chunk with a type, a TNF
    /// other than Unchanged, or the message ended mid-chunk).
    BadChunk,
    /// TNF 6 (Unchanged) on a record that does not continue a chunk.
    UnexpectedUnchanged,
    /// The record's fields break a rule for its TNF (an `Empty` record
    /// with content, an `Unknown` record with a type).
    InvalidRecord(&'static str),
    /// `record_type` is longer than 255 bytes, or `id` is.
    FieldTooLong,
}

impl fmt::Display for NdefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NdefError::Truncated => f.write_str("NDEF data ends inside a record"),
            NdefError::MessageBegin => f.write_str("NDEF Message Begin flag misplaced"),
            NdefError::MissingMessageEnd => f.write_str("NDEF message has no Message End record"),
            NdefError::TrailingBytes => f.write_str("bytes follow the NDEF Message End record"),
            NdefError::BadChunk => f.write_str("malformed chunked NDEF record"),
            NdefError::UnexpectedUnchanged => {
                f.write_str("TNF Unchanged outside a chunked NDEF record")
            }
            NdefError::InvalidRecord(why) => write!(f, "invalid NDEF record: {why}"),
            NdefError::FieldTooLong => f.write_str("NDEF type or id longer than 255 bytes"),
        }
    }
}

impl std::error::Error for NdefError {}

/// One NDEF record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NdefRecord {
    /// How to interpret `record_type`.
    pub tnf: Tnf,
    /// The type name; its meaning depends on `tnf` (`b"T"`, `b"text/plain"`,
    /// `b"example.com:badge"`, …). At most 255 bytes.
    pub record_type: Vec<u8>,
    /// Optional record id (a URI reference). At most 255 bytes.
    pub id: Vec<u8>,
    /// The record's content.
    pub payload: Vec<u8>,
}

/// A decoded NFC Forum Text record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRecord {
    /// IANA language code, e.g. `en` or `en-US`.
    pub language: String,
    /// The text.
    pub text: String,
}

/// URI Identifier Codes from the NFC Forum URI RTD: payload byte 0 indexes
/// this table and is prepended to the rest of the payload.
const URI_PREFIXES: [&str; 36] = [
    "",
    "http://www.",
    "https://www.",
    "http://",
    "https://",
    "tel:",
    "mailto:",
    "ftp://anonymous:anonymous@",
    "ftp://ftp.",
    "ftps://",
    "sftp://",
    "smb://",
    "nfs://",
    "ftp://",
    "dav://",
    "news:",
    "telnet://",
    "imap:",
    "rtsp://",
    "urn:",
    "pop:",
    "sip:",
    "sips:",
    "tftp:",
    "btspp://",
    "btl2cap://",
    "btgoep://",
    "tcpobex://",
    "irdaobex://",
    "file://",
    "urn:epc:id:",
    "urn:epc:tag:",
    "urn:epc:pat:",
    "urn:epc:raw:",
    "urn:epc:",
    "urn:nfc:",
];

impl NdefRecord {
    /// A record from raw parts.
    pub fn new(
        tnf: Tnf,
        record_type: impl Into<Vec<u8>>,
        id: impl Into<Vec<u8>>,
        payload: impl Into<Vec<u8>>,
    ) -> Self {
        Self { tnf, record_type: record_type.into(), id: id.into(), payload: payload.into() }
    }

    /// The Empty record. A message holding only this one is how a tag is
    /// "erased" while staying NDEF-formatted.
    pub fn empty() -> Self {
        Self::new(Tnf::Empty, Vec::new(), Vec::new(), Vec::new())
    }

    /// A well-known Text record, UTF-8 encoded. `language` is an IANA code
    /// such as `"en"`; it is truncated to the 63 bytes the format allows.
    pub fn text(language: &str, text: &str) -> Self {
        let lang = &language.as_bytes()[..language.len().min(0x3F)];
        let mut payload = Vec::with_capacity(1 + lang.len() + text.len());
        payload.push(lang.len() as u8);
        payload.extend_from_slice(lang);
        payload.extend_from_slice(text.as_bytes());
        Self::new(Tnf::WellKnown, b"T".to_vec(), Vec::new(), payload)
    }

    /// A well-known URI record. The longest matching standard prefix
    /// (`https://www.`, `tel:`, …) is abbreviated to its one-byte code, as
    /// readers expect.
    pub fn uri(uri: &str) -> Self {
        let (code, rest) = URI_PREFIXES
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, p)| uri.starts_with(*p))
            .max_by_key(|(_, p)| p.len())
            .map(|(i, p)| (i as u8, &uri[p.len()..]))
            .unwrap_or((0, uri));
        let mut payload = Vec::with_capacity(1 + rest.len());
        payload.push(code);
        payload.extend_from_slice(rest.as_bytes());
        Self::new(Tnf::WellKnown, b"U".to_vec(), Vec::new(), payload)
    }

    /// A MIME media record, e.g. `mime("application/json", bytes)`.
    pub fn mime(media_type: &str, data: impl Into<Vec<u8>>) -> Self {
        Self::new(Tnf::Media, media_type.as_bytes().to_vec(), Vec::new(), data)
    }

    /// An NFC Forum external-type record. `domain_type` is `domain:type`,
    /// e.g. `"example.com:badge"`; the spec makes it case-insensitive, so
    /// it is stored lowercased.
    pub fn external(domain_type: &str, payload: impl Into<Vec<u8>>) -> Self {
        Self::new(
            Tnf::External,
            domain_type.to_ascii_lowercase().into_bytes(),
            Vec::new(),
            payload,
        )
    }

    /// Whether this is a well-known record of type `t`.
    fn is_well_known(&self, t: &[u8]) -> bool {
        self.tnf == Tnf::WellKnown && self.record_type == t
    }

    /// Decode a well-known Text record. `None` when this is not one or its
    /// payload is malformed. Both UTF-8 and UTF-16 (with or without a byte
    /// order mark; big-endian when absent, per the RTD) are read.
    pub fn as_text(&self) -> Option<TextRecord> {
        if !self.is_well_known(b"T") {
            return None;
        }
        let (&status, rest) = self.payload.split_first()?;
        let lang_len = (status & 0x3F) as usize;
        if rest.len() < lang_len {
            return None;
        }
        let language = String::from_utf8(rest[..lang_len].to_vec()).ok()?;
        let body = &rest[lang_len..];
        let text = if status & 0x80 == 0 {
            String::from_utf8(body.to_vec()).ok()?
        } else {
            decode_utf16(body)?
        };
        Some(TextRecord { language, text })
    }

    /// Decode a well-known URI record into the full URI. `None` when this
    /// is not one, the prefix code is unknown, or the rest is not UTF-8.
    pub fn as_uri(&self) -> Option<String> {
        if !self.is_well_known(b"U") {
            return None;
        }
        let (&code, rest) = self.payload.split_first()?;
        let prefix = URI_PREFIXES.get(code as usize)?;
        let rest = std::str::from_utf8(rest).ok()?;
        Some(format!("{prefix}{rest}"))
    }

    /// The MIME type of a [`Tnf::Media`] record.
    pub fn media_type(&self) -> Option<&str> {
        (self.tnf == Tnf::Media).then(|| std::str::from_utf8(&self.record_type).ok()).flatten()
    }

    fn validate(&self) -> Result<(), NdefError> {
        if self.record_type.len() > 0xFF || self.id.len() > 0xFF {
            return Err(NdefError::FieldTooLong);
        }
        match self.tnf {
            Tnf::Empty
                if !(self.record_type.is_empty() && self.id.is_empty() && self.payload.is_empty()) =>
            {
                Err(NdefError::InvalidRecord("an Empty record carries a type, id or payload"))
            }
            Tnf::Unknown if !self.record_type.is_empty() => {
                Err(NdefError::InvalidRecord("an Unknown record carries a type"))
            }
            _ => Ok(()),
        }
    }
}

fn decode_utf16(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let (little, body) = match bytes {
        [0xFF, 0xFE, rest @ ..] => (true, rest),
        [0xFE, 0xFF, rest @ ..] => (false, rest),
        _ => (false, bytes),
    };
    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|c| if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
        .collect();
    String::from_utf16(&units).ok()
}

// Record header flag bits.
const MB: u8 = 0x80;
const ME: u8 = 0x40;
const CF: u8 = 0x20;
const SR: u8 = 0x10;
const IL: u8 = 0x08;
const TNF_UNCHANGED: u8 = 0x06;

/// An NDEF message: the unit a tag stores and a reader returns.
///
/// A message with no records is the decoded form of a tag that is NDEF
/// formatted but holds nothing (a zero-length NDEF file or TLV).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NdefMessage {
    /// The records, in order.
    pub records: Vec<NdefRecord>,
}

impl NdefMessage {
    /// A message from records.
    pub fn new(records: Vec<NdefRecord>) -> Self {
        Self { records }
    }

    /// Encode to the NDEF wire format. Short-record form is used for
    /// payloads under 256 bytes; records are never chunked. An empty
    /// message encodes to no bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, NdefError> {
        let mut out = Vec::new();
        let last = self.records.len().saturating_sub(1);
        for (i, r) in self.records.iter().enumerate() {
            r.validate()?;
            let short = r.payload.len() < 0x100;
            let mut header = r.tnf.to_bits();
            if i == 0 {
                header |= MB;
            }
            if i == last {
                header |= ME;
            }
            if short {
                header |= SR;
            }
            if !r.id.is_empty() {
                header |= IL;
            }
            out.push(header);
            out.push(r.record_type.len() as u8);
            if short {
                out.push(r.payload.len() as u8);
            } else {
                let len = u32::try_from(r.payload.len())
                    .map_err(|_| NdefError::InvalidRecord("payload longer than 4 GiB"))?;
                out.extend_from_slice(&len.to_be_bytes());
            }
            if !r.id.is_empty() {
                out.push(r.id.len() as u8);
            }
            out.extend_from_slice(&r.record_type);
            out.extend_from_slice(&r.id);
            out.extend_from_slice(&r.payload);
        }
        Ok(out)
    }

    /// Decode the NDEF wire format. Chunked records are reassembled into
    /// one record. Empty input decodes to a message with no records.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NdefError> {
        let mut records = Vec::new();
        if bytes.is_empty() {
            return Ok(Self { records });
        }
        let mut cur = Cursor { bytes, at: 0 };
        // A chunked record being assembled: its first chunk's tnf/type/id.
        let mut chunk: Option<NdefRecord> = None;
        let mut first = true;
        loop {
            let raw = cur.record()?;
            if first != (raw.header & MB != 0) {
                return Err(NdefError::MessageBegin);
            }
            first = false;
            let tnf_bits = raw.header & 0x07;
            let cf = raw.header & CF != 0;
            let me = raw.header & ME != 0;

            match chunk.as_mut() {
                Some(open) => {
                    // A middle or final chunk: Unchanged, no type, no id.
                    if tnf_bits != TNF_UNCHANGED || !raw.record_type.is_empty() || raw.has_id {
                        return Err(NdefError::BadChunk);
                    }
                    open.payload.extend_from_slice(raw.payload);
                    if !cf {
                        records.push(chunk.take().expect("open chunk"));
                    }
                }
                None => {
                    let tnf = Tnf::from_bits(tnf_bits).ok_or(NdefError::UnexpectedUnchanged)?;
                    let record = NdefRecord::new(
                        tnf,
                        raw.record_type.to_vec(),
                        raw.id.to_vec(),
                        raw.payload.to_vec(),
                    );
                    if cf {
                        chunk = Some(record);
                    } else {
                        record.validate()?;
                        records.push(record);
                    }
                }
            }

            if me {
                if chunk.is_some() {
                    return Err(NdefError::BadChunk);
                }
                if cur.at != bytes.len() {
                    return Err(NdefError::TrailingBytes);
                }
                return Ok(Self { records });
            }
            if cur.at == bytes.len() {
                return Err(NdefError::MissingMessageEnd);
            }
        }
    }
}

struct RawRecord<'a> {
    header: u8,
    has_id: bool,
    record_type: &'a [u8],
    id: &'a [u8],
    payload: &'a [u8],
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], NdefError> {
        let end = self.at.checked_add(n).ok_or(NdefError::Truncated)?;
        let s = self.bytes.get(self.at..end).ok_or(NdefError::Truncated)?;
        self.at = end;
        Ok(s)
    }

    fn byte(&mut self) -> Result<u8, NdefError> {
        Ok(self.take(1)?[0])
    }

    fn record(&mut self) -> Result<RawRecord<'a>, NdefError> {
        let header = self.byte()?;
        let type_len = self.byte()? as usize;
        let payload_len = if header & SR != 0 {
            self.byte()? as usize
        } else {
            let b = self.take(4)?;
            u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
        };
        let has_id = header & IL != 0;
        let id_len = if has_id { self.byte()? as usize } else { 0 };
        Ok(RawRecord {
            header,
            has_id,
            record_type: self.take(type_len)?,
            id: self.take(id_len)?,
            payload: self.take(payload_len)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_record_round_trips_through_wire_format() {
        let msg = NdefMessage::new(vec![NdefRecord::text("en", "hello")]);
        let bytes = msg.to_bytes().unwrap();
        // MB|ME|SR|WellKnown, type len 1, payload len 8, "T", status 2, "en", "hello".
        assert_eq!(bytes, b"\xD1\x01\x08T\x02enhello");
        let back = NdefMessage::from_bytes(&bytes).unwrap();
        assert_eq!(back, msg);
        let text = back.records[0].as_text().unwrap();
        assert_eq!(text, TextRecord { language: "en".into(), text: "hello".into() });
    }

    #[test]
    fn uri_record_uses_longest_prefix_code() {
        let r = NdefRecord::uri("https://www.example.com/a");
        assert_eq!(r.payload[0], 0x02, "https://www. beats https://");
        assert_eq!(&r.payload[1..], b"example.com/a");
        assert_eq!(r.as_uri().as_deref(), Some("https://www.example.com/a"));

        let tel = NdefRecord::uri("tel:+15551234");
        assert_eq!(tel.payload[0], 0x05);
        assert_eq!(tel.as_uri().as_deref(), Some("tel:+15551234"));

        let custom = NdefRecord::uri("myapp://open");
        assert_eq!(custom.payload[0], 0x00);
        assert_eq!(custom.as_uri().as_deref(), Some("myapp://open"));
    }

    #[test]
    fn multi_record_message_sets_mb_on_first_and_me_on_last() {
        let msg = NdefMessage::new(vec![
            NdefRecord::uri("https://example.com"),
            NdefRecord::mime("application/json", b"{}".to_vec()),
            NdefRecord::external("Example.com:Badge", vec![1, 2, 3]),
        ]);
        let bytes = msg.to_bytes().unwrap();
        assert_eq!(bytes[0] & (MB | ME), MB);
        let back = NdefMessage::from_bytes(&bytes).unwrap();
        assert_eq!(back.records.len(), 3);
        assert_eq!(back.records[1].media_type(), Some("application/json"));
        assert_eq!(back.records[2].record_type, b"example.com:badge");
        assert_eq!(back, msg);
    }

    #[test]
    fn long_payload_uses_four_byte_length() {
        let payload = vec![0xAB; 300];
        let msg = NdefMessage::new(vec![NdefRecord::mime("application/octet-stream", payload)]);
        let bytes = msg.to_bytes().unwrap();
        assert_eq!(bytes[0] & SR, 0, "300-byte payload must not use the short form");
        assert_eq!(&bytes[2..6], &300u32.to_be_bytes());
        assert_eq!(NdefMessage::from_bytes(&bytes).unwrap(), msg);
    }

    #[test]
    fn record_id_round_trips() {
        let mut r = NdefRecord::text("en", "x");
        r.id = b"#1".to_vec();
        let msg = NdefMessage::new(vec![r]);
        let bytes = msg.to_bytes().unwrap();
        assert_ne!(bytes[0] & IL, 0);
        assert_eq!(NdefMessage::from_bytes(&bytes).unwrap(), msg);
    }

    #[test]
    fn chunked_record_is_reassembled() {
        // Chunk 1: MB|CF|SR|Media "a/b" payload "he"; chunk 2: CF|SR|Unchanged
        // payload "ll"; chunk 3: ME|SR|Unchanged payload "o".
        let bytes = [
            0xB2, 0x03, 0x02, b'a', b'/', b'b', b'h', b'e', //
            0x36, 0x00, 0x02, b'l', b'l', //
            0x56, 0x00, 0x01, b'o',
        ];
        let msg = NdefMessage::from_bytes(&bytes).unwrap();
        assert_eq!(msg, NdefMessage::new(vec![NdefRecord::mime("a/b", b"hello".to_vec())]));
    }

    #[test]
    fn malformed_input_is_rejected() {
        assert_eq!(NdefMessage::from_bytes(&[0xD1, 0x01]), Err(NdefError::Truncated));
        // No MB on the first record.
        assert_eq!(NdefMessage::from_bytes(&[0x51, 0x01, 0x00, b'T']), Err(NdefError::MessageBegin));
        // No ME anywhere.
        assert_eq!(
            NdefMessage::from_bytes(&[0x91, 0x01, 0x00, b'T']),
            Err(NdefError::MissingMessageEnd)
        );
        // Bytes after ME.
        assert_eq!(
            NdefMessage::from_bytes(&[0xD1, 0x01, 0x00, b'T', 0x00]),
            Err(NdefError::TrailingBytes)
        );
        // Unchanged TNF on a standalone record.
        assert_eq!(
            NdefMessage::from_bytes(&[0xD6, 0x00, 0x00]),
            Err(NdefError::UnexpectedUnchanged)
        );
        // Message ends while a chunk is open.
        assert_eq!(
            NdefMessage::from_bytes(&[0xF2, 0x01, 0x00, b'a']),
            Err(NdefError::BadChunk)
        );
    }

    #[test]
    fn empty_record_and_empty_message() {
        let erase = NdefMessage::new(vec![NdefRecord::empty()]);
        assert_eq!(erase.to_bytes().unwrap(), [0xD0, 0x00, 0x00]);
        assert_eq!(NdefMessage::from_bytes(&[0xD0, 0x00, 0x00]).unwrap(), erase);
        assert_eq!(NdefMessage::from_bytes(&[]).unwrap(), NdefMessage::default());
        assert_eq!(NdefMessage::default().to_bytes().unwrap(), Vec::<u8>::new());

        let bad = NdefMessage::new(vec![NdefRecord::new(Tnf::Empty, b"x".to_vec(), vec![], vec![])]);
        assert!(matches!(bad.to_bytes(), Err(NdefError::InvalidRecord(_))));
    }

    #[test]
    fn utf16_text_record_decodes() {
        // Status: UTF-16 flag | lang len 2; BOM-less big-endian "hi".
        let r = NdefRecord::new(Tnf::WellKnown, b"T".to_vec(), vec![], b"\x82en\x00h\x00i".to_vec());
        assert_eq!(r.as_text().unwrap().text, "hi");
        // Little-endian with BOM.
        let r = NdefRecord::new(Tnf::WellKnown, b"T".to_vec(), vec![], b"\x82en\xFF\xFEh\x00i\x00".to_vec());
        assert_eq!(r.as_text().unwrap().text, "hi");
    }

    #[test]
    fn readers_return_none_for_other_types() {
        let mime = NdefRecord::mime("text/plain", b"x".to_vec());
        assert!(mime.as_text().is_none());
        assert!(mime.as_uri().is_none());
        assert!(NdefRecord::text("en", "x").media_type().is_none());
        // Unknown URI prefix code.
        let r = NdefRecord::new(Tnf::WellKnown, b"U".to_vec(), vec![], vec![0x40, b'x']);
        assert!(r.as_uri().is_none());
    }
}
