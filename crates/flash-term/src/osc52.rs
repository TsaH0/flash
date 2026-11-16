//! OSC 52: asks the terminal to put text on the system clipboard.
//!
//! Write-only: reading the clipboard this way is widely disabled, and
//! pasting from the system arrives as a bracketed paste anyway. Terminals
//! cap the payload (tmux and xterm by configuration), so large copies are
//! not sent.

/// The largest text sent (1 MiB); larger copies stay in the register only.
pub const MAX_TEXT: usize = 1 << 20;

/// The escape sequence setting the clipboard to `text`, or `None` if
/// `text` is longer than `max` bytes.
pub fn sequence(text: &str, max: usize) -> Option<Vec<u8>> {
    if text.len() > max {
        return None;
    }
    let mut out = Vec::with_capacity(8 + text.len().div_ceil(3) * 4);
    out.extend_from_slice(b"\x1b]52;c;");
    base64(text.as_bytes(), &mut out);
    out.push(0x07);
    Some(out)
}

/// Standard base64 with padding (RFC 4648).
fn base64(input: &[u8], out: &mut Vec<u8>) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let sextet = |i: u32| ALPHABET[((n >> (18 - 6 * i)) & 63) as usize];
        out.push(sextet(0));
        out.push(sextet(1));
        out.push(if chunk.len() > 1 { sextet(2) } else { b'=' });
        out.push(if chunk.len() > 2 { sextet(3) } else { b'=' });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        let mut out = Vec::new();
        base64(s.as_bytes(), &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(b64(input), want, "{input:?}");
        }
        assert_eq!(b64("\u{e9}\u{1F600}"), "w6nwn5iA");
    }

    #[test]
    fn sequence_wraps_and_caps() {
        assert_eq!(sequence("hi", 10).unwrap(), b"\x1b]52;c;aGk=\x07");
        assert_eq!(sequence("hello", 4), None);
        // The payload is base64, so no control byte from the text survives.
        let seq = sequence("\u{1b}]0;title\u{7}", MAX_TEXT).unwrap();
        assert_eq!(seq.iter().filter(|&&b| b == 0x07).count(), 1);
        assert_eq!(seq.iter().filter(|&&b| b == 0x1b).count(), 1);
    }
}
