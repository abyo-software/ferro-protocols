// SPDX-License-Identifier: Apache-2.0
//! Cross-module integration tests.
//!
//! These exercise public API surfaces in combinations the per-module
//! unit tests don't — typically: encoder → decoder → semantic check.

use ferro_lumberjack::Sequence;
use ferro_lumberjack::frame::{
    Frame, FrameDecoder, encode_ack, encode_compressed, encode_json_frame, encode_window,
};

#[test]
fn end_to_end_window_with_compressed_inner() {
    // Sender side: encode a window of 3 JSON events as a single
    // compressed frame, plus the explicit window header.
    let events: Vec<&[u8]> = vec![br#"{"msg":"a"}"#, br#"{"msg":"b"}"#, br#"{"msg":"c"}"#];
    let mut inner = Vec::new();
    for (i, e) in events.iter().enumerate() {
        let seq = u32::try_from(i).unwrap() + 1;
        inner.extend_from_slice(&encode_json_frame(seq, e));
    }
    let compressed = encode_compressed(6, &inner).unwrap();

    let mut wire = Vec::new();
    wire.extend_from_slice(&encode_window(u32::try_from(events.len()).unwrap()));
    wire.extend_from_slice(&compressed);

    // Receiver side: decode outer, then decode inner.
    let mut outer = FrameDecoder::new();
    outer.feed(&wire);

    let Frame::Window { count } = outer.next_frame().unwrap().unwrap() else {
        panic!("expected Window")
    };
    assert_eq!(count as usize, events.len());

    let Frame::Compressed { decompressed } = outer.next_frame().unwrap().unwrap() else {
        panic!("expected Compressed")
    };

    let mut inner_dec = FrameDecoder::new();
    inner_dec.feed(&decompressed);
    let mut got_seqs = Vec::new();
    while let Some(frame) = inner_dec.next_frame().unwrap() {
        if let Frame::Json { seq, payload } = frame {
            got_seqs.push((seq, String::from_utf8(payload).unwrap()));
        } else {
            panic!("expected Json inside compressed wrapper")
        }
    }
    assert_eq!(got_seqs.len(), 3);
    assert_eq!(got_seqs[0].0, 1);
    assert_eq!(got_seqs[2].1, r#"{"msg":"c"}"#);
}

#[test]
fn ack_seq_validation_with_wrap_around() {
    // A sender close to u32::MAX advances past the boundary; the
    // resulting ACK must validate under wrapping arithmetic.
    let base = Sequence::new(u32::MAX - 2);
    let after = base.advance(5); // seq value = 2 (wrapped)
    assert_eq!(after.value(), 2);

    // Encode the ACK that the receiver would have sent.
    let ack_wire = encode_ack(after.value());

    // Decode and check it lines up under wrapping subtraction.
    let mut d = FrameDecoder::new();
    d.feed(&ack_wire);
    let Frame::Ack { seq } = d.next_frame().unwrap().unwrap() else {
        panic!()
    };
    assert!(after.is_exactly_acked_by(seq));
    // Old "stale" comparisons would have rejected this — confirm the
    // RFC-1982-aware comparison accepts it.
    assert!(after.is_at_least_acked_by(seq));
}

#[test]
fn split_feed_does_not_lose_frames() {
    let big_payload = vec![b'X'; 10_000];
    let frame = encode_json_frame(42, &big_payload);

    let mut d = FrameDecoder::new();
    // Feed in three uneven chunks.
    d.feed(&frame[..7]);
    assert!(d.next_frame().unwrap().is_none());
    d.feed(&frame[7..3000]);
    assert!(d.next_frame().unwrap().is_none());
    d.feed(&frame[3000..]);
    let Frame::Json { seq, payload } = d.next_frame().unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(seq, 42);
    assert_eq!(payload.len(), big_payload.len());
}
