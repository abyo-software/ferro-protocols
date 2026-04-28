<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-lumberjack

All notable changes to this crate are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This crate moved out of the `v0.0.x` alpha series at `0.1.0`. From
`0.1.0` onward we apply
[Semantic Versioning](https://semver.org/) — minor releases may
deprecate but not remove APIs; only major bumps remove or rename
public items.

## [Unreleased]

## [0.1.0] — 2026-04-25

First release with both client and server primitives. Suitable for
production smoke-testing as a Logstash-compatible receiver in Rust;
the public API is stable for the `0.1.x` series.

### Added
- **`server` module** (default feature `server`):
  - `Server::builder() → ServerBuilder` for fluent configuration.
  - `Listener::accept() → ServerConnection` with optional TLS
    handshake performed up-front.
  - `ServerConnection::read_window()` — pulls a complete window of
    data events off the wire, transparently descending into
    compressed (`C`) batches and surfacing legacy (`D`) frames as
    "consumed slots". Handles split socket reads, EOF before/after
    the window header, and oversized frames (zlib bombs included).
  - `ServerConnection::send_ack(seq)` — explicit ACK, for callers
    that want strict ack-after-fsync semantics or partial ACKs.
  - `ServerConnection::read_and_ack()` — convenience for the common
    "ack the whole window after reading it" path.
- **`tls::ServerTlsConfig`** + builder for server-side TLS using
  rustls 0.23 / tokio-rustls 0.26. Loads cert chain and private key
  from PEM files or in-memory bytes.
- 13 server-side unit tests, 6 client+server end-to-end tests
  (covering uncompressed / compressed / consecutive windows / TLS /
  insecure-mode TLS), and an `examples/echo_server.rs` for
  interactive smoke testing.

### Changed
- TLS feature is now decoupled from the client feature. `tls` may be
  enabled with `server` only, `client` only, or both. `default`
  enables all three (`client + server + tls`).

### Notes
- The `client` API surface from `0.0.1` is unchanged.
- Sequence-number wrap-around handling matches the sender side
  documented in `0.0.1`; the server does not enforce monotonicity
  across windows (it uses whatever the sender supplies).

## [0.0.1] — 2026-04-25 (alpha)

Initial extraction from `ferro-beat` / `ferro-heartbeat` into a
standalone crate.

### Added
- `frame` module: pure-data encoders (`encode_window`,
  `encode_json_frame`, `encode_compressed`, `encode_ack`) and a
  streaming `FrameDecoder` that emits typed `Frame` values from a
  byte stream. No I/O dependencies — usable from any async runtime
  or sync code.
- `Sequence` type with wrapping `u32` arithmetic and an
  `is_acked_by` helper that correctly handles wrap-around when a
  long-lived connection emits more than `u32::MAX` events.
- `client` module (default feature `client`): async Tokio client
  with a fluent `ClientBuilder`, multi-host load balancing, optional
  zlib compression, and ACK validation.
- `tls` module (default feature `tls`): rustls 0.23 / tokio-rustls
  0.26 integration. Custom CA bundles via PEM files;
  `webpki-roots` fallback; explicit-opt-in `dangerous_disable_verification`
  for self-signed dev environments.
- One fuzz target (`parse_frame`) covering the streaming decoder.
- `proptest`-driven property tests for round-trip encoding and
  wrap-around arithmetic.
- Criterion benchmark for encode / decode hot paths.

### Known gaps (closed in `0.1.0`)
- ~~No server-side `Listener`~~ — added in `0.1.0`.
- Legacy `D` (key-value data) frames are decoded as `Frame::Unknown`;
  encoding is not supported. Modern Beats use only `J` frames.
- No `BatchSink` trait abstraction; clients must call `send_json`
  with explicit `Vec<Vec<u8>>` batches.

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-lumberjack-v0.1.0...HEAD
[0.1.0]: https://github.com/abyo-software/ferro-protocols/compare/ferro-lumberjack-v0.0.1...ferro-lumberjack-v0.1.0
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-lumberjack-v0.0.1
