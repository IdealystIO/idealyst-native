//! NDEF over raw commands, for readers that only give a command channel
//! (PC/SC). Implements the NFC Forum Type 4 Tag (ISO 7816 file access) and
//! Type 2 Tag (MIFARE Ultralight / NTAG page memory) NDEF mappings on top
//! of a [`Transport`]. Pure logic: the tests drive it against simulated
//! tags.

use crate::{NdefMessage, NfcError};

/// A command channel to one tag. `transmit` sends an APDU and returns the
/// full response including the two status bytes.
pub(crate) trait Transport {
    fn transmit(&mut self, apdu: &[u8]) -> Result<Vec<u8>, NfcError>;
}

/// Split a response into data and status word, failing unless it is 90 00.
fn ok_data(resp: Vec<u8>, what: &str) -> Result<Vec<u8>, StatusError> {
    if resp.len() < 2 {
        return Err(StatusError::Short);
    }
    let (data, sw) = resp.split_at(resp.len() - 2);
    if sw == [0x90, 0x00] {
        Ok(data.to_vec())
    } else {
        Err(StatusError::Sw(what.to_string(), sw[0], sw[1]))
    }
}

enum StatusError {
    Short,
    Sw(String, u8, u8),
}

impl From<StatusError> for NfcError {
    fn from(e: StatusError) -> Self {
        match e {
            StatusError::Short => NfcError::Backend("short response from tag".into()),
            StatusError::Sw(what, a, b) => {
                NfcError::Backend(format!("{what} failed: status {a:02X}{b:02X}"))
            }
        }
    }
}

fn status(resp: &[u8]) -> Option<[u8; 2]> {
    (resp.len() >= 2).then(|| [resp[resp.len() - 2], resp[resp.len() - 1]])
}

// ---------------------------------------------------------------------------
// Type 4: NDEF Tag Application (AID D2760000850101), CC file E103.
// ---------------------------------------------------------------------------

const NDEF_AID: [u8; 7] = [0xD2, 0x76, 0x00, 0x00, 0x85, 0x01, 0x01];
const CC_FILE: [u8; 2] = [0xE1, 0x03];

/// What the Type 4 capability container says about the NDEF file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Type4Cc {
    /// Max bytes one READ BINARY returns.
    max_le: usize,
    /// Max bytes one UPDATE BINARY takes.
    max_lc: usize,
    file_id: [u8; 2],
    /// NDEF file size, including the 2-byte NLEN.
    file_size: usize,
    writable: bool,
}

fn select_file(t: &mut dyn Transport, id: [u8; 2]) -> Result<(), NfcError> {
    ok_data(t.transmit(&[0x00, 0xA4, 0x00, 0x0C, 0x02, id[0], id[1]])?, "SELECT file")?;
    Ok(())
}

fn read_binary(t: &mut dyn Transport, offset: usize, len: usize) -> Result<Vec<u8>, NfcError> {
    let apdu = [0x00, 0xB0, (offset >> 8) as u8, offset as u8, len as u8];
    Ok(ok_data(t.transmit(&apdu)?, "READ BINARY")?)
}

fn update_binary(t: &mut dyn Transport, offset: usize, data: &[u8]) -> Result<(), NfcError> {
    let mut apdu = vec![0x00, 0xD6, (offset >> 8) as u8, offset as u8, data.len() as u8];
    apdu.extend_from_slice(data);
    ok_data(t.transmit(&apdu)?, "UPDATE BINARY")?;
    Ok(())
}

/// Select the NDEF application and read its capability container. `None`
/// when the tag has no NDEF application (not a Type 4 NDEF tag).
fn type4_open(t: &mut dyn Transport) -> Result<Option<Type4Cc>, NfcError> {
    let mut select = vec![0x00, 0xA4, 0x04, 0x00, NDEF_AID.len() as u8];
    select.extend_from_slice(&NDEF_AID);
    select.push(0x00);
    let resp = t.transmit(&select)?;
    if status(&resp) != Some([0x90, 0x00]) {
        return Ok(None);
    }
    select_file(t, CC_FILE)?;
    let cc = read_binary(t, 0, 15)?;
    // CCLEN(2) version(1) MLe(2) MLc(2) then the NDEF File Control TLV:
    // T=04 L=06 file id(2) max size(2) read access(1) write access(1).
    if cc.len() < 15 || cc[7] != 0x04 || cc[8] < 0x06 {
        return Err(NfcError::Backend("malformed Type 4 capability container".into()));
    }
    let be = |i: usize| (usize::from(cc[i]) << 8) | usize::from(cc[i + 1]);
    Ok(Some(Type4Cc {
        // A READ BINARY Le byte tops out at 255 (short APDUs).
        max_le: be(3).clamp(1, 0xFF),
        max_lc: be(5).clamp(1, 0xFF),
        file_id: [cc[9], cc[10]],
        file_size: be(11),
        writable: cc[14] == 0x00,
    }))
}

