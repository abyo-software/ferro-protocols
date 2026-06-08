// SPDX-License-Identifier: Apache-2.0
//! Pure-data frame codec — no I/O, no async runtime.
//!
//! Encoders are free functions that allocate on demand. The
//! [`FrameDecoder`] is a streaming state machine that accepts arbitrary
//! byte slices and yields fully decoded [`Frame`] values once enough
//! bytes are available. Useful from any runtime, sync or async, and
//! used by the optional [`crate::client`] under the hood.
//!
//! ## Frame layout (Lumberjack v2)
//!
//! ```text
//! Window:     [ '2' ][ 'W' ][ count : u32 BE ]                                (6 bytes)
//! JSON:       [ '2' ][ 'J' ][ seq : u32 BE ][ len : u32 BE ][ payload ]       (10 + len)
//! Compressed: [ '2' ][ 'C' ][ len : u32 BE ][ zlib(...) ]                     (6 + len)
//! Ack:        [ '2' ][ 'A' ][ seq : u32 BE ]                                  (6 bytes)
//! ```
//!
//! Frames in a "window" are sent as a Window frame followed by `count`
//! data frames (typically `J` frames; `D` legacy KV frames are decoded
//! as [`Frame::Unknown`]). When the receiver has processed all data
//! frames in the window it returns a single Ack frame whose `seq` is
//! the highest data-frame `seq` it processed.

use std::io::{Read, Write};

use crate::{DEFAULT_MAX_FRAME_PAYLOAD, FrameError, PROTOCOL_VERSION};

/// Wire byte for the Window frame type (`b'W'`).
pub const FRAME_TYPE_WINDOW: u8 = b'W';
/// Wire byte for the JSON data-frame type (`b'J'`).
pub const FRAME_TYPE_JSON: u8 = b'J';
/// Wire byte for the Compressed frame type (`b'C'`).
pub const FRAME_TYPE_COMPRESSED: u8 = b'C';
/// Wire byte for the ACK frame type (`b'A'`).
pub const FRAME_TYPE_ACK: u8 = b'A';
/// Wire byte for the legacy data-frame type (`b'D'`).
///
/// Modern Beats agents use [`FRAME_TYPE_JSON`] exclusively. The decoder
/// surfaces `D` frames as [`Frame::Unknown`] so a server-side consumer
/// can choose to decode them.
pub const FRAME_TYPE_DATA_LEGACY: u8 = b'D';

/// Identifies a Lumberjack v2 frame on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameType {
    /// Window — declares the number of data frames the sender will emit
    /// before expecting an ACK.
    Window,
    /// JSON-encoded event with a monotonic sequence number.
    Json,
    /// Compressed batch — payload is zlib-encoded inner frames.
    Compressed,
    /// ACK from receiver — `seq` is the highest sequence successfully
    /// processed.
    Ack,
}

impl FrameType {
    /// Wire byte that identifies this frame type.
    #[must_use]
    pub const fn wire_byte(self) -> u8 {
        match self {
            Self::Window => FRAME_TYPE_WINDOW,
            Self::Json => FRAME_TYPE_JSON,
            Self::Compressed => FRAME_TYPE_COMPRESSED,
            Self::Ack => FRAME_TYPE_ACK,
        }
    }
}

/// A fully decoded Lumberjack v2 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Frame {
    /// Window frame — `count` is the number of data frames the sender
    /// promises to emit before expecting an ACK.
    Window {
        /// Number of data frames in the window.
        count: u32,
    },
    /// JSON event frame.
    Json {
        /// Monotonic sequence number assigned by the sender.
        seq: u32,
        /// JSON payload bytes (UTF-8 — but the codec does not validate
        /// this; it is the caller's job).
        payload: Vec<u8>,
    },
    /// Compressed batch — the wrapped bytes are the *decompressed*
    /// inner frames (the codec hides the zlib boundary). To consume the
    /// inner frames, feed `decompressed` into a fresh [`FrameDecoder`]:
    ///
    /// ```
    /// # use ferro_lumberjack::frame::{FrameDecoder, Frame};
    /// # fn handle(outer: &mut FrameDecoder) -> Result<(), ferro_lumberjack::FrameError> {
    /// while let Some(frame) = outer.next_frame()? {
    ///     if let Frame::Compressed { decompressed } = frame {
    ///         let mut inner = FrameDecoder::new();
    ///         inner.feed(&decompressed);
    ///         while let Some(inner_frame) = inner.next_frame()? {
    ///             // …
    ///             let _ = inner_frame;
    ///         }
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    Compressed {
        /// Decompressed inner bytes (one or more concatenated frames).
        decompressed: Vec<u8>,
    },
    /// ACK frame.
    Ack {
        /// Highest sequence number the receiver has processed.
        seq: u32,
    },
    /// A frame the codec recognised as version-2 but whose type byte is
    /// not in the enumerated set above (currently `D` legacy KV frames
    /// and any future additions). The raw bytes — including the 2-byte
    /// header — are surfaced for forward-compatibility.
    Unknown {
        /// Wire type byte (e.g. `b'D'`).
        frame_type: u8,
        /// Full raw bytes of the frame, header included.
        raw: Vec<u8>,
    },
}

// ---------------------------------------------------------------------------
// Encoders
// ---------------------------------------------------------------------------

/// Encode a Window frame: `2 W <count: u32 BE>` (6 bytes total).
#[must_use]
pub fn encode_window(count: u32) -> [u8; 6] {
    let mut out = [0u8; 6];
    out[0] = PROTOCOL_VERSION;
    out[1] = FRAME_TYPE_WINDOW;
    out[2..6].copy_from_slice(&count.to_be_bytes());
    out
}

/// Encode an ACK frame: `2 A <seq: u32 BE>` (6 bytes total).
#[must_use]
pub fn encode_ack(seq: u32) -> [u8; 6] {
    let mut out = [0u8; 6];
    out[0] = PROTOCOL_VERSION;
    out[1] = FRAME_TYPE_ACK;
    out[2..6].copy_from_slice(&seq.to_be_bytes());
    out
}

