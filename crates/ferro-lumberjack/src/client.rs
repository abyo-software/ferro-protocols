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

    /// Build a `Client` with the given hosts/seq directly, bypassing the
    /// network. Used by the synchronous arithmetic/host tests.
    fn offline_client(hosts: Vec<String>, load_balance: bool, seq: u32) -> Client {
        Client {
            hosts,
            load_balance,
            timeout: DEFAULT_TIMEOUT,
            compression_level: 0,
            host_cursor: AtomicUsize::new(0),
            seq: Sequence::new(seq),
            connection: None,
            #[cfg(feature = "tls")]
            tls: None,
        }
    }

    #[test]
    fn next_seq_reports_advanced_watermark() {
        // client.rs:191 `next_seq -> 0` / `-> 1`. Start the counter at a
        // value where advance(1) is neither 0 nor 1, so both constant
        // mutants are killed.
        let c = offline_client(vec!["h:1".into()], false, 40);
        assert_eq!(c.next_seq(), 41, "advance(1) of 40 → 41");
        // next_seq is pure (does not mutate self.seq); calling twice is
        // stable.
        assert_eq!(c.next_seq(), 41);

        // Also exercise a value where the real result differs from the
        // mutant constant 1 *and* 0 unambiguously.
        let c2 = offline_client(vec!["h:1".into()], false, u32::MAX);
        assert_eq!(c2.next_seq(), 0, "advance(1) of u32::MAX wraps to 0");
        let c3 = offline_client(vec!["h:1".into()], false, 999);
        assert_eq!(c3.next_seq(), 1000);
    }

    #[test]
    fn host_count_reflects_configured_hosts() {
        // client.rs:197 `host_count -> 0` / `-> 1`.
        let c0 = offline_client(vec![], false, 0);
        assert_eq!(c0.host_count(), 0);
        let c1 = offline_client(vec!["a:1".into()], false, 0);
        assert_eq!(c1.host_count(), 1);
        let c3 = offline_client(vec!["a:1".into(), "b:2".into(), "c:3".into()], false, 0);
        assert_eq!(c3.host_count(), 3, "kills both `-> 0` and `-> 1` mutants");
    }

    #[test]
    fn pick_host_load_balanced_round_robins_with_modulo() {
        // client.rs:344 `% hosts.len()` (load-balanced arm). With 3 hosts
        // the cursor 0,1,2,3,4 must map to a,b,c,a,b — proving modulo, not
        // `/` (which would give 0,0,0,1,1 → a,a,a,b,b) and not `+` (which
        // would index out of bounds / wrong host).
        let c = offline_client(
            vec!["a:1".into(), "b:2".into(), "c:3".into()],
            true,
            0,
        );
        let picks: Vec<&str> = (0..6).map(|_| c.pick_host()).collect();
        assert_eq!(picks, ["a:1", "b:2", "c:3", "a:1", "b:2", "c:3"]);
    }

    #[test]
    fn pick_host_sticky_uses_modulo_of_cursor() {
        // client.rs:348 `% hosts.len()` (sticky arm). The sticky arm
        // does NOT advance the cursor; it maps the current cursor through
        // modulo. Pre-load the cursor to 4 with 3 hosts → 4 % 3 = 1 → "b".
        // `/`-mutant: 4 / 3 = 1 (same here) — so also test cursor=7 where
        // 7 % 3 = 1 ("b") but 7 / 3 = 2 ("c"), distinguishing `%` from `/`.
        let c = offline_client(
            vec!["a:1".into(), "b:2".into(), "c:3".into()],
            false,
            0,
        );
        c.host_cursor.store(7, Ordering::Relaxed);
        assert_eq!(c.pick_host(), "b:2", "7 % 3 = 1 → b (kills `/` → would pick c)");
        // Sticky: repeated calls return the same host (cursor unchanged).
        assert_eq!(c.pick_host(), "b:2");
        c.host_cursor.store(4, Ordering::Relaxed);
        assert_eq!(c.pick_host(), "b:2", "4 % 3 = 1 → b");
        c.host_cursor.store(5, Ordering::Relaxed);
        assert_eq!(c.pick_host(), "c:3", "5 % 3 = 2 → c");
    }

    #[test]
    fn build_window_payload_compresses_only_when_shrinking() {
        // client.rs:331 `compression_level > 0` and 333 `compressed.len()
        // < inner.len()`.
        //
        // Highly compressible large batch with compression enabled → the
        // payload must be SHORTER than the uncompressed-inner form, i.e.
        // the compressed branch was taken.
        let mut c = offline_client(vec!["h:1".into()], false, 0);
        c.compression_level = 9;
        let events: Vec<Vec<u8>> = (0..32).map(|_| vec![b'a'; 256]).collect();
        let inner_len: usize = events
            .iter()
            .map(|e| 10 + e.len()) // encode_json_frame overhead
            .sum();
        let payload = c.build_window_payload(&events, Sequence::new(0));
        // payload = 6-byte window header + body. Body should be the
        // compressed form (much smaller than inner_len).
        assert!(
            payload.len() < 6 + inner_len,
            "compressible batch must shrink: {} !< {}",
            payload.len(),
            6 + inner_len,
        );
        // First 6 bytes are the window header for count=32.
        assert_eq!(&payload[..2], b"2W");
        assert_eq!(
            u32::from_be_bytes([payload[2], payload[3], payload[4], payload[5]]),
            32,
        );
    }

    #[test]
    fn build_window_payload_skips_compression_when_disabled() {
        // client.rs:331 `compression_level > 0` (== mutant). With level 0
        // the body must be the raw uncompressed inner frames — so a JSON
        // frame header ('2','J') appears right after the 6-byte window
        // header. A `>`→`==` mutant on level 0 would also skip, but a
        // `>`→`<`/`>=` mutant misbehaves; assert the concrete layout.
        let mut c = offline_client(vec!["h:1".into()], false, 0);
        c.compression_level = 0;
        let events = vec![br#"{"x":1}"#.to_vec()];
        let payload = c.build_window_payload(&events, Sequence::new(0));
        assert_eq!(&payload[..2], b"2W", "window header");
        // Byte 6/7 begin the inner JSON frame (uncompressed).
        assert_eq!(&payload[6..8], b"2J", "uncompressed inner JSON frame");
    }

    #[test]
    fn build_window_payload_does_not_compress_incompressible_small_batch() {
        // client.rs:333 `compressed.len() < inner.len()`. A single tiny
        // event does not shrink under zlib (header overhead dominates), so
        // even with compression enabled the uncompressed branch is taken
        // and the inner JSON frame appears verbatim after the window
        // header. Kills `<`→`>`/`<=`/`==` (which would emit the larger
        // compressed form or mis-branch).
        let mut c = offline_client(vec!["h:1".into()], false, 0);
        c.compression_level = 9;
        let events = vec![b"q".to_vec()];
        let payload = c.build_window_payload(&events, Sequence::new(0));
        assert_eq!(&payload[..2], b"2W");
        assert_eq!(&payload[6..8], b"2J", "tiny batch stays uncompressed");
    }

    #[test]
    fn client_debug_includes_field_values() {
        // client.rs:140 Debug::fmt → `Ok(Default::default())`. The mutant
        // would produce an EMPTY string; assert the real impl renders the
        // struct name and concrete field values.
        let mut c = offline_client(vec!["host-x:9999".into()], true, 0);
        c.compression_level = 7;
        let s = format!("{c:?}");
        assert!(s.contains("Client"), "{s}");
        assert!(s.contains("host-x:9999"), "hosts field rendered: {s}");
        assert!(s.contains("load_balance: true"), "{s}");
        assert!(s.contains("compression_level: 7"), "{s}");
        assert!(s.contains("connected: false"), "{s}");
        assert!(!s.is_empty());
    }

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

    /// Spawn a fake Logstash that reads the request then replies with a
    /// single ACK frame carrying `ack_seq`. Returns the bound addr.
    async fn ack_server(ack_seq: u32) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await.unwrap();
            let mut ack = [b'2', b'A', 0, 0, 0, 0];
            ack[2..6].copy_from_slice(&ack_seq.to_be_bytes());
            sock.write_all(&ack).await.unwrap();
            sock.flush().await.unwrap();
        });
        addr
    }

    async fn connect_to(addr: std::net::SocketAddr) -> Client {
        ClientBuilder::new()
            .add_host(addr.to_string())
            .compression_level(0)
            .timeout(Duration::from_secs(5))
            .connect()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn full_ack_returns_count() {
        // base_seq starts at 0, send 3 events → expected last seq = 3.
        let addr = ack_server(3).await;
        let mut client = connect_to(addr).await;
        let acked = client
            .send_json(vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()])
            .await
            .unwrap();
        assert_eq!(acked, 3, "full ack must return the full count");
    }

    #[tokio::test]
    async fn partial_ack_surfaces_partial_error() {
        // client.rs:303. Send 3 events (base=0, count=3). Receiver acks
        // seq=2 → acked_count = 2, in 1..count → PartialAck{acked:2}.
        // Kills `==`→`!=` (would mark valid partial as UnexpectedAck) and
        // `>`→`<` (2 < 3 would mark partial as UnexpectedAck).
        let addr = ack_server(2).await;
        let mut client = connect_to(addr).await;
        let err = client
            .send_json(vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()])
            .await
            .expect_err("must be partial");
        match err {
            ProtocolError::AllHostsFailed(inner) => match *inner {
                ProtocolError::PartialAck { acked, sent } => {
                    assert_eq!(acked, 2);
                    assert_eq!(sent, 3);
                }
                other => panic!("expected PartialAck, got {other:?}"),
            },
            other => panic!("expected AllHostsFailed(PartialAck), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn zero_acked_count_is_unexpected_ack() {
        // client.rs:303 `acked_count == 0` arm. Receiver acks seq=0 which
        // equals base_seq → acked_count = 0 → UnexpectedAck (NOT
        // PartialAck). Kills `||`→`&&` (0 alone would slip to PartialAck)
        // and `==`→`!=`.
        let addr = ack_server(0).await;
        let mut client = connect_to(addr).await;
        let err = client
            .send_json(vec![b"a".to_vec(), b"b".to_vec()])
            .await
            .expect_err("must reject zero-acked");
        match err {
            ProtocolError::AllHostsFailed(inner) => assert!(
                matches!(*inner, ProtocolError::UnexpectedAck { .. }),
                "expected UnexpectedAck, got {inner:?}",
            ),
            other => panic!("expected AllHostsFailed(UnexpectedAck), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn over_count_ack_is_unexpected_ack() {
        // client.rs:303 `acked_count > count` arm. Send 2 events (base=0,
        // count=2). Receiver acks seq=5 → acked_count = 5 > 2 →
        // UnexpectedAck (NOT PartialAck). Kills `>`→`==` (5 != 2 so it
        // would slip to PartialAck) and `||`→`&&`.
        let addr = ack_server(5).await;
        let mut client = connect_to(addr).await;
        let err = client
            .send_json(vec![b"a".to_vec(), b"b".to_vec()])
            .await
            .expect_err("must reject over-count ack");
        match err {
            ProtocolError::AllHostsFailed(inner) => assert!(
                matches!(*inner, ProtocolError::UnexpectedAck { .. }),
                "expected UnexpectedAck, got {inner:?}",
            ),
            other => panic!("expected AllHostsFailed(UnexpectedAck), got {other:?}"),
        }
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
