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
