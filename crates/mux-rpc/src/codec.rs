//! Length-prefix wire codec: [u32 LE length][UTF-8 JSON body].
//!
//! Spec: prompts/docs/rpc-protocol.md §Protocol decision

use anyhow::{Context, Result};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum allowed message size (4 MiB) to guard against runaway allocations.
pub const MAX_MESSAGE_BYTES: u32 = 4 * 1024 * 1024;

/// Read one length-prefixed message from `reader`.
///
/// Reads the 4-byte LE length, validates it is ≤ MAX_MESSAGE_BYTES, then reads
/// the body.  Returns `Ok(None)` on a clean EOF before any bytes.
pub async fn read_message<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e).context("read message length"),
    }
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_MESSAGE_BYTES {
        anyhow::bail!("message too large: {len} bytes (max {MAX_MESSAGE_BYTES})");
    }
    let mut body = vec![0u8; len as usize];
    reader
        .read_exact(&mut body)
        .await
        .context("read message body")?;
    Ok(Some(body))
}

/// Write one length-prefixed message to `writer`.
///
/// Returns an error if the body exceeds MAX_MESSAGE_BYTES.
pub async fn write_message<W: AsyncWrite + Unpin>(writer: &mut W, body: &[u8]) -> Result<()> {
    let len = body.len();
    if len > MAX_MESSAGE_BYTES as usize {
        anyhow::bail!("message too large to send: {len} bytes (max {MAX_MESSAGE_BYTES})");
    }
    let len_bytes = (len as u32).to_le_bytes();
    writer
        .write_all(&len_bytes)
        .await
        .context("write message length")?;
    writer.write_all(body).await.context("write message body")?;
    Ok(())
}

/// Deserialise a JSON-encoded RPC message of type T from raw bytes.
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).context("decode JSON message")
}

/// Serialise a value to JSON bytes for sending.
pub fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).context("encode JSON message")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_read_roundtrip() {
        let (mut write_half, mut read_half) = tokio::io::duplex(1024);
        let body = b"hello world";
        write_message(&mut write_half, body).await.unwrap();
        drop(write_half); // flush EOF so read_exact returns
        let received = read_message(&mut read_half).await.unwrap().unwrap();
        assert_eq!(received, body);
    }

    #[tokio::test]
    async fn eof_before_length_returns_none() {
        let (write_half, mut read_half) = tokio::io::duplex(64);
        drop(write_half);
        let result = read_message(&mut read_half).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn oversized_message_rejected() {
        let (mut write_half, mut read_half) = tokio::io::duplex(64);
        // Write a length that exceeds MAX_MESSAGE_BYTES
        let huge_len = (MAX_MESSAGE_BYTES + 1).to_le_bytes();
        write_half.write_all(&huge_len).await.unwrap();
        drop(write_half);
        let result = read_message(&mut read_half).await;
        assert!(result.is_err());
    }

    #[test]
    fn encode_decode_json_roundtrip() {
        let val = serde_json::json!({"op": "Health"});
        let bytes = encode(&val).unwrap();
        let decoded: serde_json::Value = decode(&bytes).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn decode_malformed_json_returns_error() {
        let bad = b"not valid json {{{";
        let result: Result<serde_json::Value, _> = decode(bad);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn eof_mid_message_returns_error() {
        // Write a valid length header claiming 10 bytes, but drop before writing body.
        let (mut write_half, mut read_half) = tokio::io::duplex(64);
        let len_bytes: [u8; 4] = 10u32.to_le_bytes();
        write_half.write_all(&len_bytes).await.unwrap();
        drop(write_half); // EOF with body bytes missing
        let result = read_message(&mut read_half).await;
        assert!(
            result.is_err(),
            "mid-message EOF must be an error, not Ok(None)"
        );
    }

    #[tokio::test]
    async fn empty_body_roundtrip() {
        let (mut write_half, mut read_half) = tokio::io::duplex(64);
        let body: &[u8] = b"";
        write_message(&mut write_half, body).await.unwrap();
        drop(write_half);
        let received = read_message(&mut read_half).await.unwrap().unwrap();
        assert_eq!(received, body);
    }

    #[tokio::test]
    async fn multiple_messages_in_sequence() {
        let (mut write_half, mut read_half) = tokio::io::duplex(4096);
        let messages: &[&[u8]] = &[b"first", b"second", b"third"];
        for msg in messages {
            write_message(&mut write_half, msg).await.unwrap();
        }
        drop(write_half);
        for expected in messages {
            let received = read_message(&mut read_half).await.unwrap().unwrap();
            assert_eq!(&received, expected);
        }
        // After all messages, should get None (EOF)
        let eof = read_message(&mut read_half).await.unwrap();
        assert!(eof.is_none());
    }
}
