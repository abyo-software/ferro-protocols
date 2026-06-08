# ferro-lumberjack — mutation-test by-design rationale

Mirrors the ferro-heartbeat FHB8 classification scheme. Baseline before
this wave: **69.2 % kill (70 missed of 227 viable, +35 timeouts counted
as caught)**. A first hardening pass reached **94.3 % (13 missed of 227
viable, 46 timeouts counted as caught)**. This closing pass (TESTS ONLY,
no library-logic changes) killed the remaining genuinely-observable
comparison-boundary mutants and rigorously classified the irreducible
remainder.

**Closing pass — net new kills (4):** four comparison-boundary mutants
that survived the first pass because the existing boundary tests fed
just-below / over-the-cap inputs but never the *exact equality* boundary
where `<` vs `<=` / `>` vs `>=` flips:

| Mutant | New test | Distinguishing input |
|--------|----------|----------------------|
| `frame.rs:296:18 < → <=` (`next_frame` `avail < 2`) | `next_frame_min_header_boundary_is_strictly_less_than_two` | Exactly 2 bytes `[ver, 'Z']`: real reads the header → `Err(UnknownFrameType)`; `<=` mutant returns `Ok(None)`. |
| `frame.rs:376:27 < → <=` (`try_decode_compressed` `pending() < 6`) | `compressed_header_boundary_is_strictly_less_than_six` | Exactly 6-byte C header with `len == 0`: real yields `Some(Compressed{[]})`; `<=` mutant returns `Ok(None)`. |
| `frame.rs:424:24 > → >=` (`key_len > cap`) | `legacy_d_frame_key_and_val_len_caps_accept_at_exactly_cap` | D frame with `key_len == cap`: real ACCEPTS → `Frame::Unknown`; `>=` mutant rejects as `PayloadTooLarge`. |
| `frame.rs:439:24 > → >=` (`val_len > cap`) | (same test) | Same D frame with `val_len == cap`: real ACCEPTS; `>=` mutant rejects. |

Each kill was verified by manually applying the mutation to the source
and confirming the named test fails (then reverting). This lifts the
crate over the 95 % bar. The mutants below are the **irreducible
remainder**: each is genuinely un-killable by an assertion because the
replacement is behaviourally indistinguishable from the original on every
reachable, deterministic input.

Categories (same taxonomy as FHB8):
- **bitwise-equiv** — the mutated expression is provably equal to the
  original for all reachable inputs.
- **defensive-cascade** — a guard whose mutation only changes behaviour
  on an input that an earlier guard already rejected (so it is never
  reached with the differentiating value).
- **lifecycle / I/O-delegation** — a thin async wrapper delegating to a
  socket; the success path is identical and the error path requires an
  adversarial in-process socket-failure injection not expressible
  against a real `tokio::net::TcpStream`.

---

## bitwise-equiv (constructor body identical to mutant)

These constructors' bodies are literally `T::default()` / `Default::default()`,
so replacing the body with `Default::default()` produces byte-identical
behaviour. There is no observable difference to assert.

| Mutant | Reason |
|--------|--------|
| `src/server.rs:61: replace Server::builder -> ServerBuilder with Default::default()` | Body is `ServerBuilder::default()`; the mutant `Default::default()` infers to the same type and value. |
| `src/tls.rs:47: replace TlsConfig::builder -> TlsConfigBuilder with Default::default()` | Body is `TlsConfigBuilder::default()`; identical to the mutant. |
| `src/tls.rs:164: replace ServerTlsConfig::builder -> ServerTlsConfigBuilder with Default::default()` | Body is `ServerTlsConfigBuilder::default()`; identical to the mutant. |

## defensive-cascade (differentiating value unreachable)

