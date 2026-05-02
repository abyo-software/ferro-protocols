// SPDX-License-Identifier: Apache-2.0
//! Focused fuzz target for the Lumberjack v2 (Beats) frame decoder.
//!
//! `FrameDecoder::next_frame` is the network-exposed wire-protocol
//! entry point on the Logstash-compatible server side. It parses a
//! stream of byte-typed frames (`W`indow / `A`ck / `J`son /
//! `C`ompressed / legacy `D`ata), each carrying a length-prefixed
//! payload. The compressed-frame path also runs `flate2::Decompress`
//! on attacker-controlled bytes, capped to `DEFAULT_MAX_FRAME_PAYLOAD`
//! (64 MiB) per
//! `crates/ferro-lumberjack/src/lib.rs:36` to defend against zlib
//! bombs.
//!
//! Watching for: panics on malformed length prefixes, OOM from huge
//! declared sizes, integer-underflow on count/length arithmetic, and
//! unbounded loops on the `try_decode_*` paths. The decoder buffers
//! input via `feed()` so we exercise both partial and complete frames
//! by chunking the fuzz input.

#![no_main]

use libfuzzer_sys::fuzz_target;

use ferro_lumberjack::frame::FrameDecoder;

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    // The first byte selects how the input is chunked into the
    // streaming decoder so the corpus exercises both single-shot and
    // multi-feed framing. A chunk size of 0 falls back to single-shot.
    let chunk = data[0] as usize;
    let payload = &data[1..];
    let mut dec = FrameDecoder::new();

    if chunk == 0 || chunk >= payload.len() {
        dec.feed(payload);
    } else {
        for slice in payload.chunks(chunk) {
            dec.feed(slice);
            // Drain whatever frames are currently parseable between
            // feeds, so the per-feed `try_decode_*` paths see partial
            // states the way a real socket would deliver them.
            loop {
                match dec.next_frame() {
                    Ok(Some(_frame)) => continue,
                    Ok(None) => break,
                    Err(_) => return,
                }
            }
        }
    }

    // Final drain pass.
    loop {
        match dec.next_frame() {
            Ok(Some(_frame)) => continue,
            Ok(None) => break,
            Err(_) => return,
        }
    }
});
