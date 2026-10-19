//! Modified UTF-7 mailbox names (RFC 3501, section 5.1.3), used for every folder name on
//! the wire (section 9). Everything above this crate sees plain Unicode names.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

/// Encodes a Unicode mailbox name for the wire.
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = Vec::new();
    for ch in name.chars() {
        if (' '..='~').contains(&ch) {
            flush(&mut out, &mut pending);
            if ch == '&' {
                out.push_str("&-");
            } else {
                out.push(ch);
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(ch.encode_utf16(&mut buf));
        }
    }
    flush(&mut out, &mut pending);
    out
}

fn flush(out: &mut String, pending: &mut Vec<u16>) {
    if pending.is_empty() {
        return;
    }
    let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
    out.push('&');
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = chunk.len() + 1;
        for i in 0..chars {
            let idx = ((n >> (18 - 6 * i)) & 0x3f) as usize;
            out.push(char::from(ALPHABET[idx]));
        }
    }
    out.push('-');
    pending.clear();
}

/// Decodes a wire mailbox name. A malformed shift sequence is kept verbatim rather than
/// rejected, so a server's odd name still shows up and round-trips.
pub fn decode(wire: &str) -> String {
    let mut out = String::with_capacity(wire.len());
    let mut rest = wire;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let encoded = &after[..end];
        if encoded.is_empty() {
            out.push('&');
        } else {
            match decode_run(encoded) {
                Some(text) => out.push_str(&text),
                None => out.push_str(&rest[start..start + end + 2]),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn decode_run(encoded: &str) -> Option<String> {
    let mut bits: u32 = 0;
    let mut nbits = 0;
    let mut bytes = Vec::new();
    for c in encoded.bytes() {
        let v = ALPHABET.iter().position(|a| *a == c)? as u32;
        bits = (bits << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            bytes.push((bits >> nbits) as u8);
            bits &= (1 << nbits) - 1;
        }
    }
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|p| u16::from_be_bytes([p[0], p[1]]))
        .collect();
    String::from_utf16(&units).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_3501_examples() {
        assert_eq!(
            encode("~peter/mail/台北/日本語"),
            "~peter/mail/&U,BTFw-/&ZeVnLIqe-"
        );
        assert_eq!(
            decode("~peter/mail/&U,BTFw-/&ZeVnLIqe-"),
            "~peter/mail/台北/日本語"
        );
        assert_eq!(encode("Tom & Jerry"), "Tom &- Jerry");
        assert_eq!(decode("Tom &- Jerry"), "Tom & Jerry");
    }

    #[test]
    fn round_trips() {
        for name in [
            "INBOX",
            "Café/日本語",
            "Entwürfe",
            "😀 emoji",
            "a&b&c",
            "[Gmail]/All Mail",
        ] {
            let wire = encode(name);
            assert!(wire.is_ascii(), "{wire}");
            assert_eq!(decode(&wire), name);
        }
        assert_eq!(encode("Café"), "Caf&AOk-");
    }

    #[test]
    fn malformed_is_kept() {
        assert_eq!(decode("bad&!!-x"), "bad&!!-x");
        assert_eq!(decode("open&abc"), "open&abc");
    }
}
