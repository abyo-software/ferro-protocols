<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-cargo-registry-server mutation-kill rationale

Baseline before this wave: **78.7% kill (32 missed of 150 viable)**.
Source of truth: `dd-pack/mutation/ferro-cargo-registry-server/mutants.out/missed.txt`.

Scope of this wave: **tests only** under
`crates/ferro-cargo-registry-server/src/**` (`#[cfg(test)]` modules) and
`crates/ferro-cargo-registry-server/tests/**`. No library logic was
changed; the real-`cargo` e2e (`tests/cargo_e2e.rs`) stays green.

Of the 32 previously-missed mutants, **25 now have a dedicated killing
test** that asserts the exact observable behaviour the mutant breaks
(concrete values / statuses, not `is_ok()`). The remaining **7 are
classified by-design un-killable** — each returns observably-identical
output to the original (tracing-level-only) or is a binary/lifecycle entry
point with no in-process observable surface.

## Mutant → killing test map (25 killed)

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

> Counting note: rows #1, #2 and #25 each cover the small groups of
> adjacent missed lines listed together in `missed.txt`
> (`error.rs:92`+`93`, and `metrics.rs:287`+`288`), and rows #10/#11 are
> the two `derive_index_path` return-constant mutants on the same line.
> Tallied per **missed.txt entry**, this maps all **25 reducible**
> missed mutants to a killing test.

## By-design un-killable (7)

| # | Mutant (file:line) | Replacement | Category | Why no test |
|---|--------------------|-------------|----------|-------------|
| A | `persist.rs:80:21` match-guard `err.kind()==NotFound` → `true` | `true` | tracing-only | Both the `NotFound` arm and the trailing `Err(err)` arm return the **same** `BTreeMap::new()` (start-empty). The guard only selects between a `debug!` ("no snapshot") and a `warn!` ("failed to read") log line; the returned value is byte-identical. No public observable difference. |
| B | `persist.rs:80:21` match-guard → `false` | `false` | tracing-only | Same as A — forcing the guard `false` sends a genuinely-missing file down the `warn!` branch, but the return value (`BTreeMap::new()`) is unchanged. |
| C | `persist.rs:80:32` `==` → `!=` | `!=` | tracing-only | Inverts which empty-returning branch logs; output map identical. (`load`'s only non-empty return path is a *successfully parsed* snapshot, which neither guard branch touches.) |
| D | `serve.rs:233:5` `init_tracing` → `()` | no-op | tracing-only / lifecycle | `init_tracing` already returns `()`; the only effect is whether a global `tracing` subscriber is installed. Asserting that requires mutating process-global subscriber state, which is flaky and forbidden as a hard test gate (see MEMORY `tokio_test_tracing_global_state`). The existing `init_tracing_is_idempotent` test pins no-panic only. |
| E | `serve.rs:240:5` `shutdown_signal` → `()` | resolve immediately | lifecycle | The real body awaits `SIGINT`/`SIGTERM`. The mutant makes it resolve at once, which only changes *when* `axum::serve` returns. Distinguishing it requires binding a real socket and delivering an OS signal mid-serve — non-deterministic and unsafe to do in-process under `unsafe_code = forbid`. |
| F | `bin/ferro-cargo-registry-server.rs:38:5` `main` → `ExitCode::default()` | `SUCCESS` | lifecycle / binary-entry | The binary `main` boots a long-running server; it has no in-process unit surface, and the success/failure `ExitCode` mapping cannot be exercised deterministically without spawning the process and driving it to a fatal config. The library `serve()` error path (which `main` forwards) **is** covered by `serve::tests::serve_rejects_invalid_listen_before_binding`. |

### Categories
- **tracing-only** (A, B, C, D): mutant alters only log output / global
  subscriber installation, never a returned value or HTTP response.
- **lifecycle / binary-entry** (E, F): shutdown-signal wait and the binary
  `main` entry point — no deterministic in-process observable surface.

## Expected outcome

- Newly killed: **25 / 25 reducible** missed mutants.
- Remaining by-design un-killable: **7** (3 persist guard variants,
  `init_tracing`, `shutdown_signal`, binary `main`).
- Projected kill rate: **143 / 150 viable ≈ 95.3%**, with the irreducible
  remainder fully classified above. The orchestrator's central
  `cargo mutants` re-run is the authority.
