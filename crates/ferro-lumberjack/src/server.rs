// SPDX-License-Identifier: Apache-2.0
//! Async Lumberjack v2 server (receiver side).
//!
//! Build with [`Server::builder`]; the resulting [`Listener`] accepts
//! connections from Beats-style senders and exposes
//! [`ServerConnection::read_window`] for pulling decoded windows of
//! events off the wire. The caller is responsible for sending an ACK
//! ([`ServerConnection::send_ack`]) once it has durably processed the
//! window — this allows the server to implement strict
//! "ack-after-fsync" semantics, partial acks, or fire-and-forget at the
//! caller's discretion.
//!
//! ### Example
//!
//! ```no_run
//! use ferro_lumberjack::server::Server;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let listener = Server::builder()
//!     .bind("127.0.0.1:5044")
//!     .await?;
//!
//! loop {
//!     let mut conn = listener.accept().await?;
//!     tokio::spawn(async move {
//!         while let Some(window) = conn.read_window().await? {
//!             for event in &window.events {
//!                 println!("seq={} payload_bytes={}", event.seq, event.payload.len());
//!             }
//!             conn.send_ack(window.last_seq).await?;
//!         }
//!         Ok::<_, ferro_lumberjack::ProtocolError>(())
//!     });
//! }
//! # }
//! ```

use std::io;
use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};

use crate::frame::{Frame, FrameDecoder, encode_ack};
use crate::{
    DEFAULT_MAX_FRAME_PAYLOAD, DEFAULT_MAX_WINDOW_BYTES, DEFAULT_MAX_WINDOW_EVENTS, FrameError,
    ProtocolError,
};

#[cfg(feature = "tls")]
use crate::tls::ServerTlsConfig;

/// Default per-connection socket-read buffer size.
const READ_CHUNK: usize = 8 * 1024;

/// Entry point — call [`Server::builder`].
#[derive(Debug, Default)]
pub struct Server;

impl Server {
    /// Begin building a listener.
    #[must_use]
    pub fn builder() -> ServerBuilder {
        ServerBuilder::default()
    }
}

/// Builder for a [`Listener`].
#[derive(Debug, Default)]
pub struct ServerBuilder {
    max_frame_payload: Option<usize>,
    max_window_events: Option<usize>,
    max_window_bytes: Option<usize>,
    #[cfg(feature = "tls")]
    tls: Option<ServerTlsConfig>,
}

impl ServerBuilder {
    /// Cap the size of any single decoded frame payload (and the
    /// decompressed inner of `C` frames). Default is
    /// [`crate::DEFAULT_MAX_FRAME_PAYLOAD`] (64 MiB).
    #[must_use]
    pub const fn max_frame_payload(mut self, n: usize) -> Self {
        self.max_frame_payload = Some(n);
        self
    }

    /// Cap the number of data events the server will accumulate for a
    /// single window. The window's declared `count` is peer-supplied, so
    /// without this aggregate cap a peer could declare a huge count and
    /// stream many small frames, forcing the receiver's per-window buffer
    /// to grow unboundedly. A window whose declared count — or whose
    /// observed event count mid-stream — exceeds `n` is rejected with
    /// [`ProtocolError::WindowTooLarge`] and reading stops immediately.
    ///
    /// Default is [`crate::DEFAULT_MAX_WINDOW_EVENTS`] (100 000). Pass
    /// `usize::MAX` to disable (not recommended for untrusted peers).
    #[must_use]
    pub const fn max_window_events(mut self, n: usize) -> Self {
        self.max_window_events = Some(n);
        self
    }

    /// Cap the total accumulated payload bytes across all events in a
    /// single window. Complements [`Self::max_window_events`]: once the
    /// summed per-event payload bytes exceed `n`, the window is rejected
    /// with [`ProtocolError::WindowTooLarge`] and reading stops
    /// immediately (no further events are accumulated).
    ///
    /// Default is [`crate::DEFAULT_MAX_WINDOW_BYTES`] (256 MiB). Pass
    /// `usize::MAX` to disable (not recommended for untrusted peers).
    #[must_use]
    pub const fn max_window_bytes(mut self, n: usize) -> Self {
        self.max_window_bytes = Some(n);
        self
    }

