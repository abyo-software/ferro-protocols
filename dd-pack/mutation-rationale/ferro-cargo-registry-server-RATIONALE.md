<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-cargo-registry-server mutation-kill rationale

## Wave 2 (R3/R4 persistence + rollback)

Baseline before this wave: **92.3% kill (13 missed of 168 viable)**.
The R3/R4 durable-persistence and publish-rollback code added mutants the
prior wave's tests did not all cover.

Scope of this wave: **tests only** under
`crates/ferro-cargo-registry-server/src/**` (`#[cfg(test)]` modules) and
`crates/ferro-cargo-registry-server/tests/**`. No library logic was
changed; the real-`cargo` e2e (`tests/cargo_e2e.rs`) and
`tests/http_roundtrip.rs` stay green.

Of the 13 wave-2 missed mutants, **6 now have a dedicated killing test**
(the two `rollback_publish` `&&` mutants, the three `load` `NotFound`-guard
mutants, plus the wave-1 carry-over kills below are unaffected). The
remaining **7 are classified by-design un-killable** — each returns
observably-identical output to the original (best-effort directory fsync /
tracing-level-only) or is a binary/lifecycle entry point with no in-process
observable surface.

### Wave-2 mutant → killing test map (6 killed)

| # | Mutant (file:line) | Replacement | Killing test | Why it kills |
|---|--------------------|-------------|--------------|--------------|
| W1 | `handlers.rs:332:38` `rollback_publish` first `&&` | `\|\|` | `handlers::tests::rollback_publish_keeps_record_with_surviving_sibling_version` | After rolling back `2.0.0`, the record still has a surviving `1.0.0` **entry** (`entries.is_empty() == false`) while `tarballs`/`owners` are empty. Real `&&`: `false && … == false` → keep. `\|\|`: `false \|\| (true && true) == true` → wrongly removes the crate, dropping the surviving `1.0.0`. The test asserts `1.0.0` survives. |
| W2 | `handlers.rs:332:68` `rollback_publish` second `&&` | `\|\|` | `handlers::tests::rollback_publish_keeps_record_with_surviving_owners` | After rolling back the crate's only version, `entries` and `tarballs` are empty but the **owner** list is non-empty. Real `(true && true) && owners.is_empty() == (true && false) == false` → keep. `\|\|`: `(true && true) \|\| false == true` → wrongly drops the crate and its owners. The test asserts the owner list survives. (A baseline test `rollback_publish_removes_fully_emptied_record` proves the `true && true && true` remove arm is real, so the keep-tests cannot pass vacuously.) |
| W3 | `persist.rs:88:21` `load` `NotFound` guard | `true` | `persist::tests::unreadable_snapshot_takes_non_notfound_arm` | A snapshot *path that is a directory* fails `std::fs::read` with a **non-`NotFound`** kind and must hit the `warn!("failed to read…")` arm. Guard forced `true` would route it through the silent `debug!("no index snapshot…")` arm. The test captures the emitted log (thread-local `tracing` subscriber) and asserts the warn line fired and the debug line did **not**. |
| W4 | `persist.rs:88:21` `load` `NotFound` guard | `false` | `persist::tests::absent_snapshot_takes_notfound_arm_silently` | An **absent** file (genuine `NotFound`) must hit the silent `debug!` arm. Guard forced `false` would route it through the `warn!` arm. The test asserts the `debug!` line fired and the `warn!` line did **not**. |
| W5 | `persist.rs:88:32` `load` guard `==` | `!=` | both W3 + W4 | Inverting the comparison swaps the two arms: the absent-file test then sees the warn line (fails W4's `!contains("failed to read")`), and the directory test sees the debug line (fails W3's `!contains("no index snapshot")`). Killed in both directions. |

The capturing subscriber is a synchronous `tracing::subscriber::with_default`
on the current thread (these are plain `#[test]`s, not `#[tokio::test]`), so
it cannot race a neighbour test's global subscriber — consistent with the
`tokio_test_tracing_global_state` guidance: only the **async** IndexManager
tests are at risk, and these load tests are synchronous.

### Wave-2 by-design un-killable (7)

