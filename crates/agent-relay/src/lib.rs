//! The wire format deck and its container-side relay share.
//!
//! deck forwards the user's ssh-agent into a running container, where no mount
//! and no root can reach: `<engine> exec` offers one channel across the
//! boundary — its own stdio — so every agent connection accepted *inside* the
//! container is multiplexed over that single pipe. This crate is the format
//! both ends speak, kept in one place so they cannot drift, plus the relay
//! binary itself (`src/main.rs`).
//!
//! A frame is `[id: u32 BE][len: u32 BE][bytes]`, and `len == 0` means "this
//! channel is done". Ids are minted by the *accepting* side (the relay, inside
//! the container); deck never allocates one, so they cannot collide.

/// Written to stderr by the relay once its socket is bound and listening.
/// Readiness has to come out of band: stdout is the mux channel.
pub const READY_MARKER: &str = "deck-agent-relay ready";

/// Bytes of frame header ahead of every payload.
pub const HEADER_LEN: usize = 8;

/// A frame bigger than this is a desynchronised stream, not a signing request:
/// the ssh-agent protocol caps a message at 256 KiB.
pub const MAX_FRAME: u32 = 1 << 20;

/// A length no agent message could carry. The stream cannot be resynchronised
/// from here — the next bytes would be read as a header they are not — so both
/// ends treat it as fatal rather than skipping ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramingLost {
    pub claimed: u32,
}

impl core::fmt::Display for FramingLost {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "relay framing lost sync (frame claims {} bytes)",
            self.claimed
        )
    }
}

/// Serialise one frame. An empty `payload` is the close signal for `id`.
pub fn encode_frame(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Pulls whole frames out of a byte stream that arrives in arbitrary chunks —
/// a pipe splits wherever it likes, including inside a header.
#[derive(Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// `Ok(None)` when the buffer holds no complete frame yet.
    pub fn next_frame(&mut self) -> Result<Option<(u32, Vec<u8>)>, FramingLost> {
        if self.buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let id = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        let claimed = u32::from_be_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]);
        if claimed > MAX_FRAME {
            return Err(FramingLost { claimed });
        }
        let len = claimed as usize;
        if self.buf.len() < HEADER_LEN + len {
            return Ok(None);
        }
        let payload = self.buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        self.buf.drain(..HEADER_LEN + len);
        Ok(Some((id, payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_survive_arbitrary_chunking() {
        let stream: Vec<u8> = [
            encode_frame(1, b"first"),
            encode_frame(2, b""),
            encode_frame(1, b"second request"),
        ]
        .concat();

        for chunk_size in 1..=stream.len() {
            let mut decoder = FrameDecoder::default();
            let mut seen = Vec::new();
            for chunk in stream.chunks(chunk_size) {
                decoder.push(chunk);
                while let Some(frame) = decoder.next_frame().expect("valid framing") {
                    seen.push(frame);
                }
            }
            assert_eq!(
                seen,
                vec![
                    (1, b"first".to_vec()),
                    (2, Vec::new()),
                    (1, b"second request".to_vec()),
                ],
                "chunk size {chunk_size}"
            );
        }
    }

    /// An empty frame is the close signal, so it has to decode as a frame
    /// rather than as "nothing to read yet".
    #[test]
    fn a_close_frame_is_a_frame() {
        let mut decoder = FrameDecoder::default();
        decoder.push(&encode_frame(7, b""));

        assert_eq!(decoder.next_frame().unwrap(), Some((7, Vec::new())));
        assert_eq!(decoder.next_frame().unwrap(), None);
    }

    #[test]
    fn an_impossible_length_is_fatal_not_skipped() {
        let mut decoder = FrameDecoder::default();
        let mut framed = 3u32.to_be_bytes().to_vec();
        framed.extend_from_slice(&(MAX_FRAME + 1).to_be_bytes());

        assert_eq!(
            decoder.next_frame(),
            Ok(None),
            "a partial header is not an error"
        );
        decoder.push(&framed);
        assert_eq!(
            decoder.next_frame(),
            Err(FramingLost {
                claimed: MAX_FRAME + 1
            })
        );
    }
}