    /// Enable TLS for accepted connections.
    #[cfg(feature = "tls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls")))]
    #[must_use]
    pub fn tls(mut self, cfg: ServerTlsConfig) -> Self {
        self.tls = Some(cfg);
        self
    }

    /// Bind to `addr` and return a ready-to-accept [`Listener`].
    pub async fn bind(self, addr: impl ToSocketAddrs) -> io::Result<Listener> {
        let inner = TcpListener::bind(addr).await?;
        Ok(Listener {
            inner,
            max_frame_payload: self.max_frame_payload.unwrap_or(DEFAULT_MAX_FRAME_PAYLOAD),
            max_window_events: self.max_window_events.unwrap_or(DEFAULT_MAX_WINDOW_EVENTS),
            max_window_bytes: self.max_window_bytes.unwrap_or(DEFAULT_MAX_WINDOW_BYTES),
            #[cfg(feature = "tls")]
            tls: self.tls,
        })
    }
}

/// A bound listener that produces [`ServerConnection`] values via
/// [`Listener::accept`].
#[derive(Debug)]
pub struct Listener {
    inner: TcpListener,
    max_frame_payload: usize,
    max_window_events: usize,
    max_window_bytes: usize,
    #[cfg(feature = "tls")]
    tls: Option<ServerTlsConfig>,
}

impl Listener {
    /// The local address this listener is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// Accept the next inbound connection. Performs the TLS handshake
    /// up-front if the listener was built with TLS.
    pub async fn accept(&self) -> Result<ServerConnection, ProtocolError> {
        let (sock, peer) = self.inner.accept().await.map_err(ProtocolError::Io)?;

        #[cfg(feature = "tls")]
        if let Some(ref tls) = self.tls {
            let acceptor = tokio_rustls::TlsAcceptor::from(tls.inner());
            let tls_stream = acceptor.accept(sock).await.map_err(ProtocolError::Io)?;
            return Ok(ServerConnection {
                conn: Conn::Tls(Box::new(tls_stream)),
                decoder: FrameDecoder::with_max_frame_payload(self.max_frame_payload),
                max_frame_payload: self.max_frame_payload,
                max_window_events: self.max_window_events,
                max_window_bytes: self.max_window_bytes,
                peer,
            });
        }

        Ok(ServerConnection {
            conn: Conn::Plain(sock),
            decoder: FrameDecoder::with_max_frame_payload(self.max_frame_payload),
            max_frame_payload: self.max_frame_payload,
            max_window_events: self.max_window_events,
            max_window_bytes: self.max_window_bytes,
            peer,
        })
    }
}

/// Single accepted connection.
#[derive(Debug)]
pub struct ServerConnection {
    conn: Conn,
    decoder: FrameDecoder,
    max_frame_payload: usize,
    max_window_events: usize,
    max_window_bytes: usize,
    peer: SocketAddr,
}

#[derive(Debug)]
enum Conn {
    Plain(TcpStream),
    #[cfg(feature = "tls")]
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl Conn {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf).await,
            #[cfg(feature = "tls")]
            Self::Tls(s) => s.read(buf).await,
        }
    }
    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.write_all(buf).await,
            #[cfg(feature = "tls")]
            Self::Tls(s) => s.write_all(buf).await,
        }
    }
    async fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.flush().await,
            #[cfg(feature = "tls")]
            Self::Tls(s) => s.flush().await,
        }
    }
}

/// One JSON-decoded data event from a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonEvent {
    /// Sequence number assigned by the sender.
    pub seq: u32,
    /// Raw JSON payload bytes (UTF-8 expected but not validated here).
    pub payload: Vec<u8>,
}

/// A complete window of events read from the wire.
///
/// `last_seq` is the sequence number that the receiver should ACK to
/// declare a full ACK. For partial-ACK (durability-after-N-events)
/// semantics, the caller may instead send an ACK referencing any seq
/// in `events.iter().map(|e| e.seq)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// Decoded data events in receive order.
    pub events: Vec<JsonEvent>,
    /// Highest seq observed in the window; the natural target for a
    /// full ACK.
    pub last_seq: u32,
}