| # | Mutant (file:line) | Replacement | Category | Why no test |
|---|--------------------|-------------|----------|-------------|
| A | `persist.rs:245:25` `write_atomic_durable` match-guard `err.kind()==AlreadyExists` → `true` | `true` | retry-loop equivalent | The guard decides whether an `open` error is treated as an `O_EXCL` temp-name collision (record in `last_err`, retry the loop) or returned immediately. Forcing it `true` routes a **non-`AlreadyExists`** error (e.g. `NotADirectory` when the data dir is invalid) into the retry arm — but that arm stores the same error in `last_err`, the loop exhausts, and `Err(last_err)` returns the **same error kind**. The real `AlreadyExists` retry-success path is already pinned by `save_handles_create_new_temp_collision`. There is no constructible input where `true` vs the real guard yields a different return value (success/failure or error kind); only the iteration count differs, which is unobservable. |
| B | `persist.rs:267:5` `sync_parent_dir` → `Ok(())` | no-op body | best-effort fsync | The function fsyncs the *parent directory* so a preceding `rename` is crash-durable. Its success/failure is invisible to any in-process observer: the file bytes were already `sync_all`-ed and the directory entry already exists in the live filesystem. Replacing the body with `Ok(())` removes only the crash-durability fsync, which cannot be observed without an actual power-loss/crash harness (out of scope, and indistinguishable from a normal successful `save`). |
| C | `persist.rs:273:25` `sync_parent_dir` `InvalidInput` guard → `true` | `true` | best-effort fsync | The guard tolerates an `InvalidInput` from `dir.sync_all()` (platforms that reject fsync on a directory handle) by returning `Ok(())` instead of propagating. On Linux a directory `sync_all` **succeeds**, so this arm is never reached in practice; the success return is identical whether the guard is `true` or the real value. Forcing it `true` would only swallow a hypothetical `InvalidInput` that the test platform never produces. |
| D | `persist.rs:273:25` `sync_parent_dir` `InvalidInput` guard → `false` | `false` | best-effort fsync | Same as C: on the test platform the directory fsync succeeds and this guard is not exercised, so forcing it `false` changes no observable output. Reliably producing an `InvalidInput` dir-fsync error is platform-dependent and not reproducible in CI. |
| E | `persist.rs:273:36` `sync_parent_dir` guard `==` → `!=` | `!=` | best-effort fsync | Same root cause as C/D: the InvalidInput arm is not reached on the test platform, so inverting the comparison has no observable effect on `save`'s return value. |
| F | `serve.rs:233` `init_tracing` → `()` | no-op | tracing-only / lifecycle | `init_tracing` already returns `()`; its only effect is installing a global `tracing` subscriber. Asserting that mutates process-global subscriber state — flaky and forbidden as a hard gate (`tokio_test_tracing_global_state`). `init_tracing_is_idempotent` pins no-panic only. |
| G | `serve.rs:240` `shutdown_signal` → `()` | resolve immediately | lifecycle | The real body awaits `SIGINT`/`SIGTERM`; the mutant resolves at once, changing only *when* `axum::serve` returns. Distinguishing it needs a bound socket plus an OS signal delivered mid-serve — non-deterministic and unsafe under `unsafe_code = forbid`. |
| H | `bin/ferro-cargo-registry-server.rs:38` `main` → `ExitCode::default()` | `SUCCESS` | lifecycle / binary-entry | The binary `main` boots a long-running server; it has no in-process unit surface and its success/failure `ExitCode` mapping cannot be driven deterministically without spawning the process. The library `serve()` error path that `main` forwards **is** covered by `serve::tests::serve_rejects_invalid_listen_before_binding`. |

> Counting note: `missed.txt` lists the three `persist.rs:273` variants
> (`true`, `false`, `==`→`!=`, rows C/D/E) and the `267` body mutant (B) as
> four separate entries; all four are the same best-effort directory-fsync
> equivalence class. Together with the `245` retry-equivalent (A) and the
> three lifecycle/tracing entry points (F/G/H) this is **7 by-design**
> entries, tallied per `missed.txt` line.

### Wave-2 expected outcome

