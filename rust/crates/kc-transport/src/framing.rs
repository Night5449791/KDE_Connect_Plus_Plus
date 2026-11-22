//! Newline-delimited JSON framing, mirroring upstream `readLineBounded`.
//!
//! The protocol never embeds large data in the JSON packet, so a single line
//! is bounded by 512 KiB for the identity handshake and 32 MiB for the main
//! link. Reading byte-by-byte is intentional: a buffered reader would be able
//! to swallow bytes belonging to the following TLS handshake / packet.

use std::io;

use kc_protocol::{NetworkPacket, ProtocolError};
use tokio::io::{AsyncRead, AsyncReadExt};

/// Read one `\n`-terminated line, never accumulating more than `max_len` bytes.
///
/// Returns the line **including** its trailing newline, like upstream
/// `readLineBounded`. The caller should wrap slow streams (TLS) in a
/// `tokio::io::BufReader` for efficiency; the byte-at-a-time loop itself does
/// no read-ahead past the terminator.
pub async fn read_line_bounded<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_len: usize,
) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(256);
    let mut one = [0u8; 1];
    loop {
        let n = reader.read(&mut one).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stream closed before newline",
            ));
        }
        out.push(one[0]);
        if one[0] == b'\n' {
            return Ok(out);
        }
        if out.len() >= max_len {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
        }
    }
}

/// Decode one framed line into a [`NetworkPacket`].
pub fn decode_packet_line(line: &[u8]) -> Result<NetworkPacket, ProtocolError> {
    let text = std::str::from_utf8(line)?;
    NetworkPacket::deserialize(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_protocol::packet::MAX_PACKET_SIZE;

    #[tokio::test]
    async fn reads_one_line_including_newline() {
        let mut data = &b"hello\nworld\n"[..];
        let first = read_line_bounded(&mut data, 32).await.unwrap();
        assert_eq!(&first, b"hello\n");
        let second = read_line_bounded(&mut data, 32).await.unwrap();
        assert_eq!(&second, b"world\n");
    }

    #[tokio::test]
    async fn two_serialized_packets_frame_correctly() {
        let mut a = NetworkPacket::new("kdeconnect.ping");
        a.set_str("message", "one");
        let mut b = NetworkPacket::new("kdeconnect.ping");
        b.set_str("message", "two");
        let wire = format!("{}{}", a.serialize(), b.serialize());
        let mut data = wire.as_bytes();

        let line1 = read_line_bounded(&mut data, MAX_PACKET_SIZE).await.unwrap();
        let p1 = decode_packet_line(&line1).unwrap();
        assert_eq!(p1.get_str("message"), Some("one"));

        let line2 = read_line_bounded(&mut data, MAX_PACKET_SIZE).await.unwrap();
        let p2 = decode_packet_line(&line2).unwrap();
        assert_eq!(p2.get_str("message"), Some("two"));
    }

    #[tokio::test]
    async fn rejects_a_line_longer_than_the_limit() {
        // 5 payload bytes plus the newline would need a limit of 6.
        let mut data = &b"abcdef\n"[..];
        let err = read_line_bounded(&mut data, 5).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "line too long");
    }

    #[tokio::test]
    async fn accepts_a_line_exactly_at_the_limit() {
        // "abc\n" is 4 bytes including the terminator.
        let mut data = &b"abc\n"[..];
        let line = read_line_bounded(&mut data, 4).await.unwrap();
        assert_eq!(&line, b"abc\n");
    }

    #[tokio::test]
    async fn eof_before_newline_is_an_error() {
        let mut data = &b"unterminated"[..];
        let err = read_line_bounded(&mut data, 32).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