impl ServerConnection {
    /// Peer socket address.
    #[must_use]
    pub const fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Read the next complete window of events.
    ///
    /// Returns `Ok(None)` on a clean EOF *before* a Window frame has
    /// been seen — indicates the peer closed the connection between
    /// windows. EOF *during* a window (after the Window header but
    /// before all data frames have arrived) is surfaced as
    /// `Err(ProtocolError::Io(UnexpectedEof))`.
    pub async fn read_window(&mut self) -> Result<Option<Window>, ProtocolError> {
        let mut events: Vec<JsonEvent> = Vec::new();
        let mut window_remaining: Option<u32> = None;
        let mut last_seq: u32 = 0;
        let mut accumulated_bytes: usize = 0;

        loop {
            // 1) Drain anything currently buffered.
            loop {
                let frame = self.decoder.next_frame()?;
                let Some(frame) = frame else { break };
                match frame {
                    Frame::Window { count } => {
                        if window_remaining.is_some() {
                            return Err(ProtocolError::Codec(FrameError::UnknownFrameType(b'W')));
                        }
                        // Reject an over-large declared window up front so we
                        // never begin accumulating events for it.
                        Self::check_declared_window(count, self.max_window_events)?;
                        if count == 0 {
                            // Empty window — return immediately so the caller
                            // can decide whether to ACK seq=0 or skip.
                            return Ok(Some(Window {
                                events,
                                last_seq: 0,
                            }));
                        }
                        window_remaining = Some(count);
                    }
                    Frame::Json { seq, payload } => {
                        Self::record_event(
                            &mut events,
                            &mut window_remaining,
                            &mut last_seq,
                            &mut accumulated_bytes,
                            self.max_window_events,
                            self.max_window_bytes,
                            seq,
                            payload,
                        )?;
                        if window_remaining == Some(0) {
                            return Ok(Some(Window { events, last_seq }));
                        }
                    }
                    Frame::Compressed { decompressed } => {
                        // Recurse into the inner stream.
                        let mut inner =
                            FrameDecoder::with_max_frame_payload(self.max_frame_payload);
                        inner.feed(&decompressed);
                        while let Some(f) = inner.next_frame()? {
                            match f {
                                Frame::Json { seq, payload } => {
                                    Self::record_event(
                                        &mut events,
                                        &mut window_remaining,
                                        &mut last_seq,
                                        &mut accumulated_bytes,
                                        self.max_window_events,
                                        self.max_window_bytes,
                                        seq,
                                        payload,
                                    )?;
                                }
                                Frame::Unknown { .. } => {
                                    // Legacy D frame inside compressed batch — skip
                                    // payload but consume one slot.
                                    Self::consume_slot(&mut window_remaining)?;
                                }
                                Frame::Window { .. }
                                | Frame::Compressed { .. }
                                | Frame::Ack { .. } => {
                                    return Err(ProtocolError::Codec(
                                        FrameError::UnknownFrameType(0),
                                    ));
                                }
                            }
                        }
                        if window_remaining == Some(0) {
                            return Ok(Some(Window { events, last_seq }));
                        }
                    }
                    Frame::Unknown { .. } => {
                        // Legacy D frame — consume one window slot but no payload.
                        Self::consume_slot(&mut window_remaining)?;
                        if window_remaining == Some(0) {
                            return Ok(Some(Window { events, last_seq }));
                        }
                    }
                    Frame::Ack { .. } => {
                        // ACK frames are never sent to a server.
                        return Err(ProtocolError::Codec(FrameError::UnknownFrameType(b'A')));
                    }
                }
            }

            // 2) Need more bytes.
            let mut buf = [0u8; READ_CHUNK];
            let n = self.conn.read(&mut buf).await.map_err(ProtocolError::Io)?;
            if n == 0 {
                // Clean EOF.
                return if window_remaining.is_none() && events.is_empty() {
                    Ok(None)
                } else {
                    Err(ProtocolError::Io(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "lumberjack: peer closed connection mid-window",
                    )))
                };
            }
            self.decoder.feed(&buf[..n]);
        }
    }

    /// Reject a window whose peer-declared event `count` already exceeds
    /// the configured aggregate cap, before any data frame is read.
    const fn check_declared_window(count: u32, max_events: usize) -> Result<(), ProtocolError> {
        if count as usize > max_events {
            return Err(ProtocolError::WindowTooLarge {
                kind: "event count",
                requested: count as usize,
                limit: max_events,
            });
        }
        Ok(())
    }

    /// Record a JSON event into the in-flight window. Errors if a data
    /// frame arrives before its Window header, and decrements the
    /// remaining count.
    ///
    /// Also enforces the per-window aggregate caps mid-stream: if
    /// accumulating this event would push the observed event count past
    /// `max_events`, or the accumulated payload bytes past `max_bytes`,
    /// the event is **not** pushed and [`ProtocolError::WindowTooLarge`]
    /// is returned so the caller stops reading. (The declared `count` is
    /// also vetted up front in [`Self::read_window`]; this guards against
    /// a peer streaming past its own declaration via, e.g., nested
    /// compressed batches.)
    #[allow(clippy::too_many_arguments)]
    fn record_event(
        events: &mut Vec<JsonEvent>,
        window_remaining: &mut Option<u32>,
        last_seq: &mut u32,
        accumulated_bytes: &mut usize,
        max_events: usize,
        max_bytes: usize,
        seq: u32,
        payload: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        let Some(remaining) = window_remaining.as_mut() else {
            return Err(ProtocolError::Codec(FrameError::UnknownFrameType(b'J')));
        };
        if *remaining == 0 {
            return Err(ProtocolError::Codec(FrameError::UnknownFrameType(b'J')));
        }
        // Aggregate event-count cap: reject before pushing so `events`
        // never grows past the configured maximum.
        if events.len() >= max_events {
            return Err(ProtocolError::WindowTooLarge {
                kind: "event count",
                requested: events.len() + 1,
                limit: max_events,
            });
        }
        // Aggregate byte cap: reject before pushing so accumulated memory
        // never grows past the configured maximum.
        let next_bytes = accumulated_bytes.saturating_add(payload.len());
        if next_bytes > max_bytes {
            return Err(ProtocolError::WindowTooLarge {
                kind: "byte total",
                requested: next_bytes,
                limit: max_bytes,
            });
        }
        *remaining -= 1;
        *last_seq = seq;
        *accumulated_bytes = next_bytes;
        events.push(JsonEvent { seq, payload });
        Ok(())
    }

    /// Like [`Self::record_event`] but for legacy `D` frames where we
    /// consume a window slot without payload.
    const fn consume_slot(window_remaining: &mut Option<u32>) -> Result<(), ProtocolError> {
        let Some(remaining) = window_remaining.as_mut() else {
            return Err(ProtocolError::Codec(FrameError::UnknownFrameType(b'D')));
        };
        if *remaining == 0 {
            return Err(ProtocolError::Codec(FrameError::UnknownFrameType(b'D')));
        }
        *remaining -= 1;
        Ok(())
    }

    /// Send an ACK frame referencing `seq`. Typically called with the
    /// `last_seq` of a [`Window`] to declare a full ACK.
    pub async fn send_ack(&mut self, seq: u32) -> Result<(), ProtocolError> {
        let bytes = encode_ack(seq);
        self.conn
            .write_all(&bytes)
            .await
            .map_err(ProtocolError::Io)?;
        self.conn.flush().await.map_err(ProtocolError::Io)?;
        Ok(())
    }

    /// Convenience: read the next window and immediately send a full
    /// ACK referencing its `last_seq`. For most simple "log forwarder"
    /// servers this is the only method you need.
    pub async fn read_and_ack(&mut self) -> Result<Option<Window>, ProtocolError> {
        let Some(window) = self.read_window().await? else {
            return Ok(None);
        };
        self.send_ack(window.last_seq).await?;
        Ok(Some(window))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{encode_compressed, encode_json_frame, encode_window};
    use tokio::net::TcpStream as ClientTcp;

    /// Build a legacy `D` frame with `pair_count` zero-length KV pairs
    /// (so it consumes exactly one window slot without a JSON payload).
    fn legacy_d_frame_empty(seq: u32, pair_count: u32) -> Vec<u8> {
        let mut f = Vec::new();
        f.push(b'2');
        f.push(b'D');
        f.extend_from_slice(&seq.to_be_bytes());
        f.extend_from_slice(&pair_count.to_be_bytes());
        // `pair_count` pairs of (key_len=0, val_len=0).
        for _ in 0..pair_count {
            f.extend_from_slice(&0u32.to_be_bytes()); // key_len
            f.extend_from_slice(&0u32.to_be_bytes()); // val_len
        }
        f
    }

    #[test]
    fn read_chunk_constant_is_8_kib() {
        // server.rs:51 `8 * 1024` (`*`→`+` mutant → 8+1024 = 1032).
        assert_eq!(READ_CHUNK, 8192);
        assert_eq!(READ_CHUNK, 8 * 1024);
        assert_ne!(READ_CHUNK, 8 + 1024);
    }

    #[test]
    fn consume_slot_decrements_and_guards() {
        // server.rs:352/355/358. Drives consume_slot directly (it is a
        // pure const fn over the remaining-count Option).
        //
        // No window header yet → must Err (kills body→`Ok(())`).
        let mut none: Option<u32> = None;
        assert!(matches!(
            ServerConnection::consume_slot(&mut none),
            Err(ProtocolError::Codec(_))
        ));

        // remaining == 0 → must Err (kills `== 0`→`!= 0`, and body→Ok).
        let mut zero = Some(0u32);
        assert!(matches!(
            ServerConnection::consume_slot(&mut zero),
            Err(ProtocolError::Codec(_))
        ));
        assert_eq!(zero, Some(0), "errored path must not mutate the count");

        // remaining == 2 → Ok and decrements to 1 (kills `-=`→`+=`/`/=`:
        // `+=` → 3, `/=` → 0). Then 1 → 0, then 0 → Err.
        let mut two = Some(2u32);
        assert!(ServerConnection::consume_slot(&mut two).is_ok());
        assert_eq!(two, Some(1), "`-= 1` must yield 1 (not 3 via += , not 2 via /=)");
        assert!(ServerConnection::consume_slot(&mut two).is_ok());
        assert_eq!(two, Some(0));
        assert!(
            matches!(
                ServerConnection::consume_slot(&mut two),
                Err(ProtocolError::Codec(_))
            ),
            "consuming past zero must error",
        );
    }

    /// Spin up a Listener bound to 127.0.0.1:0 and return its addr +
    /// the listener itself.
    async fn ephemeral_listener() -> (SocketAddr, Listener) {
        let listener = Server::builder().bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        (addr, listener)
    }

    /// Like [`ephemeral_listener`] but with a custom window-event cap and
    /// window-byte cap, for the aggregate-DoS regression tests.
    async fn ephemeral_listener_capped(
        max_events: usize,
        max_bytes: usize,
    ) -> (SocketAddr, Listener) {
        let listener = Server::builder()
            .max_window_events(max_events)
            .max_window_bytes(max_bytes)
            .bind("127.0.0.1:0")
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        (addr, listener)
    }

    #[tokio::test]
    async fn reads_simple_uncompressed_window() {
        let (addr, listener) = ephemeral_listener().await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_and_ack().await.unwrap()
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        let mut wire = Vec::new();
        wire.extend_from_slice(&encode_window(2));
        wire.extend_from_slice(&encode_json_frame(1, br#"{"a":1}"#));
        wire.extend_from_slice(&encode_json_frame(2, br#"{"b":2}"#));
        client.write_all(&wire).await.unwrap();
        client.flush().await.unwrap();

        // Read the ACK.
        let mut ack = [0u8; 6];
        tokio::io::AsyncReadExt::read_exact(&mut client, &mut ack)
            .await
            .unwrap();
        assert_eq!(ack[0], b'2');
        assert_eq!(ack[1], b'A');
        assert_eq!(u32::from_be_bytes([ack[2], ack[3], ack[4], ack[5]]), 2);

        let window = server.await.unwrap().expect("window");
        assert_eq!(window.events.len(), 2);
        assert_eq!(window.events[0].seq, 1);
        assert_eq!(window.events[0].payload, br#"{"a":1}"#);
        assert_eq!(window.last_seq, 2);
    }

    #[tokio::test]
    async fn reads_compressed_window() {
        let (addr, listener) = ephemeral_listener().await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_and_ack().await.unwrap()
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        let mut inner = Vec::new();
        for i in 0..5_u32 {
            inner.extend_from_slice(&encode_json_frame(i + 1, b"{}"));
        }
        let compressed = encode_compressed(6, &inner).unwrap();

        let mut wire = Vec::new();
        wire.extend_from_slice(&encode_window(5));
        wire.extend_from_slice(&compressed);
        client.write_all(&wire).await.unwrap();
        client.flush().await.unwrap();

        // ACK seq=5
        let mut ack = [0u8; 6];
        tokio::io::AsyncReadExt::read_exact(&mut client, &mut ack)
            .await
            .unwrap();
        assert_eq!(u32::from_be_bytes([ack[2], ack[3], ack[4], ack[5]]), 5);

        let window = server.await.unwrap().expect("window");
        assert_eq!(window.events.len(), 5);
        assert_eq!(window.last_seq, 5);
    }

    #[tokio::test]
    async fn clean_eof_before_window_returns_none() {
        let (addr, listener) = ephemeral_listener().await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_window().await.unwrap()
        });

        let client = ClientTcp::connect(addr).await.unwrap();
        drop(client); // immediate close

        let result = server.await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn truncated_window_is_unexpected_eof() {
        let (addr, listener) = ephemeral_listener().await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_window().await
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        // Send a window header then close: server expects 3 frames but gets 0.
        client.write_all(&encode_window(3)).await.unwrap();
        client.flush().await.unwrap();
        drop(client);

        let result = server.await.unwrap();
        match result {
            Err(ProtocolError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {}
            other => panic!("expected UnexpectedEof, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn data_frame_before_window_is_rejected() {
        let (addr, listener) = ephemeral_listener().await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_window().await
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        // Send a JSON frame without a Window header first.
        client
            .write_all(&encode_json_frame(1, b"{}"))
            .await
            .unwrap();
        client.flush().await.unwrap();

        let result = server.await.unwrap();
        assert!(matches!(result, Err(ProtocolError::Codec(_))));
    }

    #[tokio::test]
    async fn split_window_across_socket_reads() {
        let (addr, listener) = ephemeral_listener().await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_and_ack().await.unwrap()
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        let mut wire = Vec::new();
        wire.extend_from_slice(&encode_window(3));
        for i in 0..3_u32 {
            wire.extend_from_slice(&encode_json_frame(i + 1, b"x"));
        }
        // Send byte by byte to force the decoder state machine through every
        // partial-buffer state.
        for byte in &wire {
            client.write_all(std::slice::from_ref(byte)).await.unwrap();
            client.flush().await.unwrap();
            tokio::task::yield_now().await;
        }

        let mut ack = [0u8; 6];
        tokio::io::AsyncReadExt::read_exact(&mut client, &mut ack)
            .await
            .unwrap();
        assert_eq!(u32::from_be_bytes([ack[2], ack[3], ack[4], ack[5]]), 3);

        let window = server.await.unwrap().expect("window");
        assert_eq!(window.events.len(), 3);
        assert_eq!(window.last_seq, 3);
    }

    #[tokio::test]
    async fn empty_window_returns_empty_events() {
        let (addr, listener) = ephemeral_listener().await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_window().await.unwrap()
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        client.write_all(&encode_window(0)).await.unwrap();
        client.flush().await.unwrap();
        drop(client);

        let window = server.await.unwrap().expect("window");
        assert!(window.events.is_empty());
        assert_eq!(window.last_seq, 0);
    }

    #[tokio::test]
    async fn window_of_legacy_d_frames_completes() {
        // server.rs:298 (`window_remaining == Some(0)` after consume_slot
        // in the Unknown arm). A window declaring 2 frames, satisfied by
        // two legacy `D` frames, must complete and ACK. With `==`→`!=` the
        // loop would never recognise completion → the read would hang
        // waiting for more bytes and then surface UnexpectedEof on close,
        // never producing the window.
        let (addr, listener) = ephemeral_listener().await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_and_ack().await
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        let mut wire = Vec::new();
        wire.extend_from_slice(&encode_window(2));
        wire.extend_from_slice(&legacy_d_frame_empty(1, 0));
        wire.extend_from_slice(&legacy_d_frame_empty(2, 0));
        client.write_all(&wire).await.unwrap();
        client.flush().await.unwrap();

        let mut ack = [0u8; 6];
        tokio::io::AsyncReadExt::read_exact(&mut client, &mut ack)
            .await
            .unwrap();
        assert_eq!(ack[1], b'A');
        // No JSON events were recorded but the window completed; last_seq
        // stays 0 because D frames carry no recorded seq.
        let window = server.await.unwrap().unwrap().expect("window completes");
        assert!(window.events.is_empty(), "D frames carry no JSON events");
    }

    #[tokio::test]
    async fn mixed_json_and_d_frames_consume_correct_slots() {
        // Reinforces server.rs:298 + consume_slot: a 3-frame window made
        // of [JSON, D, JSON] completes exactly when the third frame lands,
        // proving each D frame consumes precisely one slot.
        let (addr, listener) = ephemeral_listener().await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_and_ack().await
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        let mut wire = Vec::new();
        wire.extend_from_slice(&encode_window(3));
        wire.extend_from_slice(&encode_json_frame(1, b"first"));
        wire.extend_from_slice(&legacy_d_frame_empty(2, 0));
        wire.extend_from_slice(&encode_json_frame(3, b"third"));
        client.write_all(&wire).await.unwrap();
        client.flush().await.unwrap();

        let mut ack = [0u8; 6];
        tokio::io::AsyncReadExt::read_exact(&mut client, &mut ack)
            .await
            .unwrap();
        assert_eq!(u32::from_be_bytes([ack[2], ack[3], ack[4], ack[5]]), 3);
        let window = server.await.unwrap().unwrap().expect("window");
        assert_eq!(window.events.len(), 2, "two JSON events, D slot consumed");
        assert_eq!(window.events[0].seq, 1);
        assert_eq!(window.events[1].seq, 3);
        assert_eq!(window.last_seq, 3);
    }

    #[tokio::test]
    async fn d_frame_before_window_is_rejected() {
        // server.rs:352 consume_slot body → `Ok(())` would silently accept
        // a D frame with no window header. The real code returns
        // Codec(UnknownFrameType('D')).
        let (addr, listener) = ephemeral_listener().await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_window().await
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        client
            .write_all(&legacy_d_frame_empty(1, 0))
            .await
            .unwrap();
        client.flush().await.unwrap();

        let result = server.await.unwrap();
        assert!(
            matches!(result, Err(ProtocolError::Codec(_))),
            "D frame before window must be rejected, got {result:?}",
        );
    }

    #[tokio::test]
    async fn consecutive_windows_on_same_connection() {
        let (addr, listener) = ephemeral_listener().await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            let mut got = Vec::new();
            while let Some(window) = conn.read_window().await.unwrap() {
                conn.send_ack(window.last_seq).await.unwrap();
                got.push(window);
            }
            got
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        // Window 1: 2 events
        client.write_all(&encode_window(2)).await.unwrap();
        client.write_all(&encode_json_frame(1, b"a")).await.unwrap();
        client.write_all(&encode_json_frame(2, b"b")).await.unwrap();
        let mut ack = [0u8; 6];
        tokio::io::AsyncReadExt::read_exact(&mut client, &mut ack)
            .await
            .unwrap();
        assert_eq!(u32::from_be_bytes([ack[2], ack[3], ack[4], ack[5]]), 2);

        // Window 2: 1 event
        client.write_all(&encode_window(1)).await.unwrap();
        client.write_all(&encode_json_frame(3, b"c")).await.unwrap();
        tokio::io::AsyncReadExt::read_exact(&mut client, &mut ack)
            .await
            .unwrap();
        assert_eq!(u32::from_be_bytes([ack[2], ack[3], ack[4], ack[5]]), 3);

        drop(client);
        let windows = server.await.unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].events.len(), 2);
        assert_eq!(windows[1].events.len(), 1);
        assert_eq!(windows[1].last_seq, 3);
    }

    // -----------------------------------------------------------------
    // R6-P2: per-window aggregate caps (event count / accumulated bytes).
    // A peer can declare a huge Window `count` and stream many small
    // frames, forcing `events` to grow unboundedly. These tests pin the
    // declared-count reject, the mid-stream count reject, the byte-total
    // reject, and that a within-cap window still works.
    // -----------------------------------------------------------------

    #[test]
    fn default_window_caps_are_documented_values() {
        assert_eq!(DEFAULT_MAX_WINDOW_EVENTS, 100_000);
        assert_eq!(DEFAULT_MAX_WINDOW_BYTES, 256 * 1024 * 1024);
    }

    #[tokio::test]
    async fn declared_window_count_above_cap_is_rejected_before_accumulating() {
        // Cap at 3 events. A window declaring count=10 must be rejected
        // the instant the Window header is parsed — before ANY data frame
        // is read, so `events` never grows.
        let (addr, listener) = ephemeral_listener_capped(3, 1 << 30).await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_window().await
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        // Only send the oversized Window header — deliberately send NO
        // data frames. If the server accumulated unboundedly it would
        // block waiting for 10 frames; instead it must reject immediately.
        client.write_all(&encode_window(10)).await.unwrap();
        client.flush().await.unwrap();

        let result = server.await.unwrap();
        match result {
            Err(ProtocolError::WindowTooLarge {
                kind: "event count",
                requested: 10,
                limit: 3,
            }) => {}
            other => panic!("expected WindowTooLarge(count=10, limit=3), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn accumulated_bytes_above_cap_is_rejected_mid_window() {
        // Event-count cap is generous (100), but the byte cap is 10. The
        // window declares 5 events (within the event cap), so the up-front
        // count gate does NOT fire — the byte guard inside record_event is
        // the sole defence. Three 4-byte events = 12 bytes > 10 → reject on
        // the 3rd before it is pushed, proving accumulation stops early.
        let (addr, listener) = ephemeral_listener_capped(100, 10).await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_window().await
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        let mut wire = Vec::new();
        wire.extend_from_slice(&encode_window(5));
        for i in 0..5_u32 {
            wire.extend_from_slice(&encode_json_frame(i + 1, b"4444")); // 4 bytes each
        }
        client.write_all(&wire).await.unwrap();
        client.flush().await.unwrap();

        let result = server.await.unwrap();
        match result {
            Err(ProtocolError::WindowTooLarge {
                kind: "byte total",
                requested,
                limit: 10,
            }) => {
                // After 2 events accumulated=8; the 3rd would make 12 > 10.
                assert_eq!(requested, 12, "must reject when total would exceed 10");
            }
            other => panic!("expected WindowTooLarge(byte total, limit=10), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn byte_cap_is_enforced_inside_compressed_batch() {
        // The byte guard must also fire for events smuggled inside a
        // compressed batch (the inner record_event call site). Declared
        // count = 5 (within the event cap), byte cap = 10. Five 4-byte
        // events inside one C frame → reject mid-batch at 12 > 10.
        let (addr, listener) = ephemeral_listener_capped(100, 10).await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_window().await
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        let mut inner = Vec::new();
        for i in 0..5_u32 {
            inner.extend_from_slice(&encode_json_frame(i + 1, b"4444"));
        }
        let compressed = encode_compressed(6, &inner).unwrap();
        let mut wire = Vec::new();
        wire.extend_from_slice(&encode_window(5));
        wire.extend_from_slice(&compressed);
        client.write_all(&wire).await.unwrap();
        client.flush().await.unwrap();

        let result = server.await.unwrap();
        match result {
            Err(ProtocolError::WindowTooLarge {
                kind: "byte total",
                requested: 12,
                limit: 10,
            }) => {}
            other => panic!("expected WindowTooLarge(byte total, limit=10) in C batch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn window_exactly_at_caps_still_succeeds() {
        // A window whose declared count == cap and whose total bytes ==
        // cap must be accepted (boundary is inclusive on the accept side).
        // Cap: 3 events, 9 bytes. Send 3 events of 3 bytes = 9 bytes total.
        let (addr, listener) = ephemeral_listener_capped(3, 9).await;
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.read_and_ack().await.unwrap()
        });

        let mut client = ClientTcp::connect(addr).await.unwrap();
        let mut wire = Vec::new();
        wire.extend_from_slice(&encode_window(3));
        for i in 0..3_u32 {
            wire.extend_from_slice(&encode_json_frame(i + 1, b"abc")); // 3 bytes
        }
        client.write_all(&wire).await.unwrap();
        client.flush().await.unwrap();

        let mut ack = [0u8; 6];
        tokio::io::AsyncReadExt::read_exact(&mut client, &mut ack)
            .await
            .unwrap();
        assert_eq!(u32::from_be_bytes([ack[2], ack[3], ack[4], ack[5]]), 3);

        let window = server.await.unwrap().expect("at-cap window must succeed");
        assert_eq!(window.events.len(), 3);
        assert_eq!(window.last_seq, 3);
    }
}
