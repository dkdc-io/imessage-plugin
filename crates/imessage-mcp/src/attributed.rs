//! Decode `message.attributedBody` (typedstream NSAttributedString). Recent
//! macOS stores text here when `message.text` is NULL.
//!
//! Wire format (Apple NXTypedStream integer encoding):
//! - a single signed byte in -128..=127 encodes its own value as the length,
//! - `0x81` → next 2 bytes are a little-endian i16 length,
//! - `0x82` → next 4 bytes are a little-endian i32 length,
//! - `0x83` → next 8 bytes are a little-endian i64 length.
//!
//! The length is the UTF-8 byte count of the inline NSString payload, which
//! follows the `+` (0x2B) type tag after the `NSString` class name.

pub fn parse_attributed_body(blob: Option<&[u8]>) -> Option<String> {
    let buf = blob?;

    let marker = b"NSString";
    let mut i = memfind(buf, marker)?;
    i += marker.len();

    while i < buf.len() && buf[i] != 0x2B {
        i += 1;
    }
    if i >= buf.len() {
        return None;
    }
    i += 1;

    let len = read_typedstream_len(buf, &mut i)?;

    if i.checked_add(len)? > buf.len() {
        return None;
    }
    std::str::from_utf8(&buf[i..i + len])
        .ok()
        .map(str::to_owned)
}

fn read_typedstream_len(buf: &[u8], i: &mut usize) -> Option<usize> {
    if *i >= buf.len() {
        return None;
    }
    let tag = buf[*i];
    *i += 1;
    let signed: i64 = match tag {
        0x81 => {
            let bytes = buf.get(*i..*i + 2)?;
            *i += 2;
            i16::from_le_bytes([bytes[0], bytes[1]]) as i64
        }
        0x82 => {
            let bytes = buf.get(*i..*i + 4)?;
            *i += 4;
            i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64
        }
        0x83 => {
            let bytes = buf.get(*i..*i + 8)?;
            *i += 8;
            i64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ])
        }
        n => (n as i8) as i64,
    };
    if signed < 0 {
        return None;
    }
    Some(signed as usize)
}

fn memfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(len_prefix: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut blob = Vec::new();
        blob.extend_from_slice(b"streamtyped\x00NSString\x00\x2B");
        blob.extend_from_slice(len_prefix);
        blob.extend_from_slice(payload);
        blob
    }

    #[test]
    fn returns_none_on_empty() {
        assert_eq!(parse_attributed_body(None), None);
        assert_eq!(parse_attributed_body(Some(&[])), None);
    }

    #[test]
    fn decodes_literal_length() {
        let blob = wrap(&[0x03], b"abc");
        assert_eq!(parse_attributed_body(Some(&blob)).as_deref(), Some("abc"));
    }

    #[test]
    fn decodes_0x81_boundary_292() {
        // Production regression: 292-byte string encoded as 81 24 01.
        let payload = vec![b'a'; 292];
        let blob = wrap(&[0x81, 0x24, 0x01], &payload);
        let out = parse_attributed_body(Some(&blob)).unwrap();
        assert_eq!(out.len(), 292);
    }

    #[test]
    fn rejects_negative_literal_length() {
        let blob = wrap(&[0xFF], b"");
        assert_eq!(parse_attributed_body(Some(&blob)), None);
    }

    #[test]
    fn rejects_length_overrunning_buffer() {
        let blob = wrap(&[0x81, 0xFF, 0x00], b"short");
        assert_eq!(parse_attributed_body(Some(&blob)), None);
    }

    #[test]
    fn rejects_invalid_utf8() {
        let blob = wrap(&[0x02], &[0xFF, 0xFE]);
        assert_eq!(parse_attributed_body(Some(&blob)), None);
    }
}
