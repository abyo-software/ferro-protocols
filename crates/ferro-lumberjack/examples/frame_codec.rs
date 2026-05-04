// SPDX-License-Identifier: Apache-2.0
//! Pure frame-codec usage — no Tokio, no I/O, no async runtime.
//!
//! Demonstrates the round-trip a server-side consumer (or a
//! sync-runtime sender) would perform: encode a window of two JSON
//! events with a compressed inner batch, then decode the resulting
//! wire bytes byte-for-byte and surface the embedded events.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example frame_codec --no-default-features
//! ```
//!
//! The `--no-default-features` flag proves the codec is independently
//! useful: nothing here pulls in Tokio, rustls, or any of the optional
//! features.

use ferro_lumberjack::frame::{
    Frame, FrameDecoder, encode_compressed, encode_json_frame, encode_window,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ----- Sender side ------------------------------------------------------

    let events: [&[u8]; 2] = [br#"{"msg":"hello"}"#, br#"{"msg":"world"}"#];

    let mut inner = Vec::new();
    for (i, payload) in events.iter().enumerate() {
        let seq = u32::try_from(i + 1)?;
        inner.extend_from_slice(&encode_json_frame(seq, payload));
    }

    let mut wire = Vec::new();
    wire.extend_from_slice(&encode_window(u32::try_from(events.len())?));
    let compressed = encode_compressed(6, &inner)?;
    wire.extend_from_slice(&compressed);

    println!(
        "ferro-lumberjack: encoded window of {} events into {} wire bytes ({} compressed)",
        events.len(),
        wire.len(),
        compressed.len()
    );

    // ----- Receiver side ----------------------------------------------------

    let mut outer = FrameDecoder::new();
    outer.feed(&wire);

    while let Some(frame) = outer.next_frame()? {
        match frame {
            Frame::Window { count } => {
                println!("  Window: {count} events incoming");
            }
            Frame::Compressed { decompressed } => {
                println!(
                    "  Compressed batch: {} decompressed bytes — descending into inner stream",
                    decompressed.len()
                );
                let mut inner_dec = FrameDecoder::new();
                inner_dec.feed(&decompressed);
                while let Some(inner_frame) = inner_dec.next_frame()? {
                    if let Frame::Json { seq, payload } = inner_frame {
                        let s = std::str::from_utf8(&payload).unwrap_or("<non-utf8>");
                        println!("    seq={seq} payload={s}");
                    }
                }
            }
            other => {
                println!("  unexpected outer frame: {other:?}");
            }
        }
    }

    Ok(())
}
