<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-lumberjack

[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](../../LICENSE)
[![crates.io](https://img.shields.io/crates/v/ferro-lumberjack.svg)](https://crates.io/crates/ferro-lumberjack)
[![docs.rs](https://docs.rs/ferro-lumberjack/badge.svg)](https://docs.rs/ferro-lumberjack)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](../../rust-toolchain.toml)

Rust primitives for the **Logstash Lumberjack v2** wire protocol — the
framing protocol Filebeat, Heartbeat, and other Elastic Beats agents
use to ship events to Logstash (and to other Beats-protocol-compatible
ingestion endpoints).

> The Lumberjack v2 protocol is the most widely-deployed log-shipping
> wire format in production today, and the conversation around the
> licensing of the official Beats agents has made portable, permissively-licensed
> reimplementations more interesting than they used to be. Until now,
> there has been no Rust crate implementing either side of the wire.
> This crate is both — a frame codec, an async client, and an async
> server, with TLS in both directions via rustls. Extracted from
> production use in [`ferro-beat`] (Filebeat-compatible log shipper)
> and [`ferro-heartbeat`] (Heartbeat-compatible monitor).

> **Beta (`v0.1.0`).** Both client and server primitives are
> available. The public API is stable for the `0.1.x` series — minor
> releases may deprecate but not remove APIs. See [Status](#status).

Part of the **Ferro ecosystem**.

[`ferro-beat`]: https://github.com/abyo-software/ferro-beat
[`ferro-heartbeat`]: https://github.com/abyo-software/ferro-heartbeat

## What this crate provides

- **Frame codec** (`frame` module) — encoders for Window (`W`),
  JSON-payload (`J`), Compressed (`C`), and Ack (`A`) frames; a
  streaming decoder that consumes bytes incrementally and emits typed
  frames. Pure data, usable from any runtime.
- **Async client** (`client` module, default feature `client`) —
  builds a connection to a Logstash endpoint, sends batches as
  Lumberjack v2 windows, parses the ACK response, validates the
  acknowledged sequence number, and surfaces partial-ACK /
  sequence-mismatch errors. Supports load-balanced multi-host
  failover and a persistent monotonic sequence counter that handles
  `u32::MAX` wrap-around correctly.
- **Async server** (`server` module, default feature `server`) —
  binds a TCP listener, accepts inbound connections, and exposes
  `ServerConnection::read_window` for pulling decoded windows of
  events. The caller controls when to ACK, allowing strict
  ack-after-fsync, partial ACKs, or fire-and-forget. Compressed
  windows are decoded transparently; legacy `D` frames are surfaced
  as "consumed slots" without a payload.
- **TLS** (default feature `tls`, both directions) —
  `tokio-rustls` based. Client side: custom CA bundles via
  `rustls-pemfile` or `webpki-roots` fallback, plus an explicitly
  opt-in `dangerous_disable_verification` mode. Server side:
  cert chain + private-key PEM loading via `ServerTlsConfig`.

## What this crate does **not** provide (yet)

- **Field-by-field `D` (data) frames.** Modern Beats use the JSON
  `J` frame exclusively; the legacy `D` (key-value) frame is not
  encoded here. Decoding is supported (skipped frames are surfaced
  as `Frame::Unknown`) so a server-side path can choose to handle
  them.
- **A runtime-agnostic API.** This crate is Tokio-only on purpose;
  if you need a different runtime, the `frame` codec is runtime-free
  and you can drive your own I/O.
- **Built-in event de-duplication or persistence.** The server
  surfaces decoded events; durability is the caller's concern.

## Specification compliance

The protocol is most clearly described in the
[`elastic/go-lumber`](https://github.com/elastic/go-lumber) reference
implementation; there is no formal RFC. The frame layout we implement:

| Frame  | Bytes | Meaning |
|---|---|---|
| `2 W <u32 count>` | 6  | Window — number of `J`/`D` data frames the sender will transmit before expecting an ACK. |
| `2 J <u32 seq> <u32 len> <payload>` | 10 + len | JSON-encoded event with monotonic sequence number. |
| `2 C <u32 len> <zlib bytes>` | 6 + len | Compressed batch — payload is a zlib stream containing concatenated `J`/`D` frames. |
| `2 A <u32 seq>` | 6  | ACK from receiver — `seq` is the highest sequence number successfully processed. |

Sequence numbers are `u32` and wrap modulo `2^32`. Compare with
wrapping subtraction (`acked.wrapping_sub(expected) == 0`) — see
`Sequence::is_acked_by`. This is the only correct way to handle
long-lived connections that send more than `u32::MAX` events.

## Status

| Aspect | Status |
|---|---|
| API stability | **beta** (`v0.1.x` — semver applies) |
| Client | working, used in production by `ferro-beat` / `ferro-heartbeat` |
| Server | working, exercised by 6 client↔server end-to-end tests |
| TLS | rustls 0.23 + tokio-rustls 0.26; no openssl, both directions |
| MSRV | rustc **1.88** |
| Fuzz harness | `parse_frame` (decoder) — covered nightly |
| Coverage target | 80%+ line; current measured in CI |
| Async runtime | Tokio (no other runtime supported) |

## Quick start (client)

```rust,no_run
use ferro_lumberjack::client::ClientBuilder;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let mut client = ClientBuilder::new()
    .add_host("logstash-1.internal:5044")
    .add_host("logstash-2.internal:5044")
    .load_balance(true)
    .timeout(std::time::Duration::from_secs(30))
    .compression_level(3)
    .connect()
    .await?;

let events: Vec<Vec<u8>> = vec![
    br#"{"message":"hello","level":"info"}"#.to_vec(),
    br#"{"message":"world","level":"info"}"#.to_vec(),
];
let acked = client.send_json(events).await?;
assert_eq!(acked, 2);
# Ok(()) }
```

With TLS:

```rust,no_run
use ferro_lumberjack::client::ClientBuilder;
use ferro_lumberjack::tls::TlsConfig;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let tls = TlsConfig::builder()
    .add_ca_pem_file("/etc/ferro/ca.pem")?
    .build()?;

let mut client = ClientBuilder::new()
    .add_host("logstash.internal:5044")
    .tls(tls)
    .connect()
    .await?;
# Ok(()) }
```

## Quick start (server)

```rust,no_run
use ferro_lumberjack::server::Server;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let listener = Server::builder()
    .bind("127.0.0.1:5044")
    .await?;

loop {
    let mut conn = listener.accept().await?;
    tokio::spawn(async move {
        while let Some(window) = conn.read_window().await? {
            for event in &window.events {
                println!("seq={} {} bytes", event.seq, event.payload.len());
            }
            conn.send_ack(window.last_seq).await?;
        }
        Ok::<_, ferro_lumberjack::ProtocolError>(())
    });
}
# }
```

There is also a runnable echo server in `examples/echo_server.rs`:

```bash
cargo run --example echo_server -- 127.0.0.1:5044
```

## Frame codec (no I/O — usable from any runtime)

```rust
use ferro_lumberjack::frame::{FrameDecoder, encode_window, encode_json_frame};

let mut decoder = FrameDecoder::new();
decoder.feed(&encode_window(2));
decoder.feed(&encode_json_frame(1, br#"{"a":1}"#));
decoder.feed(&encode_json_frame(2, br#"{"b":2}"#));

while let Some(frame) = decoder.next_frame()? {
    println!("got {frame:?}");
}
# Ok::<_, ferro_lumberjack::FrameError>(())
```

## Used in production by

- [**FerroBeat**](https://github.com/abyo-software/ferro-beat) — Filebeat-compatible
  Rust log shipper. (Source crate; will switch to `ferro-lumberjack`
  once published.)
- [**FerroHeartbeat**](https://github.com/abyo-software/ferro-heartbeat) — Heartbeat-compatible
  monitor. (Source crate; will switch once published.)

## Triage policy

See [the workspace `CONTRIBUTING.md`](../../CONTRIBUTING.md). In
short: security 48h, bugs (with a reproducer) 14 days best-effort,
features collected for the next minor.

## Trademarks

Logstash® and Elastic® are registered trademarks of Elasticsearch
B.V. This crate implements a wire protocol that is compatible with
those products; it is not endorsed by, or affiliated with, Elastic.

## License

Apache-2.0. See [`LICENSE`](../../LICENSE).
