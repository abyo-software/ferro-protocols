# ferro-lumberjack — mutation-test by-design rationale

Mirrors the ferro-heartbeat FHB8 classification scheme. Baseline before
this wave: **69.2 % kill (70 missed of 227 viable, +35 timeouts counted
as caught)**. This wave added/strengthened TESTS ONLY (no library-logic
changes) to kill the killable comparison-boundary, constant-return, and
arithmetic mutants. The mutants below are the **irreducible remainder**:
each is genuinely un-killable by an assertion because the replacement is
behaviourally indistinguishable from the original on every reachable,
deterministic input.

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
| `src/client.rs:303:44: replace > with >=` in `Client::send_payload_once` | `acked_count > count` only differs from `acked_count >= count` at `acked_count == count`. But `acked_count == count` means `acked_seq == base_seq + count`, which is exactly the **full-ack** case handled earlier by `last.is_exactly_acked_by(acked_seq)` → `return Ok(count)`. Line 303 is therefore never reached with `acked_count == count`, so `>` and `>=` are indistinguishable. (The `> == 0` and the `> count` *strictly-greater* cases are killed by the new `zero_acked_count_is_unexpected_ack` and `over_count_ack_is_unexpected_ack` tests.) |
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
- **comparison-boundary** (`frame.rs:272/296/340/359/376/381/387/407/417/424/432/439/448/479`;
  `client.rs:303` reachable arms / `331/333`; `server.rs:298/355`):
  boundary trios (just-below / at / just-above the threshold) asserting
  the decode result/state so `<`, `<=`, `==`, `>`, `>=` are all
  distinguishable.
- **arithmetic** (`frame.rs:393` `+= / +`; `frame.rs:272:65` `/ -> % / *`;
  `client.rs:344/348` `% -> / / +`; `server.rs:358` `-= -> += / /=`;
  `frame.rs:417/432` `+ -> -`): inputs chosen so the mutated operation
  yields a different computed value, asserted directly (cursor advance,
  round-robin host index, slot-count decrement).