pub(crate) fn type4_read(t: &mut dyn Transport) -> Result<Option<NdefMessage>, NfcError> {
    let Some(cc) = type4_open(t)? else { return Ok(None) };
    select_file(t, cc.file_id)?;
    let nlen = read_binary(t, 0, 2)?;
    if nlen.len() < 2 {
        return Err(NfcError::Backend("short NLEN from Type 4 tag".into()));
    }
    let len = (usize::from(nlen[0]) << 8) | usize::from(nlen[1]);
    let mut data = Vec::with_capacity(len);
    while data.len() < len {
        let chunk = read_binary(t, 2 + data.len(), (len - data.len()).min(cc.max_le))?;
        if chunk.is_empty() {
            return Err(NfcError::Backend("Type 4 tag returned no data".into()));
        }
        data.extend_from_slice(&chunk);
    }
    data.truncate(len);
    Ok(Some(NdefMessage::from_bytes(&data)?))
}

pub(crate) fn type4_write(t: &mut dyn Transport, bytes: &[u8]) -> Result<bool, NfcError> {
    let Some(cc) = type4_open(t)? else { return Ok(false) };
    if !cc.writable {
        return Err(NfcError::ReadOnly);
    }
    let capacity = cc.file_size.saturating_sub(2);
    if bytes.len() > capacity {
        return Err(NfcError::TooLarge { needed: bytes.len(), capacity });
    }
    select_file(t, cc.file_id)?;
    // NFC Forum T4T write procedure: zero NLEN, write the body, then set
    // NLEN — a reader never sees a length over a half-written body.
    update_binary(t, 0, &[0, 0])?;
    for (i, chunk) in bytes.chunks(cc.max_lc).enumerate() {
        update_binary(t, 2 + i * cc.max_lc, chunk)?;
    }
    update_binary(t, 0, &(bytes.len() as u16).to_be_bytes())?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Type 2: 4-byte pages. Page 3 is the capability container; the data area
// starts at page 4 and holds TLVs. Accessed through the PC/SC storage-card
// pseudo-APDUs READ BINARY (FF B0, 16 bytes = 4 pages) and UPDATE BINARY
// (FF D6, one 4-byte page), which contactless readers translate to the
// tag's native READ / WRITE.
// ---------------------------------------------------------------------------

const PAGE: usize = 4;
const DATA_START_PAGE: usize = 4;
const TLV_NULL: u8 = 0x00;
const TLV_NDEF: u8 = 0x03;
const TLV_TERMINATOR: u8 = 0xFE;

fn t2_read_pages(t: &mut dyn Transport, page: usize) -> Result<Vec<u8>, NfcError> {
    Ok(ok_data(t.transmit(&[0xFF, 0xB0, 0x00, page as u8, 0x10])?, "READ BINARY")?)
}

fn t2_write_page(t: &mut dyn Transport, page: usize, data: &[u8; PAGE]) -> Result<(), NfcError> {
    let mut apdu = vec![0xFF, 0xD6, 0x00, page as u8, PAGE as u8];
    apdu.extend_from_slice(data);
    ok_data(t.transmit(&apdu)?, "UPDATE BINARY")?;
    Ok(())
}

struct Type2Layout {
    /// Data area size in bytes (CC byte 2 × 8).
    data_size: usize,
    writable: bool,
    /// The data area as read.
    data: Vec<u8>,
}

/// Read the CC and the whole data area. `None` when page 3 lacks the NDEF
/// magic number (not NDEF formatted).
fn type2_open(t: &mut dyn Transport) -> Result<Option<Type2Layout>, NfcError> {
    let cc_block = t2_read_pages(t, 3)?;
    if cc_block.len() < PAGE || cc_block[0] != 0xE1 {
        return Ok(None);
    }
    let data_size = usize::from(cc_block[2]) * 8;
    let writable = cc_block[3] & 0x0F == 0x00;
    let mut data = Vec::with_capacity(data_size);
    while data.len() < data_size {
        let block = t2_read_pages(t, DATA_START_PAGE + data.len() / PAGE)?;
        if block.is_empty() {
            return Err(NfcError::Backend("Type 2 tag returned no data".into()));
        }
        data.extend_from_slice(&block);
    }
    data.truncate(data_size);
    Ok(Some(Type2Layout { data_size, writable, data }))
}

/// A TLV's length field: one byte, or 0xFF then two bytes.
fn tlv_len(data: &[u8], at: usize) -> Option<(usize, usize)> {
    match *data.get(at)? {
        0xFF => Some(((usize::from(*data.get(at + 1)?) << 8) | usize::from(*data.get(at + 2)?), 3)),
        n => Some((usize::from(n), 1)),
    }
}

/// Find the NDEF TLV: `(offset of its T byte, value offset, value length)`.
fn find_ndef_tlv(data: &[u8]) -> Option<(usize, usize, usize)> {
    let mut at = 0;
    while at < data.len() {
        match data[at] {
            TLV_NULL => at += 1,
            TLV_TERMINATOR => return None,
            tag => {
                let (len, len_bytes) = tlv_len(data, at + 1)?;
                let value = at + 1 + len_bytes;
                if tag == TLV_NDEF {
                    return Some((at, value, len));
                }
                at = value + len;
            }
        }
    }
    None
}

pub(crate) fn type2_read(t: &mut dyn Transport) -> Result<Option<NdefMessage>, NfcError> {
    let Some(layout) = type2_open(t)? else { return Ok(None) };
    let Some((_, value, len)) = find_ndef_tlv(&layout.data) else {
        // Formatted, but no NDEF TLV yet.
        return Ok(Some(NdefMessage::default()));
    };
    let bytes = layout
        .data
        .get(value..value + len)
        .ok_or_else(|| NfcError::Backend("NDEF TLV runs past the Type 2 data area".into()))?;
    Ok(Some(NdefMessage::from_bytes(bytes)?))
}

pub(crate) fn type2_write(t: &mut dyn Transport, bytes: &[u8]) -> Result<bool, NfcError> {
    let Some(layout) = type2_open(t)? else { return Ok(false) };
    if !layout.writable {
        return Err(NfcError::ReadOnly);
    }
    // Keep any Lock / Memory Control TLVs ahead of the NDEF TLV: they
    // describe reserved memory and must stay where they are.
    // NULL padding does not move the insertion point: a blank tag is all
    // zeros, and the NDEF TLV belongs at its start.
    let start = find_ndef_tlv(&layout.data).map(|(at, _, _)| at).unwrap_or_else(|| {
        let (mut at, mut insert) = (0, 0);
        while let Some(&tag) = layout.data.get(at) {
            match tag {
                TLV_NULL => at += 1,
                0x01 | 0x02 => match tlv_len(&layout.data, at + 1) {
                    Some((len, lb)) => {
                        at += 1 + lb + len;
                        insert = at;
                    }
                    None => break,
                },
                _ => break,
            }
        }
        insert
    });
    let mut tlv = vec![TLV_NDEF];
    if bytes.len() < 0xFF {
        tlv.push(bytes.len() as u8);
    } else {
        tlv.push(0xFF);
        tlv.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    }
    tlv.extend_from_slice(bytes);
    // The terminator is optional when the TLV fills the area exactly.
    let capacity = layout.data_size.saturating_sub(start + (tlv.len() - bytes.len()));
    if bytes.len() > capacity {
        return Err(NfcError::TooLarge { needed: bytes.len(), capacity });
    }
    if start + tlv.len() < layout.data_size {
        tlv.push(TLV_TERMINATOR);
    }
    let mut area = layout.data.clone();
    area[start..start + tlv.len()].copy_from_slice(&tlv);
    // Write whole pages from the one holding `start`; the bytes before
    // `start` in that page are rewritten with what was read.
    let first_page = start / PAGE;
    let last_page = (start + tlv.len()).div_ceil(PAGE);
    for page in first_page..last_page {
        let mut chunk = [0u8; PAGE];
        let from = page * PAGE;
        let to = (from + PAGE).min(area.len());
        chunk[..to - from].copy_from_slice(&area[from..to]);
        t2_write_page(t, DATA_START_PAGE + page, &chunk)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NdefRecord;

    /// A Type 4 tag: the NDEF application with a CC file and an NDEF file.
    struct SimType4 {
        selected_app: bool,
        file: Option<[u8; 2]>,
        cc: Vec<u8>,
        ndef: Vec<u8>,
        max_le: usize,
    }

    impl SimType4 {
        fn new(message: &[u8], file_size: u16, writable: bool) -> Self {
            let mut ndef = vec![0u8; usize::from(file_size)];
            ndef[..2].copy_from_slice(&(message.len() as u16).to_be_bytes());
            ndef[2..2 + message.len()].copy_from_slice(message);
            let fs = file_size.to_be_bytes();
            let cc = vec![
                0x00, 0x0F, 0x20, 0x00, 0x10, 0x00, 0x08, // CCLEN, v2.0, MLe=16, MLc=8
                0x04, 0x06, 0xE1, 0x04, fs[0], fs[1], 0x00, if writable { 0x00 } else { 0xFF },
            ];
            Self { selected_app: false, file: None, cc, ndef, max_le: 16 }
        }
    }

    impl Transport for SimType4 {
        fn transmit(&mut self, apdu: &[u8]) -> Result<Vec<u8>, NfcError> {
            let ok = |mut d: Vec<u8>| {
                d.extend_from_slice(&[0x90, 0x00]);
                Ok(d)
            };
            match (apdu[1], apdu[2]) {
                (0xA4, 0x04) => {
                    self.selected_app = apdu[5..12] == NDEF_AID;
                    if self.selected_app { ok(vec![]) } else { Ok(vec![0x6A, 0x82]) }
                }
                (0xA4, 0x00) if self.selected_app => {
                    self.file = Some([apdu[5], apdu[6]]);
                    ok(vec![])
                }
                (0xB0, _) => {
                    let off = (usize::from(apdu[2]) << 8) | usize::from(apdu[3]);
                    let len = usize::from(apdu[4]);
                    assert!(len <= self.max_le, "READ BINARY over MLe");
                    let file = if self.file == Some(CC_FILE) { &self.cc } else { &self.ndef };
                    ok(file[off..(off + len).min(file.len())].to_vec())
                }
                (0xD6, _) => {
                    let off = (usize::from(apdu[2]) << 8) | usize::from(apdu[3]);
                    let len = usize::from(apdu[4]);
                    assert!(len <= 8, "UPDATE BINARY over MLc");
                    self.ndef[off..off + len].copy_from_slice(&apdu[5..5 + len]);
                    ok(vec![])
                }
                _ => Ok(vec![0x6D, 0x00]),
            }
        }
    }

    /// A Type 2 tag (NTAG213-sized: 144-byte data area) behind PC/SC
    /// storage-card pseudo-APDUs.
    struct SimType2 {
        pages: Vec<[u8; 4]>,
    }

    impl SimType2 {
        fn new(cc: [u8; 4], data: &[u8]) -> Self {
            let data_pages = usize::from(cc[2]) * 8 / PAGE;
            let mut pages = vec![[0u8; 4]; DATA_START_PAGE + data_pages];
            pages[3] = cc;
            for (i, b) in data.iter().enumerate() {
                pages[DATA_START_PAGE + i / 4][i % 4] = *b;
            }
            Self { pages }
        }

        fn data_area(&self) -> Vec<u8> {
            self.pages[DATA_START_PAGE..].concat()
        }
    }

    impl Transport for SimType2 {
        fn transmit(&mut self, apdu: &[u8]) -> Result<Vec<u8>, NfcError> {
            let page = usize::from(apdu[3]);
            match apdu[1] {
                0xB0 => {
                    // NTAG READ returns 4 pages, wrapping past the end.
                    let mut out: Vec<u8> =
                        (0..4).flat_map(|i| self.pages[(page + i) % self.pages.len()]).collect();
                    out.extend_from_slice(&[0x90, 0x00]);
                    Ok(out)
                }
                0xD6 => {
                    self.pages[page].copy_from_slice(&apdu[5..9]);
                    Ok(vec![0x90, 0x00])
                }
                _ => Ok(vec![0x6D, 0x00]),
            }
        }
    }

    const NTAG213_CC: [u8; 4] = [0xE1, 0x10, 0x12, 0x00];

    fn uri_message() -> NdefMessage {
        NdefMessage::new(vec![NdefRecord::uri("https://example.com/badge/42")])
    }

    #[test]
    fn type4_reads_message_in_mle_sized_chunks() {
        let msg = NdefMessage::new(vec![NdefRecord::text("en", &"x".repeat(40))]);
        let bytes = msg.to_bytes().unwrap();
        let mut tag = SimType4::new(&bytes, 256, true);
        assert_eq!(type4_read(&mut tag).unwrap(), Some(msg));
    }

    #[test]
    fn type4_write_then_read_round_trips() {
        let mut tag = SimType4::new(&[], 128, true);
        assert_eq!(type4_read(&mut tag).unwrap(), Some(NdefMessage::default()));
        let bytes = uri_message().to_bytes().unwrap();
        assert!(type4_write(&mut tag, &bytes).unwrap());
        assert_eq!(&tag.ndef[..2], &(bytes.len() as u16).to_be_bytes());
        assert_eq!(type4_read(&mut tag).unwrap(), Some(uri_message()));
    }

    #[test]
    fn type4_rejects_read_only_and_oversized_writes() {
        let mut ro = SimType4::new(&[], 128, false);
        assert_eq!(type4_write(&mut ro, &[0xD0, 0, 0]), Err(NfcError::ReadOnly));
        let mut small = SimType4::new(&[], 10, true);
        assert_eq!(
            type4_write(&mut small, &[0; 9]),
            Err(NfcError::TooLarge { needed: 9, capacity: 8 })
        );
    }

    #[test]
    fn type4_without_ndef_application_is_not_ndef() {
        struct NoApp;
        impl Transport for NoApp {
            fn transmit(&mut self, _: &[u8]) -> Result<Vec<u8>, NfcError> {
                Ok(vec![0x6A, 0x82])
            }
        }
        assert_eq!(type4_read(&mut NoApp).unwrap(), None);
        assert!(!type4_write(&mut NoApp, &[]).unwrap());
    }

    #[test]
    fn type2_reads_ndef_tlv_after_lock_control_tlv() {
        let msg = uri_message().to_bytes().unwrap();
        let mut data = vec![0x01, 0x03, 0xA0, 0x0C, 0x34, TLV_NDEF, msg.len() as u8];
        data.extend_from_slice(&msg);
        data.push(TLV_TERMINATOR);
        let mut tag = SimType2::new(NTAG213_CC, &data);
        assert_eq!(type2_read(&mut tag).unwrap(), Some(uri_message()));
    }

    #[test]
    fn type2_write_keeps_lock_control_and_round_trips() {
        let lock = [0x01, 0x03, 0xA0, 0x0C, 0x34];
        let mut tag = SimType2::new(NTAG213_CC, &[&lock[..], &[TLV_NDEF, 0x00, TLV_TERMINATOR]].concat());
        assert_eq!(type2_read(&mut tag).unwrap(), Some(NdefMessage::default()));
        let bytes = uri_message().to_bytes().unwrap();
        assert!(type2_write(&mut tag, &bytes).unwrap());
        let area = tag.data_area();
        assert_eq!(&area[..5], &lock, "lock control TLV must survive the write");
        assert_eq!(area[5], TLV_NDEF);
        assert_eq!(area[7 + bytes.len()], TLV_TERMINATOR);
        assert_eq!(type2_read(&mut tag).unwrap(), Some(uri_message()));
    }

    #[test]
    fn type2_long_message_uses_three_byte_length() {
        let msg = NdefMessage::new(vec![NdefRecord::mime("a/b", vec![7u8; 300])]);
        // NTAG216: 872-byte data area.
        let mut tag = SimType2::new([0xE1, 0x10, 0x6D, 0x00], &[]);
        let bytes = msg.to_bytes().unwrap();
        assert!(type2_write(&mut tag, &bytes).unwrap());
        let area = tag.data_area();
        assert_eq!(&area[..4], &[TLV_NDEF, 0xFF, (bytes.len() >> 8) as u8, bytes.len() as u8]);
        assert_eq!(type2_read(&mut tag).unwrap(), Some(msg));
    }

    #[test]
    fn type2_rejects_read_only_oversized_and_unformatted() {
        let mut ro = SimType2::new([0xE1, 0x10, 0x12, 0x0F], &[]);
        assert_eq!(type2_write(&mut ro, &[0xD0, 0, 0]), Err(NfcError::ReadOnly));
        let mut tag = SimType2::new(NTAG213_CC, &[]);
        assert_eq!(
            type2_write(&mut tag, &[0; 200]),
            Err(NfcError::TooLarge { needed: 200, capacity: 142 })
        );
        let mut blank = SimType2::new([0x00, 0x00, 0x12, 0x00], &[]);
        assert_eq!(type2_read(&mut blank).unwrap(), None);
    }
}
