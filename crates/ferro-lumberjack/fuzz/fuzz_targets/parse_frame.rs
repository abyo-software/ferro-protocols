// SPDX-License-Identifier: Apache-2.0
//! Fuzz target: feed arbitrary bytes into `FrameDecoder` and confirm it
//! never panics. Drains decoded frames until the decoder returns
//! `Ok(None)` or an error.

#![no_main]

use ferro_lumberjack::frame::FrameDecoder;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // 1 MiB cap so the fuzzer can't OOM by feeding a 4 GiB length prefix.
    let mut d = FrameDecoder::with_max_frame_payload(1024 * 1024);
    d.feed(data);
    // Bound the loop; libFuzzer bails out on extremely long-running
    // inputs anyway, but defence-in-depth here.
    for _ in 0..1024 {
        match d.next_frame() {
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => break,
        }
    }
});