/// Encode a JSON frame: `2 J <seq: u32 BE> <len: u32 BE> <payload>`.
///
/// `payload` is written verbatim — no UTF-8 validation, no JSON
/// validation. Both are the caller's responsibility.
#[must_use]
pub fn encode_json_frame(seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(10 + payload.len());
    out.push(PROTOCOL_VERSION);
    out.push(FRAME_TYPE_JSON);
    out.extend_from_slice(&seq.to_be_bytes());
    // Length field is u32 BE — see `FrameError::PayloadTooLarge` in the
    // decoder. We do not refuse to encode large payloads here; that is a
    // policy decision for the caller.
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Encode a Compressed frame containing the supplied (already-encoded)
/// inner frame bytes.
///
/// `level` is passed straight to [`flate2::Compression::new`]; valid
/// values are `0..=9`. Higher levels compress more at the cost of CPU.
///
/// Returns the wire bytes of the `C` frame, including the 6-byte header.
pub fn encode_compressed(level: u32, inner_frames: &[u8]) -> Result<Vec<u8>, FrameError> {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(level));
    encoder
        .write_all(inner_frames)
        .map_err(|e| FrameError::Compression(e.to_string()))?;
    let compressed = encoder
        .finish()
        .map_err(|e| FrameError::Compression(e.to_string()))?;

    let len = u32::try_from(compressed.len()).map_err(|_| FrameError::PayloadTooLarge {
        requested: compressed.len(),
        limit: u32::MAX as usize,
    })?;

    let mut out = Vec::with_capacity(6 + compressed.len());
    out.push(PROTOCOL_VERSION);
    out.push(FRAME_TYPE_COMPRESSED);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Streaming decoder
// ---------------------------------------------------------------------------

/// Streaming Lumberjack v2 frame decoder.
///
/// Accepts arbitrary slices of bytes via [`FrameDecoder::feed`] and emits
/// fully decoded [`Frame`] values via [`FrameDecoder::next_frame`]. The
/// internal buffer grows on demand and is compacted when consumed bytes
/// pass a threshold so long-lived decoders do not accumulate unbounded
/// dead capacity.
///
/// The decoder is **not** zero-copy: each `Frame` variant owns its
/// payload bytes. This is deliberate — yielded frames are typically
/// JSON-decoded immediately and the original bytes are dropped.
///
/// ### Resource bounding
///
/// Both raw payload lengths (`J`, `C` frames) **and** decompressed
/// inner sizes (`C` frame contents) are capped by
/// [`FrameDecoder::with_max_frame_payload`]. The default
/// is [`crate::DEFAULT_MAX_FRAME_PAYLOAD`] (64 MiB). This protects
/// servers reading from untrusted peers from naive resource-exhaustion
/// attacks (huge declared lengths, zlib bombs).
#[derive(Debug)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    /// Current read position in `buf`. Bytes before this index have
    /// already been consumed and will be reclaimed on the next compaction.
    read_pos: usize,
    /// Maximum bytes a single frame's payload (or, for `C` frames, the
    /// decompressed inner content) may occupy.
    max_frame_payload: usize,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    /// Create a decoder with the default 64 MiB per-frame size cap.
    #[must_use]
    pub const fn new() -> Self {
        Self::with_max_frame_payload(DEFAULT_MAX_FRAME_PAYLOAD)
    }

    /// Create a decoder with a custom per-frame size cap.
    ///
    /// Applies to both raw payload lengths and decompressed inner
    /// content of `C` frames. Set to `usize::MAX` for "no cap" (not
    /// recommended on inputs from untrusted peers).
    #[must_use]
    pub const fn with_max_frame_payload(max_frame_payload: usize) -> Self {
        Self {
            buf: Vec::new(),
            read_pos: 0,
            max_frame_payload,
        }
    }

    /// Append bytes to the internal buffer. The bytes will be parsed on
    /// subsequent calls to [`Self::next_frame`].
    pub fn feed(&mut self, bytes: &[u8]) {
        // Compact only when consumed bytes are at least 1/2 of the buffer
        // so we amortise the memmove cost over many feeds.
        if self.read_pos > 0 && self.read_pos >= self.buf.len() / 2 {
            self.buf.drain(..self.read_pos);
            self.read_pos = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// How many fed bytes remain unconsumed in the internal buffer.
    #[must_use]
    pub const fn pending(&self) -> usize {
        self.buf.len() - self.read_pos
    }

    /// Try to decode one frame from the buffered bytes.
    ///
    /// Returns:
    ///
    /// - `Ok(Some(frame))` — a complete frame is available.
    /// - `Ok(None)` — not enough bytes yet; feed more.
    /// - `Err(_)` — malformed input; the buffer is left untouched at
    ///   the offending position so the caller may inspect it. To
    ///   continue parsing, callers typically tear down the connection.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        let avail = self.pending();
        if avail < 2 {
            return Ok(None);
        }
        let header = &self.buf[self.read_pos..self.read_pos + 2];
        if header[0] != PROTOCOL_VERSION {
            return Err(FrameError::UnsupportedVersion(header[0]));
        }
        let frame_type = header[1];
        match frame_type {
            FRAME_TYPE_WINDOW => Ok(self.try_decode_window()),
            FRAME_TYPE_ACK => Ok(self.try_decode_ack()),
            FRAME_TYPE_JSON => self.try_decode_json(),
            FRAME_TYPE_COMPRESSED => self.try_decode_compressed(),
            FRAME_TYPE_DATA_LEGACY => self.try_decode_unknown_with_seq_count(b'D'),
            other => Err(FrameError::UnknownFrameType(other)),
        }
    }

    /// Reborrow `&self.buf[read_pos..read_pos + n]` as a `[u8; M]`.
    fn read_at<const M: usize>(&self, offset: usize) -> Option<[u8; M]> {
        let start = self.read_pos + offset;
        if self.buf.len() < start + M {
            return None;
        }
        let mut out = [0u8; M];
        out.copy_from_slice(&self.buf[start..start + M]);
        Some(out)
    }

    fn try_decode_window(&mut self) -> Option<Frame> {
        // Layout: 2 W <u32 count> = 6 bytes.
        if self.pending() < 6 {
            return None;
        }
        let count = u32::from_be_bytes(
            self.read_at::<4>(2)
                .expect("just verified ≥ 6 bytes pending"),
        );
        self.read_pos += 6;
        Some(Frame::Window { count })
    }

    fn try_decode_ack(&mut self) -> Option<Frame> {
        // Layout: 2 A <u32 seq> = 6 bytes.
        if self.pending() < 6 {
            return None;
        }
        let seq = u32::from_be_bytes(
            self.read_at::<4>(2)
                .expect("just verified ≥ 6 bytes pending"),
        );
        self.read_pos += 6;
        Some(Frame::Ack { seq })
    }

    fn try_decode_json(&mut self) -> Result<Option<Frame>, FrameError> {
        // Layout: 2 J <u32 seq> <u32 len> <payload> = 10 + len bytes.
        if self.pending() < 10 {
            return Ok(None);
        }
        let seq = u32::from_be_bytes(self.read_at::<4>(2).expect("≥ 10 pending"));
        let len_raw = u32::from_be_bytes(self.read_at::<4>(6).expect("≥ 10 pending"));
        let len = len_raw as usize;
        if len > self.max_frame_payload {
            return Err(FrameError::PayloadTooLarge {
                requested: len,
                limit: self.max_frame_payload,
            });
        }
        if self.pending() < 10 + len {
            return Ok(None);
        }
        let start = self.read_pos + 10;
        let payload = self.buf[start..start + len].to_vec();
        self.read_pos += 10 + len;
        Ok(Some(Frame::Json { seq, payload }))
    }

    fn try_decode_compressed(&mut self) -> Result<Option<Frame>, FrameError> {
        // Layout: 2 C <u32 len> <zlib bytes> = 6 + len bytes.
        if self.pending() < 6 {
            return Ok(None);
        }
        let len_raw = u32::from_be_bytes(self.read_at::<4>(2).expect("≥ 6 pending"));
        let len = len_raw as usize;
        if len > self.max_frame_payload {
            return Err(FrameError::PayloadTooLarge {
                requested: len,
                limit: self.max_frame_payload,
            });
        }
        if self.pending() < 6 + len {
            return Ok(None);
        }
        let start = self.read_pos + 6;
        let compressed = &self.buf[start..start + len];
        let decompressed = decompress_capped(compressed, self.max_frame_payload)?;
        self.read_pos += 6 + len;
        Ok(Some(Frame::Compressed { decompressed }))
    }

    /// Decode a known-but-not-handled frame type whose first 8 bytes
    /// after the header are `<seq: u32 BE> <count: u32 BE>` — i.e. the
    /// legacy `D` frame. Each KV pair is then `<key_len: u32 BE> <key>
    /// <value_len: u32 BE> <value>`. We don't decode the pairs; we just
    /// scan past them so subsequent frames can be parsed, and surface the
    /// raw bytes as [`Frame::Unknown`].
    fn try_decode_unknown_with_seq_count(
        &mut self,
        type_byte: u8,
    ) -> Result<Option<Frame>, FrameError> {
        if self.pending() < 10 {
            return Ok(None);
        }
        let pair_count = u32::from_be_bytes(self.read_at::<4>(6).expect("≥ 10 pending")) as usize;

        // Walk pair-by-pair without copying, computing total length.
        // 10 bytes header + 2 * (4-byte length prefix + content).
        let mut cursor = 10;
        for _ in 0..pair_count {
            // Need 4 bytes for key_len.
            if self.pending() < cursor + 4 {
                return Ok(None);
            }
            let key_len = u32::from_be_bytes(
                self.read_at::<4>(cursor)
                    .expect("just bounded by pending check"),
            ) as usize;
            if key_len > self.max_frame_payload {
                return Err(FrameError::PayloadTooLarge {
                    requested: key_len,
                    limit: self.max_frame_payload,
                });
            }
            cursor += 4 + key_len;

            if self.pending() < cursor + 4 {
                return Ok(None);
            }
            let val_len = u32::from_be_bytes(
                self.read_at::<4>(cursor)
                    .expect("just bounded by pending check"),
            ) as usize;
            if val_len > self.max_frame_payload {
                return Err(FrameError::PayloadTooLarge {
                    requested: val_len,
                    limit: self.max_frame_payload,
                });
            }
            cursor += 4 + val_len;
        }

        if self.pending() < cursor {
            return Ok(None);
        }
        let raw = self.buf[self.read_pos..self.read_pos + cursor].to_vec();
        self.read_pos += cursor;
        Ok(Some(Frame::Unknown {
            frame_type: type_byte,
            raw,
        }))
    }
}

/// Decompress zlib bytes into a fresh `Vec<u8>`, stopping if the output
/// would exceed `limit`. Returns `FrameError::DecompressedTooLarge` if
/// the stream would have produced more than `limit` bytes — i.e. zlib
/// bomb defence.
fn decompress_capped(compressed: &[u8], limit: usize) -> Result<Vec<u8>, FrameError> {
    use flate2::read::ZlibDecoder;

    // Read up to `limit + 1` bytes so we can distinguish "exactly limit"
    // from "more than limit".
    let mut out = Vec::new();
    let take_limit = u64::try_from(limit).unwrap_or(u64::MAX);
    let take_plus_one = take_limit.saturating_add(1);

    let decoder = ZlibDecoder::new(compressed);
    let mut bounded = decoder.take(take_plus_one);
    bounded
        .read_to_end(&mut out)
        .map_err(|e| FrameError::Decompression(e.to_string()))?;

    if out.len() > limit {
        return Err(FrameError::DecompressedTooLarge { limit });
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn encode_window_layout() {
        let bytes = encode_window(42);
        assert_eq!(bytes[0], b'2');
        assert_eq!(bytes[1], b'W');
        assert_eq!(
            u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]),
            42
        );
    }

    #[test]
    fn encode_ack_layout() {
        let bytes = encode_ack(7);
        assert_eq!(bytes[0], b'2');
        assert_eq!(bytes[1], b'A');
        assert_eq!(
            u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]),
            7
        );
    }

    #[test]
    fn encode_json_frame_layout() {
        let bytes = encode_json_frame(13, b"hello");
        assert_eq!(&bytes[..2], b"2J");
        assert_eq!(
            u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]),
            13
        );
        assert_eq!(
            u32::from_be_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]),
            5
        );
        assert_eq!(&bytes[10..], b"hello");
    }

    #[test]
    fn decode_window_round_trip() {
        let mut d = FrameDecoder::new();
        d.feed(&encode_window(123));
        let f = d.next_frame().unwrap().unwrap();
        assert_eq!(f, Frame::Window { count: 123 });
        assert!(d.next_frame().unwrap().is_none());
    }

    #[test]
    fn decode_ack_round_trip() {
        let mut d = FrameDecoder::new();
        d.feed(&encode_ack(987_654));
        assert_eq!(d.next_frame().unwrap(), Some(Frame::Ack { seq: 987_654 }));
    }

    #[test]
    fn decode_json_round_trip() {
        let mut d = FrameDecoder::new();
        d.feed(&encode_json_frame(1, br#"{"k":"v"}"#));
        let f = d.next_frame().unwrap().unwrap();
        let Frame::Json { seq, payload } = f else {
            panic!("expected Json")
        };
        assert_eq!(seq, 1);
        assert_eq!(payload, br#"{"k":"v"}"#);
    }

    #[test]
    fn decode_handles_concatenated_frames() {
        let mut d = FrameDecoder::new();
        let mut feed = Vec::new();
        feed.extend_from_slice(&encode_window(2));
        feed.extend_from_slice(&encode_json_frame(1, b"a"));
        feed.extend_from_slice(&encode_json_frame(2, b"bb"));
        feed.extend_from_slice(&encode_ack(2));
        d.feed(&feed);

        assert_eq!(d.next_frame().unwrap(), Some(Frame::Window { count: 2 }));
        let Some(Frame::Json { seq: 1, payload }) = d.next_frame().unwrap() else {
            panic!()
        };
        assert_eq!(payload, b"a");
        let Some(Frame::Json { seq: 2, payload }) = d.next_frame().unwrap() else {
            panic!()
        };
        assert_eq!(payload, b"bb");
        assert_eq!(d.next_frame().unwrap(), Some(Frame::Ack { seq: 2 }));
        assert!(d.next_frame().unwrap().is_none());
    }

    #[test]
    fn decode_handles_byte_at_a_time_feeds() {
        // Pathological feed pattern: one byte at a time.
        let mut d = FrameDecoder::new();
        let frame = encode_json_frame(5, b"abcdefgh");
        for byte in &frame {
            assert!(d.next_frame().unwrap().is_none());
            d.feed(std::slice::from_ref(byte));
        }
        let Frame::Json { seq, payload } = d.next_frame().unwrap().unwrap() else {
            panic!()
        };
        assert_eq!(seq, 5);
        assert_eq!(payload, b"abcdefgh");
    }

    #[test]
    fn decode_compressed_round_trip() {
        let inner = [
            encode_json_frame(1, b"hello").as_slice(),
            encode_json_frame(2, b"world").as_slice(),
        ]
        .concat();
        let outer = encode_compressed(6, &inner).unwrap();

        let mut d = FrameDecoder::new();
        d.feed(&outer);
        let Frame::Compressed { decompressed } = d.next_frame().unwrap().unwrap() else {
            panic!()
        };
        assert_eq!(decompressed, inner);
    }

    #[test]
    fn decode_rejects_bad_version() {
        let mut d = FrameDecoder::new();
        d.feed(&[b'1', b'W', 0, 0, 0, 1]);
        assert!(matches!(
            d.next_frame(),
            Err(FrameError::UnsupportedVersion(b'1'))
        ));
    }

    #[test]
    fn decode_rejects_unknown_frame_type() {
        let mut d = FrameDecoder::new();
        d.feed(&[b'2', b'Z', 0, 0, 0, 1]);
        assert!(matches!(
            d.next_frame(),
            Err(FrameError::UnknownFrameType(b'Z'))
        ));
    }

    #[test]
    fn decode_caps_oversize_json_payload() {
        let mut d = FrameDecoder::with_max_frame_payload(16);
        // Declares a 100-byte payload but our limit is 16.
        let mut buf = vec![b'2', b'J', 0, 0, 0, 1];
        buf.extend_from_slice(&100u32.to_be_bytes());
        d.feed(&buf);
        assert!(matches!(
            d.next_frame(),
            Err(FrameError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn decode_caps_zlib_bomb() {
        // 1 MiB of zeros compresses to ~1 KiB. If we cap at 64 KiB we
        // should reject decompression. Use a tiny limit to confirm.
        let original = vec![0u8; 1024 * 64];
        let frame = encode_compressed(9, &original).unwrap();
        let mut d = FrameDecoder::with_max_frame_payload(1024); // 1 KiB cap
        d.feed(&frame);
        match d.next_frame() {
            // ok — the compressed form may itself exceed 1 KiB, or the
            // decompressed cap may have triggered. Either is the intended
            // defence.
            Err(FrameError::DecompressedTooLarge { .. } | FrameError::PayloadTooLarge { .. }) => {}
            other => panic!("expected size-related error, got {other:?}"),
        }
    }

    #[test]
    fn legacy_d_frame_is_decoded_as_unknown_and_advances() {
        // Manually build a 'D' frame with one KV pair: key="foo", value="bar"
        let mut frame = Vec::new();
        frame.push(b'2');
        frame.push(b'D');
        frame.extend_from_slice(&5u32.to_be_bytes()); // seq
        frame.extend_from_slice(&1u32.to_be_bytes()); // pair_count
        // pair: key
        frame.extend_from_slice(&3u32.to_be_bytes());
        frame.extend_from_slice(b"foo");
        // pair: value
        frame.extend_from_slice(&3u32.to_be_bytes());
        frame.extend_from_slice(b"bar");

        // Append a known-good ack so we can confirm the cursor advanced.
        frame.extend_from_slice(&encode_ack(5));

        let mut d = FrameDecoder::new();
        d.feed(&frame);
        let f = d.next_frame().unwrap().unwrap();
        let Frame::Unknown { frame_type, raw } = f else {
            panic!()
        };
        assert_eq!(frame_type, b'D');
        assert_eq!(&raw[..2], b"2D");
        // Next frame should be the ack.
        assert_eq!(d.next_frame().unwrap(), Some(Frame::Ack { seq: 5 }));
    }

    #[test]
    fn decoder_compacts_after_consuming_half() {
        let mut d = FrameDecoder::new();
        for _ in 0..32 {
            d.feed(&encode_ack(1));
            let _ = d.next_frame().unwrap();
        }
        // After many ack consumptions, internal buffer must not grow
        // unboundedly.
        assert!(d.buf.capacity() < 1024, "buf cap = {}", d.buf.capacity());
    }

    // -----------------------------------------------------------------
    // Mutation-hardening: exact wire-byte assertions (frame.rs:63).
    // -----------------------------------------------------------------

    #[test]
    fn frame_type_wire_bytes_are_exact() {
        // Pins each FrameType's wire byte. Kills `wire_byte -> 0` and
        // `wire_byte -> 1` constant-return mutants: a constant return
        // would collapse all four to the same value and/or change them
        // off their documented ASCII letters.
        assert_eq!(FrameType::Window.wire_byte(), b'W');
        assert_eq!(FrameType::Json.wire_byte(), b'J');
        assert_eq!(FrameType::Compressed.wire_byte(), b'C');
        assert_eq!(FrameType::Ack.wire_byte(), b'A');

        // None of them is 0 or 1 (the mutant constants) and all four are
        // pairwise distinct.
        for ft in [
            FrameType::Window,
            FrameType::Json,
            FrameType::Compressed,
            FrameType::Ack,
        ] {
            assert_ne!(ft.wire_byte(), 0);
            assert_ne!(ft.wire_byte(), 1);
        }
        let bytes = [
            FrameType::Window.wire_byte(),
            FrameType::Json.wire_byte(),
            FrameType::Compressed.wire_byte(),
            FrameType::Ack.wire_byte(),
        ];
        let mut sorted = bytes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "wire bytes must be pairwise distinct");

        // And they line up with the module-level constants and the
        // encoders' emitted header bytes.
        assert_eq!(FrameType::Window.wire_byte(), FRAME_TYPE_WINDOW);
        assert_eq!(FrameType::Json.wire_byte(), FRAME_TYPE_JSON);
        assert_eq!(FrameType::Compressed.wire_byte(), FRAME_TYPE_COMPRESSED);
        assert_eq!(FrameType::Ack.wire_byte(), FRAME_TYPE_ACK);
        assert_eq!(encode_window(0)[1], FrameType::Window.wire_byte());
        assert_eq!(encode_ack(0)[1], FrameType::Ack.wire_byte());
        assert_eq!(encode_json_frame(0, b"")[1], FrameType::Json.wire_byte());
    }

    // -----------------------------------------------------------------
    // Mutation-hardening: feed() compaction predicate (frame.rs:272).
    //
    // `feed` compacts iff `read_pos > 0 && read_pos >= buf.len()/2`.
    // These tests pin each operand by constructing exact (read_pos,
    // buf.len()) states and asserting whether compaction fired —
    // observable via the in-module `buf`/`read_pos` fields.
    // -----------------------------------------------------------------

    /// Drive a decoder to a known `(read_pos, buf.len())` by feeding
    /// `total` filler bytes then consuming `consumed` of them via
    /// 6-byte ack frames. Returns the decoder positioned with
    /// `read_pos == consumed*6` and `buf.len() == total*6` (no compaction
    /// triggered yet because we feed once up front).
    fn decoder_at(acks_total: usize, acks_consumed: usize) -> FrameDecoder {
        let mut d = FrameDecoder::new();
        let mut feed = Vec::new();
        for _ in 0..acks_total {
            feed.extend_from_slice(&encode_ack(7));
        }
        d.feed(&feed);
        for _ in 0..acks_consumed {
            let _ = d.next_frame().unwrap();
        }
        d
    }

    #[test]
    fn feed_compacts_when_read_pos_at_least_half() {
        // read_pos == buf.len()/2 exactly: compaction MUST fire (`>=`).
        // 4 acks fed (buf.len()=24), 2 consumed (read_pos=12). 12 >= 12.
        let mut d = decoder_at(4, 2);
        assert_eq!(d.read_pos, 12);
        assert_eq!(d.buf.len(), 24);
        d.feed(&[]); // empty append still runs the compaction predicate
        assert_eq!(d.read_pos, 0, "compaction must reset read_pos at the boundary");
        assert_eq!(d.buf.len(), 12, "drained the 12 consumed bytes");
        // Remaining bytes still decode correctly.
        assert_eq!(d.next_frame().unwrap(), Some(Frame::Ack { seq: 7 }));
    }

    #[test]
    fn feed_does_not_compact_when_read_pos_below_half() {
        // read_pos just BELOW buf.len()/2: compaction must NOT fire.
        // 6 acks fed (buf.len()=36, half=18), 2 consumed (read_pos=12).
        let mut d = decoder_at(6, 2);
        assert_eq!(d.read_pos, 12);
        assert_eq!(d.buf.len(), 36);
        d.feed(&[]);
        assert_eq!(d.read_pos, 12, "below half → no compaction");
        assert_eq!(d.buf.len(), 36, "buffer untouched below threshold");
    }

    #[test]
    fn feed_does_not_compact_when_read_pos_zero() {
        // read_pos == 0 guard: even though 0 >= buf.len()/2 is false here,
        // pin the `read_pos > 0` operand. Fresh decoder, nothing consumed.
        let mut d = FrameDecoder::new();
        d.feed(&encode_ack(7));
        assert_eq!(d.read_pos, 0);
        let len_before = d.buf.len();
        d.feed(&[]);
        assert_eq!(d.read_pos, 0);
        assert_eq!(d.buf.len(), len_before);
    }

    #[test]
    fn feed_compacts_strictly_above_half() {
        // read_pos strictly GREATER than buf.len()/2: distinguishes the
        // mutant `> ` / `==` forms of the boundary that flips `>=` to
        // other operators. 4 acks (buf=24, half=12), 3 consumed (rp=18).
        let mut d = decoder_at(4, 3);
        assert_eq!(d.read_pos, 18);
        assert_eq!(d.buf.len(), 24);
        d.feed(&[]);
        assert_eq!(d.read_pos, 0, "well above half must compact");
        assert_eq!(d.buf.len(), 6);
        assert_eq!(d.next_frame().unwrap(), Some(Frame::Ack { seq: 7 }));
    }

    #[test]
    fn feed_half_uses_integer_division_not_mod_or_mul() {
        // Pins the `/` in `buf.len() / 2`. With buf.len()=24, `/2`=12,
        // `%2`=0, `*2`=48. read_pos=12:
        //   - real (`/2`=12): 12 >= 12 → compact.
        //   - `%2`=0:         12 >= 0  → would also compact (not
        //                     distinguishable here) — handled below.
        //   - `*2`=48:        12 >= 48 → would NOT compact → read_pos
        //                     stays 12. So `*` is killed here.
        let mut d = decoder_at(4, 2);
        assert_eq!((d.read_pos, d.buf.len()), (12, 24));
        d.feed(&[]);
        assert_eq!(d.read_pos, 0, "`* ` mutant would skip compaction");

        // Kill the `%` mutant: choose a state where `/2` says "don't
        // compact" but `%2` says "compact". buf.len()=36 → /2=18, %2=0.
        // read_pos=6: real 6>=18 false (no compact); `%`-mutant 6>=0 true
        // (would compact). So observing NO compaction kills `%`.
        let mut d2 = decoder_at(6, 1);
        assert_eq!((d2.read_pos, d2.buf.len()), (6, 36));
        d2.feed(&[]);
        assert_eq!(d2.read_pos, 6, "`%` mutant would wrongly compact here");
        assert_eq!(d2.buf.len(), 36);
    }

    #[test]
    fn feed_and_predicate_requires_both_operands() {
        // `&&` → `||`: with `||`, compaction fires if EITHER `read_pos>0`
        // OR `read_pos>=len/2`. State: read_pos=6, buf.len()=36 (half=18).
        // real: 6>0 && 6>=18 → false (no compact).
        // `||`-mutant: 6>0 || 6>=18 → true (would compact). Observing NO
        // compaction kills `||`.
        let mut d = decoder_at(6, 1);
        assert_eq!((d.read_pos, d.buf.len()), (6, 36));
        d.feed(&[]);
        assert_eq!(d.read_pos, 6, "`||` mutant would compact with rp>0 alone");
    }

    // -----------------------------------------------------------------
    // Mutation-hardening: per-decoder "need more bytes" boundaries.
    // For each `pending() < N` guard, feed N-1 / N bytes and assert
    // None vs Some(frame). This pins the comparison operator and the N.
    // -----------------------------------------------------------------

    #[test]
    fn window_needs_exactly_six_bytes() {
        // frame.rs:296 `pending() < 6`.
        let full = encode_window(0xDEAD_BEEF);
        // 5 bytes → not enough.
        let mut d = FrameDecoder::new();
        d.feed(&full[..5]);
        assert_eq!(d.next_frame().unwrap(), None, "5 bytes < 6 → None");
        // 6th byte arrives → decodes.
        d.feed(&full[5..6]);
        assert_eq!(
            d.next_frame().unwrap(),
            Some(Frame::Window { count: 0xDEAD_BEEF }),
            "exactly 6 bytes → Some",
        );
    }

    #[test]
    fn ack_needs_exactly_six_bytes() {
        // frame.rs:340 `pending() < 6`.
        let full = encode_ack(0x0102_0304);
        let mut d = FrameDecoder::new();
        d.feed(&full[..5]);
        assert_eq!(d.next_frame().unwrap(), None, "5 bytes → None");
        d.feed(&full[5..6]);
        assert_eq!(
            d.next_frame().unwrap(),
            Some(Frame::Ack { seq: 0x0102_0304 }),
        );
    }

    #[test]
    fn json_needs_ten_header_bytes_then_payload() {
        // frame.rs (try_decode_json header `pending() < 10`).
        let full = encode_json_frame(9, b"ABCD"); // 10 + 4 = 14 bytes
        let mut d = FrameDecoder::new();
        d.feed(&full[..9]);
        assert_eq!(d.next_frame().unwrap(), None, "9 header bytes → None");
        d.feed(&full[9..10]);
        // Header complete but payload not yet.
        assert_eq!(d.next_frame().unwrap(), None, "header only, no payload → None");
        d.feed(&full[10..]);
        let Some(Frame::Json { seq, payload }) = d.next_frame().unwrap() else {
            panic!("expected Json")
        };
        assert_eq!(seq, 9);
        assert_eq!(payload, b"ABCD");
    }

    #[test]
    fn json_payload_cap_is_strictly_greater_than() {
        // frame.rs:359 `len > max_frame_payload`. Boundary trio at the cap.
        // len == cap must be ACCEPTED; len == cap+1 must be REJECTED.
        let cap = 8;
        // Exactly cap: accepted (round-trips).
        let payload = vec![b'z'; cap];
        let frame = encode_json_frame(1, &payload);
        let mut d = FrameDecoder::with_max_frame_payload(cap);
        d.feed(&frame);
        let Some(Frame::Json { payload: got, .. }) = d.next_frame().unwrap() else {
            panic!("len == cap must be accepted (`>=` mutant would reject)")
        };
        assert_eq!(got.len(), cap);

        // cap + 1: rejected.
        let payload2 = vec![b'z'; cap + 1];
        let frame2 = encode_json_frame(1, &payload2);
        let mut d2 = FrameDecoder::with_max_frame_payload(cap);
        d2.feed(&frame2);
        assert!(
            matches!(d2.next_frame(), Err(FrameError::PayloadTooLarge { requested, limit })
                if requested == cap + 1 && limit == cap),
            "len == cap+1 must be rejected",
        );
    }

    #[test]
    fn compressed_needs_six_header_bytes() {
        // frame.rs:376 `pending() < 6` and 387 `pending() < 6 + len`.
        let inner = encode_json_frame(1, b"hi");
        let frame = encode_compressed(6, &inner).unwrap();
        let mut d = FrameDecoder::new();
        d.feed(&frame[..5]);
        assert_eq!(d.next_frame().unwrap(), None, "5 header bytes → None");
        d.feed(&frame[5..6]);
        // Header (len) known but body bytes not all present yet.
        assert_eq!(d.next_frame().unwrap(), None, "header only → None");
        d.feed(&frame[6..frame.len() - 1]);
        assert_eq!(d.next_frame().unwrap(), None, "body short by 1 → None");
        d.feed(&frame[frame.len() - 1..]);
        let Some(Frame::Compressed { decompressed }) = d.next_frame().unwrap() else {
            panic!("complete body → Some")
        };
        assert_eq!(decompressed, inner);
    }

    #[test]
    fn compressed_payload_cap_is_strictly_greater_than() {
        // frame.rs:381 `len > max_frame_payload`. The declared compressed
        // length is compared against the cap before decompression. Build
        // a C frame whose *compressed* length is exactly N, then cap at
        // N (accept path) and N-1 (reject path).
        let inner = encode_json_frame(1, b"hello world payload");
        let frame = encode_compressed(0, &inner).unwrap(); // level 0: stored
        let clen = u32::from_be_bytes([frame[2], frame[3], frame[4], frame[5]]) as usize;

        // cap == compressed len → accepted (the *decompressed* size also
        // fits because inner is small).
        let mut d = FrameDecoder::with_max_frame_payload(clen.max(inner.len()));
        d.feed(&frame);
        assert!(
            matches!(d.next_frame(), Ok(Some(Frame::Compressed { .. }))),
            "declared len == cap must pass the `>` gate",
        );

        // cap == compressed len - 1 → rejected at the length gate.
        let mut d2 = FrameDecoder::with_max_frame_payload(clen - 1);
        d2.feed(&frame);
        assert!(
            matches!(d2.next_frame(), Err(FrameError::PayloadTooLarge { requested, limit })
                if requested == clen && limit == clen - 1),
            "declared len > cap must be rejected",
        );
    }

    #[test]
    fn compressed_read_pos_advances_by_six_plus_len() {
        // frame.rs:393 `self.read_pos += 6 + len`. A trailing ack must be
        // decodable immediately after, proving the cursor advanced by
        // exactly 6+len (not 6*len via `*` mutant, nor 6 via dropped +len).
        let inner = encode_json_frame(1, b"payload-bytes");
        let mut wire = encode_compressed(0, &inner).unwrap();
        let clen = wire.len() - 6;
        wire.extend_from_slice(&encode_ack(55));
        let mut d = FrameDecoder::with_max_frame_payload(64 * 1024);
        d.feed(&wire);
        assert!(matches!(
            d.next_frame().unwrap(),
            Some(Frame::Compressed { .. })
        ));
        // If read_pos advanced by anything other than 6+clen, the next
        // frame would be garbage or None.
        assert_eq!(
            d.next_frame().unwrap(),
            Some(Frame::Ack { seq: 55 }),
            "cursor must advance by exactly 6 + len (={})",
            6 + clen,
        );
        assert_eq!(d.next_frame().unwrap(), None);
    }

    #[test]
    fn legacy_d_frame_header_boundary_is_ten() {
        // frame.rs:407 `pending() < 10`.
        let mut frame = Vec::new();
        frame.push(b'2');
        frame.push(b'D');
        frame.extend_from_slice(&5u32.to_be_bytes()); // seq
        frame.extend_from_slice(&0u32.to_be_bytes()); // pair_count = 0
        // 10-byte D frame with zero pairs.
        let mut d = FrameDecoder::new();
        d.feed(&frame[..9]);
        assert_eq!(d.next_frame().unwrap(), None, "9 header bytes → None");
        d.feed(&frame[9..10]);
        let Some(Frame::Unknown { frame_type, raw }) = d.next_frame().unwrap() else {
            panic!("10-byte zero-pair D frame → Unknown")
        };
        assert_eq!(frame_type, b'D');
        assert_eq!(raw.len(), 10);
    }

    #[test]
    fn legacy_d_frame_pair_length_boundaries() {
        // frame.rs:417/424/432/439/448 — per-pair key_len / val_len need
        // checks and the `cursor + 4` boundary. Build a D frame with one
        // KV pair (key="ab", value="cde") and feed it in cursor-precise
        // increments, asserting None until the very last byte arrives.
        let mut frame = Vec::new();
        frame.push(b'2');
        frame.push(b'D');
        frame.extend_from_slice(&5u32.to_be_bytes()); // seq
        frame.extend_from_slice(&1u32.to_be_bytes()); // pair_count = 1
        frame.extend_from_slice(&2u32.to_be_bytes()); // key_len
        frame.extend_from_slice(b"ab");
        frame.extend_from_slice(&3u32.to_be_bytes()); // val_len
        frame.extend_from_slice(b"cde");
        let total = frame.len(); // 10 + (4+2) + (4+3) = 23

        // Feed every prefix except the last byte → always None.
        for cut in 1..total {
            let mut d = FrameDecoder::new();
            d.feed(&frame[..cut]);
            assert_eq!(
                d.next_frame().unwrap(),
                None,
                "{cut} of {total} bytes must be incomplete",
            );
        }
        // Full frame → decodes, raw length == cursor (== total).
        let mut d = FrameDecoder::new();
        d.feed(&frame);
        let Some(Frame::Unknown { raw, .. }) = d.next_frame().unwrap() else {
            panic!("full D frame → Unknown")
        };
        assert_eq!(raw.len(), total, "cursor must equal 10 + 4+key + 4+val");
    }

    #[test]
    fn legacy_d_frame_key_and_val_len_caps() {
        // frame.rs:424/439 `key_len > cap` / `val_len > cap`. Boundary at
        // the cap: a pair whose key_len equals cap+1 must reject.
        let cap: u32 = 4;
        let cap_usize = cap as usize;
        let mut frame = Vec::new();
        frame.push(b'2');
        frame.push(b'D');
        frame.extend_from_slice(&1u32.to_be_bytes()); // seq
        frame.extend_from_slice(&1u32.to_be_bytes()); // pair_count
        frame.extend_from_slice(&(cap + 1).to_be_bytes()); // key_len = cap+1
        let mut d = FrameDecoder::with_max_frame_payload(cap_usize);
        d.feed(&frame);
        assert!(
            matches!(d.next_frame(), Err(FrameError::PayloadTooLarge { requested, limit })
                if requested == cap_usize + 1 && limit == cap_usize),
            "key_len > cap must reject",
        );

        // val_len > cap rejects too (key fits, value oversize).
        let mut frame2 = Vec::new();
        frame2.push(b'2');
        frame2.push(b'D');
        frame2.extend_from_slice(&1u32.to_be_bytes());
        frame2.extend_from_slice(&1u32.to_be_bytes());
        frame2.extend_from_slice(&1u32.to_be_bytes()); // key_len = 1
        frame2.push(b'k');
        frame2.extend_from_slice(&(cap + 1).to_be_bytes()); // val_len = cap+1
        let mut d2 = FrameDecoder::with_max_frame_payload(cap_usize);
        d2.feed(&frame2);
        assert!(
            matches!(d2.next_frame(), Err(FrameError::PayloadTooLarge { requested, limit })
                if requested == cap_usize + 1 && limit == cap_usize),
            "val_len > cap must reject",
        );
    }

    #[test]
    fn decompress_cap_is_inclusive_at_limit() {
        // frame.rs:479 `out.len() > limit`. Decompressed output of
        // exactly `limit` bytes must be ACCEPTED; `limit - 1` must be
        // REJECTED as DecompressedTooLarge. Use a 32-byte all-zero inner
        // so the *compressed* form is tiny and the compressed-length gate
        // never fires — only the decompressed-size gate is under test.
        let inner = vec![0u8; 32]; // zeros compress to well under 32 bytes
        let frame = encode_compressed(9, &inner).unwrap();
        let clen = frame.len() - 6;
        assert!(clen <= 31, "compressed zeros must stay tiny (got {clen})");

        // limit == decompressed size (32): accepted (`>=` mutant rejects).
        let mut d_ok = FrameDecoder::with_max_frame_payload(32);
        d_ok.feed(&frame);
        let Some(Frame::Compressed { decompressed }) = d_ok.next_frame().unwrap() else {
            panic!("decompressed len == limit must be accepted")
        };
        assert_eq!(decompressed.len(), 32);

        // limit == 31: the same 32-byte output exceeds it → rejected.
        let mut d_bad = FrameDecoder::with_max_frame_payload(31);
        d_bad.feed(&frame);
        assert!(
            matches!(
                d_bad.next_frame(),
                Err(FrameError::DecompressedTooLarge { limit: 31 })
            ),
            "decompressed len > limit must reject",
        );
    }

    #[test]
    fn next_frame_min_header_boundary_is_strictly_less_than_two() {
        // frame.rs:296 `avail < 2` (the "need a 2-byte header" guard).
        // The existing `*_needs_exactly_six_bytes` tests feed 5→6 bytes and
        // never exercise the `avail == 2` boundary, so `<` vs `<=` is not
        // distinguished there. Feed EXACTLY two bytes that form a complete,
        // header-decidable input: `[PROTOCOL_VERSION, <unknown type>]`.
        //
        //   real (`avail < 2`): 2 < 2 is false → reads the header → the
        //       unknown frame type surfaces as `Err(UnknownFrameType)`.
        //   `<=` mutant:        2 <= 2 is true  → returns `Ok(None)`.
        //
        // Asserting the Err at avail == 2 kills the `<=` mutant (and the
        // `==`/`>` siblings, which would mis-handle the 2-byte case too).
        let mut d = FrameDecoder::new();
        d.feed(&[PROTOCOL_VERSION, b'Z']);
        assert_eq!(d.pending(), 2, "exactly two bytes pending");
        assert!(
            matches!(d.next_frame(), Err(FrameError::UnknownFrameType(b'Z'))),
            "avail == 2 must read the header (not be treated as `need more`)",
        );

        // And a bad-version 2-byte input is likewise decided at avail == 2.
        let mut d2 = FrameDecoder::new();
        d2.feed(b"1W");
        assert_eq!(d2.pending(), 2);
        assert!(
            matches!(d2.next_frame(), Err(FrameError::UnsupportedVersion(b'1'))),
            "avail == 2 must reach the version check",
        );

        // One byte really IS too few → None (pins the `2` and the `<`).
        let mut d3 = FrameDecoder::new();
        d3.feed(&[PROTOCOL_VERSION]);
        assert_eq!(d3.next_frame().unwrap(), None, "1 byte < 2 → None");
    }

    #[test]
    fn compressed_header_boundary_is_strictly_less_than_six() {
        // frame.rs:376 `pending() < 6`. The existing
        // `compressed_needs_six_header_bytes` test feeds a frame whose
        // declared `len` is > 0, so at `pending() == 6` the *body* check
        // (`pending() < 6 + len`) returns None regardless of the `< 6`
        // operator — leaving `<` vs `<=` indistinguishable. Hand-craft a
        // C frame with declared `len == 0` so the header alone (6 bytes) is
        // a *complete* frame:
        //
        //   real (`pending() < 6`): 6 < 6 false → proceeds; the empty
        //       compressed body decodes to an empty Vec, yielding
        //       `Ok(Some(Frame::Compressed { decompressed: [] }))`.
        //   `<=` mutant:            6 <= 6 true → returns `Ok(None)`.
        //
        // Some-vs-None at pending == 6 kills the `<=` mutant.
        let mut d = FrameDecoder::new();
        d.feed(&[PROTOCOL_VERSION, FRAME_TYPE_COMPRESSED, 0, 0, 0, 0]);
        assert_eq!(d.pending(), 6, "exactly the 6-byte C header, len = 0");
        let Some(Frame::Compressed { decompressed }) = d.next_frame().unwrap() else {
            panic!(
                "pending == 6 must enter the body path (empty body → empty frame), \
                 not be treated as `need more bytes` (the `<=` mutant returns None)"
            )
        };
        assert!(decompressed.is_empty(), "len == 0 → empty decompressed body");

        // 5 bytes truly is short → None (pins the `6` and the `<`).
        let mut d2 = FrameDecoder::new();
        d2.feed(&[PROTOCOL_VERSION, FRAME_TYPE_COMPRESSED, 0, 0, 0]);
        assert_eq!(d2.next_frame().unwrap(), None, "5 < 6 → None");
    }

    #[test]
    fn legacy_d_frame_key_and_val_len_caps_accept_at_exactly_cap() {
        // frame.rs:424 `key_len > cap` and 439 `val_len > cap`. The existing
        // `*_caps` test only checks `cap + 1` (reject). The `>` vs `>=`
        // mutants flip behaviour at the EXACT boundary `len == cap`, which
        // must be ACCEPTED by the real `>` but REJECTED by the `>=` mutant.
        // Build a complete D frame whose key_len and val_len both equal the
        // cap and assert it decodes to a `Frame::Unknown`.
        let cap: u32 = 3;
        let cap_usize = cap as usize;
        let mut frame = Vec::new();
        frame.push(PROTOCOL_VERSION);
        frame.push(FRAME_TYPE_DATA_LEGACY);
        frame.extend_from_slice(&7u32.to_be_bytes()); // seq
        frame.extend_from_slice(&1u32.to_be_bytes()); // pair_count = 1
        frame.extend_from_slice(&cap.to_be_bytes()); // key_len == cap (3)
        frame.extend_from_slice(b"abc"); // 3-byte key
        frame.extend_from_slice(&cap.to_be_bytes()); // val_len == cap (3)
        frame.extend_from_slice(b"xyz"); // 3-byte value

        let mut d = FrameDecoder::with_max_frame_payload(cap_usize);
        d.feed(&frame);
        let Some(Frame::Unknown { frame_type, raw }) = d.next_frame().unwrap() else {
            panic!(
                "key_len == cap && val_len == cap must be ACCEPTED \
                 (the `>=` mutants on 424/439 would reject as PayloadTooLarge)"
            )
        };
        assert_eq!(frame_type, b'D');
        // raw == 10 header + (4 + 3) key + (4 + 3) val = 24 bytes.
        assert_eq!(raw.len(), 24);

        // Sanity: cap - 1 (so len == cap is now strictly over) DOES reject,
        // confirming the gate is live and the accept above was meaningful.
        let mut d2 = FrameDecoder::with_max_frame_payload(cap_usize - 1);
        d2.feed(&frame);
        assert!(
            matches!(d2.next_frame(), Err(FrameError::PayloadTooLarge { requested, limit })
                if requested == cap_usize && limit == cap_usize - 1),
            "key_len (== cap) now strictly exceeds the lowered cap → reject",
        );
    }

    proptest! {
        #[test]
        fn prop_json_frame_round_trip(seq: u32, payload: Vec<u8>) {
            let bytes = encode_json_frame(seq, &payload);
            let mut d = FrameDecoder::new();
            d.feed(&bytes);
            let frame = d.next_frame().unwrap().unwrap();
            let Frame::Json { seq: got_seq, payload: got_payload } = frame else {
                panic!("expected Json")
            };
            prop_assert_eq!(got_seq, seq);
            prop_assert_eq!(got_payload, payload);
            prop_assert!(d.next_frame().unwrap().is_none());
        }

        #[test]
        fn prop_window_round_trip(count: u32) {
            let bytes = encode_window(count);
            let mut d = FrameDecoder::new();
            d.feed(&bytes);
            prop_assert_eq!(d.next_frame().unwrap(), Some(Frame::Window { count }));
        }

        #[test]
        fn prop_ack_round_trip(seq: u32) {
            let bytes = encode_ack(seq);
            let mut d = FrameDecoder::new();
            d.feed(&bytes);
            prop_assert_eq!(d.next_frame().unwrap(), Some(Frame::Ack { seq }));
        }

        #[test]
        fn prop_compressed_round_trip(payloads in proptest::collection::vec(any::<Vec<u8>>(), 1..16)) {
            let mut inner = Vec::new();
            for (i, p) in payloads.iter().enumerate() {
                let seq = u32::try_from(i + 1).unwrap_or(u32::MAX);
                inner.extend_from_slice(&encode_json_frame(seq, p));
            }
            let outer = encode_compressed(3, &inner).unwrap();
            let mut d = FrameDecoder::new();
            d.feed(&outer);
            let Some(Frame::Compressed { decompressed }) = d.next_frame().unwrap() else {
                panic!()
            };
            prop_assert_eq!(decompressed, inner);
        }

        /// Decoder must never panic on arbitrary fed bytes.
        #[test]
        fn prop_decoder_does_not_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let mut d = FrameDecoder::with_max_frame_payload(8 * 1024);
            d.feed(&bytes);
            // Drain until decoder returns None or an error — never panic.
            for _ in 0..1024 {
                match d.next_frame() {
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => break,
                }
            }
        }
    }
}
