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
use crate::{DEFAULT_MAX_FRAME_PAYLOAD, FrameError, ProtocolError};

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
                peer,
            });
        }

        Ok(ServerConnection {
            conn: Conn::Plain(sock),
            decoder: FrameDecoder::with_max_frame_payload(self.max_frame_payload),
            max_frame_payload: self.max_frame_payload,
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

    /// Record a JSON event into the in-flight window. Errors if a data
    /// frame arrives before its Window header, and decrements the
    /// remaining count.
    fn record_event(
        events: &mut Vec<JsonEvent>,
        window_remaining: &mut Option<u32>,
        last_seq: &mut u32,
        seq: u32,
        payload: Vec<u8>,
    ) -> Result<(), ProtocolError> {
        let Some(remaining) = window_remaining.as_mut() else {
            return Err(ProtocolError::Codec(FrameError::UnknownFrameType(b'J')));
        };
        if *remaining == 0 {
            return Err(ProtocolError::Codec(FrameError::UnknownFrameType(b'J')));
        }
        *remaining -= 1;
        *last_seq = seq;
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

    /// Spin up a Listener bound to 127.0.0.1:0 and return its addr +
    /// the listener itself.
    async fn ephemeral_listener() -> (SocketAddr, Listener) {
        let listener = Server::builder().bind("127.0.0.1:0").await.unwrap();
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
}
