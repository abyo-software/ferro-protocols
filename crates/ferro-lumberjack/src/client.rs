// SPDX-License-Identifier: Apache-2.0
//! Async Lumberjack v2 client (sender side).
//!
//! Build with [`ClientBuilder`]; it produces a [`Client`] that holds an
//! open connection to a Logstash endpoint and exposes
//! [`Client::send_json`] for shipping batches of JSON-encoded events.
//!
//! ### Sequence numbers
//!
//! The client owns a persistent monotonic [`crate::Sequence`] counter
//! across calls. Every call to [`Client::send_json`] advances the
//! counter by the number of events sent and validates that the ACK from
//! Logstash references the expected last-in-window sequence under
//! wrapping `u32` arithmetic (see [`crate::Sequence`]).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::frame::{encode_compressed, encode_json_frame, encode_window};
use crate::{ProtocolError, Sequence};

#[cfg(feature = "tls")]
use crate::tls::TlsConfig;

/// Default per-operation timeout: 30 seconds. Applied to connect, write,
/// and ACK read independently.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default zlib compression level. `0` disables compression entirely.
pub const DEFAULT_COMPRESSION: u32 = 3;

/// Builder for [`Client`].
#[derive(Debug, Default)]
pub struct ClientBuilder {
    hosts: Vec<String>,
    load_balance: bool,
    timeout: Option<Duration>,
    compression_level: Option<u32>,
    #[cfg(feature = "tls")]
    tls: Option<TlsConfig>,
}

impl ClientBuilder {
    /// Begin a new builder with no hosts configured.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a Logstash endpoint of the form `host:port` (or
    /// `[v6]:port`). Calling more than once enables fail-over (see
    /// [`Self::load_balance`]).
    #[must_use]
    pub fn add_host(mut self, host: impl Into<String>) -> Self {
        self.hosts.push(host.into());
        self
    }

    /// When `true`, successive [`Client::send_json`] calls round-robin
    /// across the configured hosts. When `false` (default), the client
    /// always tries the first host first and falls through the rest on
    /// failure.
    #[must_use]
    pub const fn load_balance(mut self, enabled: bool) -> Self {
        self.load_balance = enabled;
        self
    }

    /// Per-operation timeout (connect, write, ack-read). Default 30s.
    #[must_use]
    pub const fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }

    /// zlib compression level for the entire batch. `0` to disable, `9`
    /// for maximum. Default `3`.
    #[must_use]
    pub const fn compression_level(mut self, level: u32) -> Self {
        self.compression_level = Some(level);
        self
    }

    /// Enable TLS for the connection.
    #[cfg(feature = "tls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tls")))]
    #[must_use]
    pub fn tls(mut self, tls: TlsConfig) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Open a connection to one of the configured hosts and return a
    /// [`Client`] ready to ship events. The TCP (and TLS, if enabled)
    /// handshake is performed eagerly; subsequent
    /// [`Client::send_json`] calls reuse the same connection until it
    /// fails, at which point the next call reconnects transparently.
    pub async fn connect(self) -> Result<Client, ProtocolError> {
        if self.hosts.is_empty() {
            return Err(ProtocolError::NoHostsConfigured);
        }
        let timeout = self.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let compression_level = self.compression_level.unwrap_or(DEFAULT_COMPRESSION);

        let mut client = Client {
            hosts: self.hosts,
            load_balance: self.load_balance,
            timeout,
            compression_level,
            host_cursor: AtomicUsize::new(0),
            seq: Sequence::new(0),
            connection: None,
            #[cfg(feature = "tls")]
            tls: self.tls,
        };
        client.reconnect().await?;
        Ok(client)
    }
}

/// An open connection (or pre-connection) to a Logstash endpoint, plus
/// the persistent state needed to drive Lumberjack v2 batches.
pub struct Client {
    hosts: Vec<String>,
    load_balance: bool,
    timeout: Duration,
    compression_level: u32,
    host_cursor: AtomicUsize,
    seq: Sequence,
    connection: Option<Connection>,
    #[cfg(feature = "tls")]
    tls: Option<TlsConfig>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("Client");
        d.field("hosts", &self.hosts)
            .field("load_balance", &self.load_balance)
            .field("timeout", &self.timeout)
            .field("compression_level", &self.compression_level)
            .field("host_cursor", &self.host_cursor)
            .field("seq", &self.seq.value())
            .field("connected", &self.connection.is_some());
        #[cfg(feature = "tls")]
        d.field("tls_enabled", &self.tls.is_some());
        d.finish_non_exhaustive()
    }
}

enum Connection {
    Plain(TcpStream),
    #[cfg(feature = "tls")]
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl Connection {
    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.write_all(buf).await,
            #[cfg(feature = "tls")]
            Self::Tls(s) => s.write_all(buf).await,
        }
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush().await,
            #[cfg(feature = "tls")]
            Self::Tls(s) => s.flush().await,
        }
    }

    async fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read_exact(buf).await,
            #[cfg(feature = "tls")]
            Self::Tls(s) => s.read_exact(buf).await,
        }
    }
}

