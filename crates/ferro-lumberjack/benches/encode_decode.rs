// SPDX-License-Identifier: Apache-2.0
//! Criterion benchmarks for the `frame` codec hot paths.

#![allow(missing_docs)] // criterion_group! generates undocumented items

use criterion::{Criterion, criterion_group, criterion_main};
use ferro_lumberjack::frame::{FrameDecoder, encode_compressed, encode_json_frame, encode_window};
use std::hint::black_box;

fn bench_encode_json(c: &mut Criterion) {
    let payload = vec![b'x'; 256];
    c.bench_function("encode_json_frame/256B", |b| {
        b.iter(|| {
            let bytes = encode_json_frame(black_box(1), black_box(&payload));
            black_box(bytes);
        });
    });

    let payload_4k = vec![b'x'; 4096];
    c.bench_function("encode_json_frame/4KB", |b| {
        b.iter(|| {
            let bytes = encode_json_frame(black_box(1), black_box(&payload_4k));
            black_box(bytes);
        });
    });
}

fn bench_decode_window_batch(c: &mut Criterion) {
    let mut wire = Vec::new();
    wire.extend_from_slice(&encode_window(100));
    for i in 0..100 {
        wire.extend_from_slice(&encode_json_frame(i + 1, b"{\"k\":1}"));
    }

    c.bench_function("decode_window+100json", |b| {
        b.iter(|| {
            let mut d = FrameDecoder::new();
            d.feed(black_box(&wire));
            while let Some(frame) = d.next_frame().unwrap() {
                black_box(frame);
            }
        });
    });
}

fn bench_compress(c: &mut Criterion) {
    let mut inner = Vec::new();
    for i in 0..200 {
        inner.extend_from_slice(&encode_json_frame(i + 1, b"{\"a\":1,\"b\":2}"));
    }

    let mut group = c.benchmark_group("encode_compressed");
    for level in [1u32, 3, 6, 9] {
        group.bench_function(format!("level{level}"), |b| {
            b.iter(|| {
                let bytes = encode_compressed(black_box(level), black_box(&inner)).unwrap();
                black_box(bytes);
            });
        });
    }
    group.finish();
}

fn bench_decompress(c: &mut Criterion) {
    let mut inner = Vec::new();
    for i in 0..200 {
        inner.extend_from_slice(&encode_json_frame(i + 1, b"{\"a\":1,\"b\":2}"));
    }
    let outer = encode_compressed(6, &inner).unwrap();

    c.bench_function("decode_compressed", |b| {
        b.iter(|| {
            let mut d = FrameDecoder::new();
            d.feed(black_box(&outer));
            let frame = d.next_frame().unwrap().unwrap();
            black_box(frame);
        });
    });
}

criterion_group!(
    benches,
    bench_encode_json,
    bench_decode_window_batch,
    bench_compress,
    bench_decompress,
);
criterion_main!(benches);