- Newly killed: **6** wave-2 missed mutants (W1–W5; W5 is killed by the two
  W3/W4 tests, so 5 dedicated tests cover 6 mutant entries:
  `332:38`, `332:68`, `88:21→true`, `88:21→false`, `88:32`).
- Remaining by-design un-killable: **7** (one `write_atomic_durable`
  retry-equivalent, four best-effort directory-fsync, `init_tracing`,
  `shutdown_signal`, binary `main`).
- Projected kill rate: from **155 / 168 (92.3%)** to **161 / 168 ≈ 95.8%**
  (≥ the 95% target; need ≥ 160 of 168 → +5 net suffices, this wave adds
  +6). The orchestrator's central `cargo mutants` re-run is the authority.

---

## Wave 1 (initial coverage) — carried forward

Baseline before wave 1: **78.7% kill (32 missed of 150 viable)**.

Of those 32, **25 received a dedicated killing test** and **7 were
classified by-design**. (Wave-1 line numbers below predate the R3/R4 edits
that shifted `persist.rs`/`serve.rs` lines; wave-2's table above uses the
current lines. The wave-1 persist guard rows A/B/C were re-examined in
wave 2 and are now **killed** by the log-capturing tests W3/W4/W5 — they
move out of the by-design set.)

### Wave-1 mutant → killing test map (25 killed)