| Mutant | Reason |
|--------|--------|
| `src/client.rs:303:44: replace > with >=` in `Client::send_payload_once` | `acked_count > count` only differs from `acked_count >= count` at `acked_count == count`. But `acked_count == count` means `acked_seq == base_seq + count == expected_seq`, which is exactly the **full-ack** case handled earlier by `last.is_exactly_acked_by(acked_seq)` → `return Ok(count)`. Line 303 is therefore never reached with `acked_count == count`, so `>` and `>=` are indistinguishable. (The `> == 0` and the `> count` *strictly-greater* cases are killed by the new `zero_acked_count_is_unexpected_ack` and `over_count_ack_is_unexpected_ack` tests.) Verified: applying the `>=` mutation leaves the whole client test suite green. |
| `src/client.rs:331:35: replace > with >=` in `Client::build_window_payload` | `self.compression_level > 0` differs from `>= 0` only at `compression_level == 0`. At level 0 the `>=` mutant enters the `&&` chain and calls `encode_compressed(0, &inner)` — a zlib level-0 ("stored") stream, which **always adds framing overhead** (2-byte header + per-block markers + 4-byte Adler-32), so the third clause `compressed.len() < inner.len()` is **always false** and the function still appends the *uncompressed* `inner`. Both operators therefore emit byte-identical output at the only differentiating input. Verified: applying the `>= 0` mutation leaves the whole client test suite green. |
| `src/client.rs:333:33: replace < with <=` in `Client::build_window_payload` | `compressed.len() < inner.len()` differs from `<=` only at the exact equality `compressed.len() == inner.len()`. zlib output is discrete and framing-overhead-dominated: for compressible inputs `compressed.len() < inner.len()` by a wide margin; for incompressible inputs `compressed.len() > inner.len()` by the ~6–11-byte overhead. There is no deterministic, portable input that lands `compressed.len()` *exactly* on `inner.len()` (the crossover jumps over equality), so the differentiating value is not reliably constructible across zlib/flate2 versions — a true exact-boundary equivalent for the test surface. The strict `<` direction is independently pinned by `build_window_payload_compresses_only_when_shrinking` (shrinks) and `build_window_payload_does_not_compress_incompressible_small_batch` (grows → uncompressed). Verified: applying the `<=` mutation leaves the whole client test suite green. |
| `src/frame.rs:272:26: replace > with >=` in `FrameDecoder::feed` | The guarded operand is `self.read_pos > 0`. It differs from `read_pos >= 0` (always true for `usize`) only at `read_pos == 0`. At `read_pos == 0` the compaction body runs `self.buf.drain(..0)` (drains nothing) and sets `read_pos = 0` (already 0) — a **no-op**. So the `>` vs `>=` choice has no observable effect at the only differentiating value. (The sibling `> -> ==` and `> -> <` mutants on the same operator ARE killed: with `==`/`<` the guard fails at `read_pos = 12`, suppressing a compaction the `feed_compacts_*` tests require.) |

## I/O-delegation (success path identical, error path not injectable)

These are one-line `async fn` wrappers that forward to
`AsyncWriteExt::flush` on the underlying stream. On a real connected
`TcpStream` (the only kind a deterministic test can construct without an
adversarial mock transport), `flush()` returns `Ok(())`; the wrapper has
no logic of its own to observe. Replacing the body with `Ok(())` is
indistinguishable on the success path, and producing a deterministic
flush *error* would require injecting a failing transport, which the
public API does not expose. Every send/ack round-trip test already drives
these wrappers on the success path.

| Mutant | Reason |
|--------|--------|
| `src/client.rs:170:9: replace Connection::flush -> std::io::Result<()> with Ok(())` | Thin delegation to `TcpStream::flush`; success path identical, error path not injectable. |
| `src/server.rs:178:9: replace Conn::flush -> io::Result<()> with Ok(())` | Same — delegation wrapper for the server `Conn`. |

---

## Killed in this wave (for the record)

All other entries in `mutants.out/missed.txt` were killed by new
assertions, grouped by category:

- **constants / wire-byte** (`frame.rs:63` `wire_byte -> 0|1`;
  `lib.rs:57` `* -> +`; `server.rs:51` `* -> +`;
  `client.rs:191/197`; `frame.rs` debug/fmt; `tls.rs:184` fmt;
  `tls.rs:77/195/218` `-> Ok(Default::default())`):
  exact-value assertions and missing-file error propagation.
- **comparison-boundary** (`frame.rs:272(siblings)/340/359/381/387/407/417/432/448/479`;
  `client.rs:303` reachable arms; `server.rs:298/355`):
  boundary trios (just-below / at / just-above the threshold) asserting
  the decode result/state so `<`, `<=`, `==`, `>`, `>=` are all
  distinguishable. The four exact-equality survivors
  (`frame.rs:296/376/424/439`) were closed in the closing pass — see the
  net-new-kills table at the top of this document.
- **arithmetic** (`frame.rs:393` `+= / +`; `frame.rs:272:65` `/ -> % / *`;
  `client.rs:344/348` `% -> / / +`; `server.rs:358` `-= -> += / /=`;
  `frame.rs:417/432` `+ -> -`): inputs chosen so the mutated operation
  yields a different computed value, asserted directly (cursor advance,
  round-robin host index, slot-count decrement).
