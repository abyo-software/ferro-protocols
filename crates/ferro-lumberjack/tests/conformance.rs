// SPDX-License-Identifier: Apache-2.0
//! Conformance tests against vendored realistic-synthetic Lumberjack v2
//! wire frames.
//!
//! These exercise the streaming `FrameDecoder` against the byte-for-byte
//! shape a Filebeat 8.x agent emits on the wire and the byte-for-byte
//! Logstash reply ACK. The fixtures are derived hand-from-spec rather
//! than captured pcap (see `tests/fixtures/README.md` for rationale).

use ferro_lumberjack::frame::{Frame, FrameDecoder};

const BEATS_WINDOW: &[u8] = include_bytes!("fixtures/beats_filebeat_window_v2.bin");
const LOGSTASH_ACK: &[u8] = include_bytes!("fixtures/logstash_ack_v2.bin");

#[test]
fn upstream_beats_window_decodes_to_window_plus_two_json_frames() {
    let mut d = FrameDecoder::new();
    d.feed(BEATS_WINDOW);

    // First frame: Window(2)
    let f1 = d.next_frame().expect("decode 1").expect("frame ready");
    let Frame::Window { count } = f1 else {
        panic!("expected Window, got {f1:?}");
    };
    assert_eq!(count, 2);

    // Second frame: JSON event 1, seq=1
    let f2 = d.next_frame().expect("decode 2").expect("frame ready");
    let Frame::Json { seq, payload } = f2 else {
        panic!("expected Json, got {f2:?}");
    };
    assert_eq!(seq, 1);
    let utf = std::str::from_utf8(&payload).expect("Filebeat payloads are UTF-8");
    assert!(
        utf.contains(r#""@timestamp":"2024-09-15T12:00:00.000Z""#),
        "expected real Filebeat-style @timestamp field",
    );
    assert!(utf.contains(r#""beat":"filebeat""#));
    assert!(utf.contains(r#""path":"/var/log/syslog""#));

    // Third frame: JSON event 2, seq=2
    let f3 = d.next_frame().expect("decode 3").expect("frame ready");
    let Frame::Json { seq, payload } = f3 else {
        panic!("expected Json, got {f3:?}");
    };
    assert_eq!(seq, 2);
    let utf = std::str::from_utf8(&payload).expect("UTF-8");
    assert!(
        utf.contains(r#""offset":1156"#),
        "expected the second event's log.offset progression",
    );

    // No further frames.
    assert!(d.next_frame().expect("decode 4").is_none());
}

#[test]
fn upstream_logstash_ack_decodes_with_correct_seq() {
    let mut d = FrameDecoder::new();
    d.feed(LOGSTASH_ACK);
    let f = d.next_frame().expect("decode").expect("ready");
    let Frame::Ack { seq } = f else {
        panic!("expected Ack, got {f:?}");
    };
    // The fixture ACKs the second JSON frame in the window, which is
    // the highest seq the receiver processed.
    assert_eq!(seq, 2);
    assert!(d.next_frame().expect("decode 2").is_none());
}

#[test]
fn upstream_beats_window_split_feed_chunks_decode_identically() {
    // Real TCP receivers don't get one whole frame per recv() call;
    // simulate a worst-case 1-byte-per-feed path and assert the decoder
    // produces the same frame sequence as a single-shot feed.
    let mut single = FrameDecoder::new();
    single.feed(BEATS_WINDOW);
    let mut single_frames = Vec::new();
    while let Some(f) = single.next_frame().expect("decode") {
        single_frames.push(f);
    }

    let mut chunked = FrameDecoder::new();
    let mut chunked_frames = Vec::new();
    for byte in BEATS_WINDOW {
        chunked.feed(std::slice::from_ref(byte));
        while let Some(f) = chunked.next_frame().expect("decode") {
            chunked_frames.push(f);
        }
    }

    assert_eq!(single_frames, chunked_frames);
}

#[test]
fn upstream_fixture_frame_header_bytes_match_lumberjack_v2_spec() {
    // First two bytes of any v2 frame are `'2'` followed by the
    // type byte. Validate the literal header bytes against the spec
    // so the fixture itself doesn't drift from the protocol.
    assert_eq!(BEATS_WINDOW[0], b'2', "version byte");
    assert_eq!(BEATS_WINDOW[1], b'W', "window frame type byte");
    // Window count is u32 BE: bytes 2..6
    let count = u32::from_be_bytes([
        BEATS_WINDOW[2],
        BEATS_WINDOW[3],
        BEATS_WINDOW[4],
        BEATS_WINDOW[5],
    ]);
    assert_eq!(count, 2);

    assert_eq!(LOGSTASH_ACK.len(), 6, "ACK frame is exactly 6 bytes");
    assert_eq!(LOGSTASH_ACK[0], b'2');
    assert_eq!(LOGSTASH_ACK[1], b'A');
}