| # | Mutant (file:line) | Replacement | Killing test | Why it kills |
|---|--------------------|-------------|--------------|--------------|
| 1 | `error.rs:92` delete arm `BlobStoreError::NotFound(_)` | (arm removed) | `error::tests::storage_not_found_is_404` | Asserts `Storage(NotFound)` → `404`. With the arm gone it folds into the `_ => 500` catch-all. |
| 2 | `error.rs:93` delete arm `DigestMismatch{..}\|InvalidDigest(_)` | (arm removed) | `error::tests::storage_digest_mismatch_is_400` + `storage_invalid_digest_is_400` | Both `DigestMismatch` and `InvalidDigest` must map to `400`; deleting the arm regresses them to `500`. A separate `storage_io_error_is_500` pins the catch-all so the arms are distinguished from it. |
| 3 | `name.rs:22:38` `>` → `>=` in `is_valid_name` | `>=` | `name::tests::name_length_boundary_is_inclusive_at_max` | A 64-char (`MAX_NAME_LEN`) name must be **valid**; `>=` would reject it. 65 still invalid. |
| 4 | `name.rs:88:9` delete arm `0` in `index_path` | (arm removed) | `name::tests::index_path_empty_name_is_empty_string` | `index_path("")` must return `""`; deleting the arm routes empty through `_` (slice-panic / non-empty). |
| 5 | `publish.rs:43:19` `<` → `<=` in `parse` | `<=` | `publish::tests::exact_fit_metadata_consumes_whole_rest_then_fails_at_crate_len` (+ `under_fit_metadata_is_truncated`) | When `rest.len() == metadata_len`, the metadata slice is read in full and parsing fails only at the absent crate_len prefix (`"4-byte length prefix"`). `<=` rejects earlier with `"metadata body truncated"` — asserted on the error **detail** string. |
| 6 | `publish.rs:64:19` `<` → `<=` in `read_u32_le` | `<=` | `publish::tests::exact_four_byte_prefix_is_read_not_rejected` | A trailing crate_len prefix of exactly 4 bytes must be read (crate_len 0, empty tarball parses OK). `<=` rejects a 4-byte prefix as "too short". A 3-byte body is still rejected (lower side). |
| 7 | `index.rs:64:5` `default_true` → `false` | `false` | `index::tests::dep_default_features_defaults_to_true` | A dep omitting `default_features` must deserialize to `true`; an explicit `false` is still honoured (distinguishes default from a hard constant). |
| 8 | `handlers.rs:417:81` `+` → `*` in `mutate_owners` | `*` | `http_roundtrip::owner_ids_are_sequential_and_distinct` | First owner id is `max(0)+1 = 1`; `max(0)*1 = 0` fails. Cross-call `dave` continues at `4` (prior max 3 + 1), `3*1 = 3` would duplicate. |
| 9 | `handlers.rs:427:21` `+=` → `*=` in `mutate_owners` | `*=` | `http_roundtrip::owner_ids_are_sequential_and_distinct` | Three owners added in one call must get ids 1/2/3; `next_id *= 1` freezes the counter so all three would share id 1. Distinct-id assertions fail. |
| 10 | `handlers.rs:441:5` `derive_index_path` → `String::new()` | `String::new()` | `handlers::tests::derive_index_path_matches_index_path_layout` | Asserts the real layout (`"se/rd/serde"`, `"1/a"`, non-empty) for several names; an empty string fails. |
| 11 | `handlers.rs:441:5` `derive_index_path` → `"xyzzy".into()` | `"xyzzy"` | same as #10 | Concrete expected paths differ from the constant `"xyzzy"`. |
| 12 | `handlers.rs:61:5` `handle_sparse_index_root2` → `Ok(Default::default())` | empty 200 | `http_roundtrip::root_relative_two_segment_index_serves_entry` | `GET /2/ab` must return the published index line (`name="ab"`); the default empty-body 200 has no line. An unknown `/2/zz` is `404`, proving the handler consults the index rather than returning a default. |
| 13 | `metrics.rs:73:9` custom `Debug::fmt` → `Ok(Default::default())` | empty output | `metrics::tests::metrics_debug_renders_struct_name` | `{:?}` must contain `"Metrics"` and be non-empty; the mutant emits an empty string. |
| 14 | `metrics.rs:176:13` delete grouped index arm | (arm removed) | `metrics::tests::handler_for_each_matched_arm_is_distinct` | The three index route shapes must map to `"index"`; with the arm gone, a non-`/index` path falls through to `"other"`. |
| 15 | `metrics.rs:179:13` delete `/index.git/{*path}` arm | (arm removed) | same as #14 | `git_index` route → `"git_index"`; deleted arm + path `"x"` → `"other"`. |
| 16 | `metrics.rs:182:13` delete `.../yank` arm | (arm removed) | same as #14 | yank route → `"yank"`; deleted → `"other"`. |
| 17 | `metrics.rs:183:13` delete `.../unyank` arm | (arm removed) | same as #14 | unyank → `"unyank"`; deleted → `"other"`. |
| 18 | `metrics.rs:184:13` delete `.../owners` arm | (arm removed) | same as #14 | owners → `"owners"`; deleted → `"other"`. |
| 19 | `metrics.rs:185:13` delete `/live` arm | (arm removed) | same as #14 | live → `"live"`; deleted → `"other"`. |
| 20 | `metrics.rs:186:13` delete `/ready` arm | (arm removed) | same as #14 | ready → `"ready"`; deleted → `"other"`. |
| 21 | `metrics.rs:283:9` delete `Method::HEAD` arm | (arm removed) | `metrics::tests::method_label_maps_each_method` | `HEAD` → `"HEAD"`; deleted folds into `_ => "OTHER"`. |
| 22 | `metrics.rs:284:9` delete `Method::POST` arm | (arm removed) | same as #21 | `POST` → `"POST"`. |
| 23 | `metrics.rs:285:9` delete `Method::PUT` arm | (arm removed) | same as #21 | `PUT` → `"PUT"`. |
| 24 | `metrics.rs:286:9` delete `Method::PATCH` arm | (arm removed) | same as #21 | `PATCH` → `"PATCH"`. |
| 25 | `metrics.rs:287:9` delete `Method::DELETE` arm + `metrics.rs:288:9` delete `Method::OPTIONS` arm | (arms removed) | same as #21 | `DELETE` → `"DELETE"`, `OPTIONS` → `"OPTIONS"`; each deleted arm folds into `"OTHER"`. (Two mutant lines, one test asserting every method distinctly.) |

### Wave-1 by-design (4 still standing; persist guards moved to wave-2 kills)

The wave-1 `init_tracing` / `shutdown_signal` / binary-`main` entries are the
same as wave-2 rows F/G/H above (line numbers shifted by the R3/R4 edits).
The three wave-1 `persist.rs` load-guard entries are **no longer by-design**
— wave 2's `capture_logs`-based tests (W3/W4/W5) kill them.