impl Client {
    /// Current sequence-number watermark — the next sequence the
    /// client will assign to an event. Useful for testing.
    #[must_use]
    pub const fn next_seq(&self) -> u32 {
        self.seq.advance(1).value()
    }

    /// How many hosts are configured.
    #[must_use]
    pub const fn host_count(&self) -> usize {
        self.hosts.len()
    }

    /// Send a batch of JSON-encoded events as a single Lumberjack v2
    /// window. Returns the count of events the receiver acknowledged.
    ///
    /// On success the persistent sequence counter is advanced by the
    /// batch size. On failure (ack mismatch, partial ack, I/O, …) the
    /// connection is dropped and will be re-established on the next
    /// call; the sequence counter is **not** rolled back, so retries
    /// must use a fresh batch.
    pub async fn send_json(&mut self, events: Vec<Vec<u8>>) -> Result<u32, ProtocolError> {
        if events.is_empty() {
            return Ok(0);
        }
        let count =
            u32::try_from(events.len()).expect("ferro-lumberjack: batch size exceeds u32::MAX");

        // Reserve the sequence range up-front so a failed send still
        // moves the counter — same invariant as ferro-heartbeat /
        // ferro-beat: a successful retransmit on a new connection uses
        // a fresh sequence range.
        let base_seq = self.seq;
        let last_seq = self.seq.advance(count);
        self.seq = last_seq;

        let payload = self.build_window_payload(&events, base_seq);

        // Up to `hosts.len()` attempts: one per host, in load-balanced
        // order if enabled. Track last error for the all-failed case.
        let mut last_err: Option<ProtocolError> = None;
        for _ in 0..self.hosts.len() {
            match self
                .send_payload_once(&payload, base_seq.value(), count)
                .await
            {
                Ok(acked) => return Ok(acked),
                Err(e) => {
                    self.connection = None;
                    last_err = Some(e);
                    // Try a different host on the next iteration.
                    self.host_cursor.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let _ = last_seq; // last_seq is conceptually relevant for callers but
        // its value is also derivable from base_seq + count; we keep it as
        // a local for the readability of the seq-reservation step above.
        Err(ProtocolError::AllHostsFailed(Box::new(
            last_err.unwrap_or(ProtocolError::NoHostsConfigured),
        )))
    }

    /// One attempt against the *currently selected* host: ensure
    /// connected, write the payload, await ACK.
    async fn send_payload_once(
        &mut self,
        payload: &[u8],
        base_seq: u32,
        count: u32,
    ) -> Result<u32, ProtocolError> {
        let expected_seq = base_seq.wrapping_add(count);
        if self.connection.is_none() {
            self.reconnect().await?;
        }
        let conn = self
            .connection
            .as_mut()
            .expect("reconnect leaves connection populated or returns Err");

        tokio::time::timeout(self.timeout, conn.write_all(payload))
            .await
            .map_err(|_| ProtocolError::Timeout("write"))?
            .map_err(ProtocolError::Io)?;
        tokio::time::timeout(self.timeout, conn.flush())
            .await
            .map_err(|_| ProtocolError::Timeout("flush"))?
            .map_err(ProtocolError::Io)?;

        // ACK frame: 6 bytes (`2 A <u32 seq>`).
        let mut ack = [0u8; 6];
        tokio::time::timeout(self.timeout, conn.read_exact(&mut ack))
            .await
            .map_err(|_| ProtocolError::Timeout("ack"))?
            .map_err(ProtocolError::Io)?;

        let acked_seq = u32::from_be_bytes([ack[2], ack[3], ack[4], ack[5]]);
        if ack[0] != b'2' || ack[1] != b'A' {
            return Err(ProtocolError::UnexpectedAck {
                version: ack[0],
                frame_type: ack[1],
                acked_seq,
                expected_seq,
            });
        }

        let last = Sequence::new(expected_seq);
        if last.is_exactly_acked_by(acked_seq) {
            // Full ack: the receiver acknowledged every event we sent.
            return Ok(count);
        }

        // Partial / out-of-window: compute the acked count under wrapping
        // arithmetic. `acked_count` should be in `1..=count`; anything
        // else is a malformed ACK we surface as `UnexpectedAck`.
        let acked_count = acked_seq.wrapping_sub(base_seq);
        if acked_count == 0 || acked_count > count {
            return Err(ProtocolError::UnexpectedAck {
                version: ack[0],
                frame_type: ack[1],
                acked_seq,
                expected_seq,
            });
        }
        Err(ProtocolError::PartialAck {
            acked: acked_count,
            sent: count,
        })
    }

    fn build_window_payload(&self, events: &[Vec<u8>], base_seq: Sequence) -> Vec<u8> {
        // Build the inner data frames first.
        let mut inner = Vec::with_capacity(events.len() * 64);
        for (i, event) in events.iter().enumerate() {
            let seq = base_seq.advance(u32::try_from(i).unwrap_or(u32::MAX) + 1);
            inner.extend_from_slice(&encode_json_frame(seq.value(), event));
        }

        // Window header (always uncompressed).
        let count = u32::try_from(events.len()).unwrap_or(u32::MAX);
        let mut out = Vec::with_capacity(6 + inner.len());
        out.extend_from_slice(&encode_window(count));

        // Compress if requested AND if it actually shrinks the bytes.
        if self.compression_level > 0
            && let Ok(compressed) = encode_compressed(self.compression_level, &inner)
            && compressed.len() < inner.len()
        {
            out.extend_from_slice(&compressed);
            return out;
        }
        out.extend_from_slice(&inner);
        out
    }

    fn pick_host(&self) -> &str {
        if self.load_balance {
            let idx = self.host_cursor.fetch_add(1, Ordering::Relaxed) % self.hosts.len();
            &self.hosts[idx]
        } else {
            // Sticky to the cursor so failover advances it on error.
            let idx = self.host_cursor.load(Ordering::Relaxed) % self.hosts.len();
            &self.hosts[idx]
        }
    }

    async fn reconnect(&mut self) -> Result<(), ProtocolError> {
        let host = self.pick_host().to_string();
        let tcp = tokio::time::timeout(self.timeout, TcpStream::connect(&host))
            .await
            .map_err(|_| ProtocolError::Timeout("connect"))?
            .map_err(ProtocolError::Io)?;

        #[cfg(feature = "tls")]
        if let Some(ref tls) = self.tls {
            let connector = tokio_rustls::TlsConnector::from(tls.inner());
            let server_name = crate::tls::parse_sni(&host)?;
            let tls_stream =
                tokio::time::timeout(self.timeout, connector.connect(server_name, tcp))
                    .await
                    .map_err(|_| ProtocolError::Timeout("tls handshake"))?
                    .map_err(ProtocolError::Io)?;
            self.connection = Some(Connection::Tls(Box::new(tls_stream)));
            return Ok(());
        }

        self.connection = Some(Connection::Plain(tcp));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_requires_hosts() {
        let res = tokio_test::block_on(ClientBuilder::new().connect());
        assert!(matches!(res, Err(ProtocolError::NoHostsConfigured)));
    }

    #[tokio::test]
    async fn happy_path_uncompressed_round_trip() {
        // Spawn a fake "Logstash" that reads the window + JSON frames and
        // returns a matching ACK.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await.unwrap();
            // Reply with seq 2 (we sent 2 events).
            sock.write_all(&[b'2', b'A', 0, 0, 0, 2]).await.unwrap();
            sock.flush().await.unwrap();
        });

        let mut client = ClientBuilder::new()
            .add_host(addr.to_string())
            .compression_level(0)
            .timeout(Duration::from_secs(5))
            .connect()
            .await
            .unwrap();

        let acked = client
            .send_json(vec![br#"{"a":1}"#.to_vec(), br#"{"b":2}"#.to_vec()])
            .await
            .unwrap();
        assert_eq!(acked, 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn unexpected_ack_version_is_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await.unwrap();
            // Wrong version byte.
            sock.write_all(&[b'1', b'A', 0, 0, 0, 1]).await.unwrap();
            sock.flush().await.unwrap();
        });

        let mut client = ClientBuilder::new()
            .add_host(addr.to_string())
            .compression_level(0)
            .timeout(Duration::from_secs(5))
            .connect()
            .await
            .unwrap();

        let err = client
            .send_json(vec![b"x".to_vec()])
            .await
            .expect_err("must reject bad version");
        assert!(matches!(err, ProtocolError::AllHostsFailed(_)));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn ack_timeout_surfaces_as_timeout_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await.unwrap();
            tokio::time::sleep(Duration::from_secs(3)).await;
            // Drop without acking.
            drop(sock);
        });

        let mut client = ClientBuilder::new()
            .add_host(addr.to_string())
            .compression_level(0)
            .timeout(Duration::from_millis(200))
            .connect()
            .await
            .unwrap();

        let err = client
            .send_json(vec![b"x".to_vec()])
            .await
            .expect_err("must time out");
        assert!(matches!(err, ProtocolError::AllHostsFailed(_)));
        server.abort();
    }

    #[tokio::test]
    async fn empty_batch_returns_zero() {
        // No server needed — empty batches short-circuit before I/O.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Single connection just to satisfy ClientBuilder::connect.
        let server = tokio::spawn(async move {
            let _ = listener.accept().await.unwrap();
        });

        let mut client = ClientBuilder::new()
            .add_host(addr.to_string())
            .timeout(Duration::from_secs(5))
            .connect()
            .await
            .unwrap();
        let acked = client.send_json(vec![]).await.unwrap();
        assert_eq!(acked, 0);
        let _ = server.await;
    }
}
