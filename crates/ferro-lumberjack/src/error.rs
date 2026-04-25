// SPDX-License-Identifier: Apache-2.0
//! Error types.
//!
//! Two distinct error families:
//!
//! - [`FrameError`] — pure codec errors (no I/O). Returned by the
//!   [`crate::frame::FrameDecoder`] and the encoder helpers.
//! - [`ProtocolError`] — higher-level errors emitted by the
//!   client (transport, TLS, ACK validation). Wraps `FrameError`
//!   and `std::io::Error`.

use thiserror::Error;

/// Errors that can occur while encoding or decoding a Lumberjack v2 frame.
///
/// All variants are non-exhaustive in spirit: future versions of the codec
/// may add new variants without bumping the major version while in the
/// `v0.0.x` series.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FrameError {
    /// Wire byte 0 is not `b'2'`. The decoder refuses to advance because
    /// it cannot determine which protocol version applies.
    #[error("unsupported lumberjack version byte: {0:#x} (expected 0x32 / '2')")]
    UnsupportedVersion(u8),

    /// Wire byte 1 is not a known frame type.
    #[error("unknown lumberjack frame type: {0:#x}")]
    UnknownFrameType(u8),

    /// A length field declared a frame larger than
    /// [`crate::DEFAULT_MAX_FRAME_PAYLOAD`] (or the configured per-decoder
    /// limit). Surfaces resource-exhaustion attacks before the decoder
    /// allocates.
    #[error("frame payload length {requested} exceeds configured limit of {limit} bytes")]
    PayloadTooLarge {
        /// Length declared on the wire.
        requested: usize,
        /// Configured cap for this decoder.
        limit: usize,
    },

    /// Decompression failed (`C` frame had invalid zlib content).
    #[error("zlib decompression failed: {0}")]
    Decompression(String),

    /// The decompressed contents of a `C` frame would exceed the
    /// per-decoder size limit. Caps zlib-bomb input.
    #[error("decompressed payload would exceed limit of {limit} bytes")]
    DecompressedTooLarge {
        /// Configured cap for this decoder.
        limit: usize,
    },

    /// The compressor returned an error while encoding (typically out of
    /// memory). Returned only by [`crate::encode_compressed`].
    #[error("zlib compression failed: {0}")]
    Compression(String),
}

/// Errors emitted by the high-level client.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// I/O error while reading from or writing to the network.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Codec error (frame parse / encode failure).
    #[error("codec: {0}")]
    Codec(#[from] FrameError),

    /// Operation timed out (connect, write, or ACK read).
    #[error("operation timed out: {0}")]
    Timeout(&'static str),

    /// The receiver's ACK had an unexpected version or frame type, or its
    /// sequence number didn't match what the client just sent.
    #[error(
        "unexpected ack: version=0x{version:02x} type=0x{frame_type:02x} seq={acked_seq} (expected_last_seq={expected_seq})"
    )]
    UnexpectedAck {
        /// Wire-byte 0.
        version: u8,
        /// Wire-byte 1.
        frame_type: u8,
        /// Sequence number on the ACK frame.
        acked_seq: u32,
        /// Sequence number this client expected the ACK to reference.
        expected_seq: u32,
    },

    /// The receiver acknowledged fewer events than were sent (partial ACK).
    /// Surfaces so the caller can decide whether to retry the unacked tail.
    #[error("partial ack: {acked} of {sent} events acknowledged")]
    PartialAck {
        /// Number of events the receiver acknowledged.
        acked: u32,
        /// Number of events the client sent in this window.
        sent: u32,
    },

    /// No hosts were configured for the client.
    #[error("no hosts configured")]
    NoHostsConfigured,

    /// All configured hosts have been tried and all failed; the wrapped
    /// error is the most recent failure.
    #[error("all configured hosts failed; last error: {0}")]
    AllHostsFailed(Box<Self>),

    /// TLS configuration error (key/cert load, server-name parse, …).
    #[cfg(feature = "tls")]
    #[error("tls config: {0}")]
    Tls(String),
}
