// SPDX-License-Identifier: Apache-2.0
#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

mod error;
pub mod frame;
mod sequence;

#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub mod client;

#[cfg(feature = "server")]
#[cfg_attr(docsrs, doc(cfg(feature = "server")))]
pub mod server;

#[cfg(feature = "tls")]
#[cfg_attr(docsrs, doc(cfg(feature = "tls")))]
pub mod tls;

pub use error::{FrameError, ProtocolError};
pub use frame::{
    Frame, FrameDecoder, FrameType, encode_ack, encode_compressed, encode_json_frame, encode_window,
};
pub use sequence::Sequence;

/// Lumberjack v2 protocol version byte (`b'2'`).
pub const PROTOCOL_VERSION: u8 = b'2';

/// Default maximum decoded frame payload size (64 MiB).
///
/// Caps both raw frame payloads and the *decompressed* size of `C` frames,
/// to make zlib-bomb attacks O(memory-bounded) instead of unbounded. Used
/// by [`FrameDecoder::new`].
pub const DEFAULT_MAX_FRAME_PAYLOAD: usize = 64 * 1024 * 1024;
