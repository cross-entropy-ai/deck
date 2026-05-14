use super::{Osc52Forwarder, Osc52Sink, MAX_BUF_LEN};

/// Test sink that records each forwarded sequence verbatim.
#[derive(Default)]
struct VecSink {
    out: Vec<Vec<u8>>,
}

impl Osc52Sink for VecSink {
    fn write(&mut self, data: &[u8]) {
        self.out.push(data.to_vec());
    }
}

fn forwarder() -> Osc52Forwarder<VecSink> {
    Osc52Forwarder::with_sink(VecSink::default())
}

fn forwarded(fw: &Osc52Forwarder<VecSink>) -> &[Vec<u8>] {
    &fw.sink.out
}

#[test]
fn complete_sequence_in_one_chunk_is_forwarded() {
    let mut fw = forwarder();
    let seq = b"\x1b]52;c;SGVsbG8=\x07";
    fw.push(seq);
    assert_eq!(forwarded(&fw), &[seq.to_vec()]);
}

#[test]
fn st_terminated_sequence_is_forwarded() {
    let mut fw = forwarder();
    let seq = b"\x1b]52;c;SGVsbG8=\x1b\\";
    fw.push(seq);
    assert_eq!(forwarded(&fw), &[seq.to_vec()]);
}

#[test]
fn sequence_split_across_two_chunks_in_payload_is_forwarded() {
    let mut fw = forwarder();
    fw.push(b"\x1b]52;c;SGVs");
    assert!(forwarded(&fw).is_empty());
    fw.push(b"bG8=\x07");
    assert_eq!(forwarded(&fw), &[b"\x1b]52;c;SGVsbG8=\x07".to_vec()]);
}

#[test]
fn sequence_split_across_three_chunks_in_marker_payload_and_st() {
    let mut fw = forwarder();
    // Split mid-marker: "\x1b]5" then "2;c;SGVs" then payload + ESC,
    // then a final chunk that begins with the '\\' completing ST.
    fw.push(b"\x1b]5");
    assert!(forwarded(&fw).is_empty());
    fw.push(b"2;c;SGVs");
    assert!(forwarded(&fw).is_empty());
    fw.push(b"bG8=\x1b");
    assert!(forwarded(&fw).is_empty());
    fw.push(b"\\");
    assert_eq!(forwarded(&fw), &[b"\x1b]52;c;SGVsbG8=\x1b\\".to_vec()]);
}

#[test]
fn multiple_sequences_in_one_chunk_are_all_forwarded() {
    let mut fw = forwarder();
    let one = b"\x1b]52;c;QQ==\x07";
    let two = b"\x1b]52;c;Qg==\x1b\\";
    let mut chunk = Vec::new();
    chunk.extend_from_slice(one);
    chunk.extend_from_slice(two);
    fw.push(&chunk);
    assert_eq!(forwarded(&fw), &[one.to_vec(), two.to_vec()]);
}

#[test]
fn oversize_payload_is_dropped_then_next_sequence_parses() {
    let mut fw = forwarder();
    fw.push(b"\x1b]52;c;");
    // Spew more than MAX_BUF_LEN bytes of payload with no terminator.
    let junk = vec![b'A'; MAX_BUF_LEN + 16];
    fw.push(&junk);
    // Terminate the oversize sequence so the forwarder leaves overflow.
    fw.push(b"\x07");
    assert!(
        forwarded(&fw).is_empty(),
        "oversize sequence must be dropped, not forwarded"
    );

    // A following well-formed sequence still parses.
    let ok = b"\x1b]52;c;b2s=\x07";
    fw.push(ok);
    assert_eq!(forwarded(&fw), &[ok.to_vec()]);
}

#[test]
fn garbage_bytes_interleaved_with_valid_sequence_are_ignored() {
    let mut fw = forwarder();
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"random text\x1b[0m more\x1b]not52;skip");
    chunk.extend_from_slice(b"\x1b]52;c;b2s=\x07");
    chunk.extend_from_slice(b"trailing\x1bnoise");
    fw.push(&chunk);
    assert_eq!(forwarded(&fw), &[b"\x1b]52;c;b2s=\x07".to_vec()]);
}

#[test]
fn esc_inside_payload_not_followed_by_backslash_is_preserved() {
    // \x1b followed by something other than '\\' is part of the
    // payload (some terminals OSC-quote with embedded ESC bytes).
    let mut fw = forwarder();
    let seq = b"\x1b]52;c;A\x1bB\x07";
    fw.push(seq);
    assert_eq!(forwarded(&fw), &[seq.to_vec()]);
}

#[test]
fn marker_byte_one_at_a_time_still_starts_sequence() {
    let mut fw = forwarder();
    for &b in b"\x1b]52;c;X\x07" {
        fw.push(std::slice::from_ref(&b));
    }
    assert_eq!(forwarded(&fw), &[b"\x1b]52;c;X\x07".to_vec()]);
}
