<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-oci-server mutation-kill rationale

Baseline before this wave: **81.5% kill (54 missed of 292 viable)**.
Source of truth: `dd-pack/mutation/ferro-oci-server/mutants.out/missed.txt`.

Scope of this wave: **tests only** under
`crates/ferro-oci-server/src/**` (`#[cfg(test)]` modules) and
`crates/ferro-oci-server/tests/**`. No library / source logic was changed
(test-only helpers only); the OCI conformance suite and existing tests
stay green.

Of the 54 previously-missed mutants, **50 now have a dedicated killing
test** that asserts the exact observable behaviour the mutant breaks
(concrete values / HTTP status codes / OCI error-code JSON / header
values / gauge values, never `is_ok()`). The remaining **4 are classified
by-design un-killable** — each is a binary/lifecycle entry point, a
tracing-only side effect, or reachable only by mutating process-global
environment state (forbidden under `unsafe_code = forbid`).

## Mutant → killing test map (50 killed)

| # | Mutant (file:line) | Replacement | Killing test | Why it kills |
|---|--------------------|-------------|--------------|--------------|
| 1 | `media_types.rs:67:56` `&&` → `\|\|` in `classify_manifest_media_type` | `\|\|` | `media_types::tests::vnd_prefix_without_json_suffix_is_not_artifact` | A `vnd.*` media type NOT ending `+json` (tar.gzip) must be `None`; `\|\|` would classify it Artifact. A non-`vnd` `+json` type is also `None`, pinning both operands. |
| 2 | `reference.rs:44:19` `>` → `>=` in `validate_name` | `>=` | `reference::tests::name_at_exact_max_length_is_valid_over_is_rejected` | A 255-char name (exactly `MAX_NAME_LENGTH`) must be valid; `>=` rejects it. 256 still invalid. |
| 3 | `reference.rs:81:13` `<` → `==` in `validate_component` | `==` | `reference::tests::internal_invalid_separator_is_rejected` | `foo!bar` must be rejected; with `i == bytes.len()` the component-walk body never runs and the bad `!` separator is never checked → wrongly accepted. |
| 4 | `reference.rs:81:13` `<` → `>` in `validate_component` | `>` | same as #3 | `i > bytes.len()` is never true at the start → loop body never runs → `foo!bar` wrongly accepted. |
| 5 | `reference.rs:89:17` `<` → `<=` in `validate_component` | `<=` | same as #3 | The separator-scan inner loop indexes `bytes[i]`; `<=` reads `bytes[len]` (panic) or changes the scanned run, diverging from the valid-name baseline. `foo!bar` rejection + `foobar` acceptance pin it. |
| 6 | `reference.rs:107:5` `is_valid_separator` → `true` | `true` | same as #3 | Always-true accepts the `!` separator → `foo!bar` wrongly valid. |
| 7 | `reference.rs:111:19` `&&` → `\|\|` in `is_valid_separator` | `\|\|` | `reference::tests::mixed_dash_dot_separator_run_is_rejected` | Separator run `-.` is rejected by `!empty && all(=='-')`; `\|\|` makes `!empty` alone accept it. `foo-.bar` invalid + `foo--bar` valid pin it. |
| 8 | `reference.rs:118:5` `is_valid_tag` → `true` | `true` | `reference::tests::tag_starting_with_dot_is_rejected` | `.bad` (leading `.`) must be rejected; always-true accepts it. |
| 9 | `reference.rs:118:23` `\|\|` → `&&` in `is_valid_tag` | `&&` | `reference::tests::empty_tag_is_rejected` | `tag.is_empty() \|\| len>MAX`; `&&` stops the empty-string short-circuit, then `bytes[0]` panics on empty. Empty reference must reject. |
| 10 | `reference.rs:118:36` `>` → `==` in `is_valid_tag` | `==` | `reference::tests::tag_at_exact_max_length_boundary` | 128-char tag (`MAX_TAG_LENGTH`) valid, 129 invalid; `==` would reject the valid 128. |
| 11 | `reference.rs:118:36` `>` → `>=` in `is_valid_tag` | `>=` | same as #10 | `>=` rejects the valid 128-char tag. |
| 12 | `reference.rs:122:65` `==` → `!=` in `is_valid_tag` | `!=` | `reference::tests::tag_starting_with_underscore_is_valid` | First char `_` is allowed (`bytes[0] == b'_'`); `!=` rejects `_internal`. |
| 13 | `reference.rs:150:9` `Reference::is_digest` → `true` | `true` | `reference::tests::is_digest_and_as_digest_distinguish_variants` | A `Tag` must report `is_digest() == false`; always-true fails it. |
| 14 | `reference.rs:156:9` `Reference::as_digest` → `None` | `None` | same as #13 | A `Digest` reference must yield `Some(digest)` that round-trips; constant `None` fails. |
| 15 | `registry.rs:45:73` `*` → `+` (`60 * 60` in `DEFAULT_UPLOAD_SESSION_TTL`) | `+` | `registry::tests::default_session_ttl_is_one_hour` | TTL must equal `Duration::from_secs(3600)`; `+` gives 120s. |
| 16 | `registry.rs:45:73` `*` → `/` | `/` | same as #15 | `/` gives 1s, not 3600. |
| 17 | `registry.rs:321:23` match-guard `e.kind()==NotFound` → `true` in `load_snapshot` | `true` | `registry::tests::load_snapshot_non_notfound_io_error_is_err` | Reading a directory yields a non-`NotFound` IO error that must propagate as `Err`; always-true swallows EVERY error into `Ok(None)`. |
| 18 | `registry.rs:321:23` match-guard → `false` | `false` | `registry::tests::load_snapshot_missing_file_is_ok_none` | A missing file must be `Ok(None)`; always-false routes `NotFound` to `Err`. |
| 19 | `registry.rs:321:32` `==` → `!=` in `load_snapshot` | `!=` | both #17 & #18 | `!=` swaps the arms: `NotFound` → `Err`, real errors → `Ok(None)`. The missing-file test (must be `Ok(None)`) and the directory test (must be `Err`, non-NotFound) jointly pin it. |
| 20 | `registry.rs:424:16` `-` → `+` in `sweep_idle_uploads` | `+` | `registry::tests::sweep_idle_uploads_returns_count_swept` | Sweeping 2 idle sessions must return swept-count `2` (`before - after`); `+` returns `before + after = 4`. |
| 21 | `registry.rs:492:45` `!=` → `==` in `delete_manifest` | `==` | `registry::tests::delete_by_digest_drops_only_tags_pointing_at_it` | `tag_map.retain(\|_, v\| v != digest)` keeps tags NOT pointing at the deleted digest; `==` keeps ONLY those pointing at it, dropping the unrelated tag. Asserts unrelated tag survives. |
| 22 | `registry.rs:518:41` `>` → `==` in `list_tags` | `==` | `registry::tests::list_tags_last_cursor_is_strictly_after` | tags `[a,b,c]`, `last="b"` ⇒ `[c]`; `==` ⇒ `[]`. |
| 23 | `registry.rs:518:41` `>` → `<` in `list_tags` | `<` | same as #22 | `<` ⇒ `[a]`, not `[c]`. |
| 24 | `registry.rs:518:41` `>` → `>=` in `list_tags` | `>=` | same as #22 | `>=` ⇒ `[b,c]`, not `[c]`. |
| 25 | `registry.rs:532:41` `>` → `==` in `list_repositories` | `==` | `registry::tests::list_repositories_last_cursor_is_strictly_after` | repos `[repo-a,repo-b,repo-c]`, `last="repo-b"` ⇒ `[repo-c]`; `==` ⇒ `[]`. |
| 26 | `registry.rs:532:41` `>` → `<` in `list_repositories` | `<` | same as #25 | `<` ⇒ `[repo-a]`. |
| 27 | `registry.rs:532:41` `>` → `>=` in `list_repositories` | `>=` | same as #25 | `>=` ⇒ `[repo-b,repo-c]`. |
| 28 | `registry.rs:588:9` `complete_upload` → `Ok(())` | no-op | `registry::tests::complete_upload_removes_the_session` | `complete_upload` must remove the session; the constant `Ok(())` skips the `uploads.remove`, so `get_upload_state` would still resolve. Asserts it is `None` afterward. |
| 29 | `router.rs:150:9` `AppState::blob_count` → `0` | `0` | `app_state_tests::blob_count_reflects_increments_and_saturating_decrements` | After 2 inc + 1 dec the count must be `1` (and `2` mid-way); a constant `0` fails. |
| 30 | `router.rs:150:9` `AppState::blob_count` → `1` | `1` | same as #29 | The fresh-state `0` and post-inc `2` assertions both differ from constant `1`. |
| 31 | `router.rs:150:9` `AppState::blob_count` → `-1` | `-1` | #29 + `app_state_tests::blob_count_saturates_at_zero_never_negative` | The count is never negative (saturating dec) and reaches `2`; constant `-1` fails both. |
| 32 | `serve.rs:121:5` `build_app_persisted` → `Default::default()` | empty `Router` | `serve::tests::build_app_persisted_serves_v2_and_survives_restart` | An empty default router 404s `/v2/` and persists nothing; the test asserts `/v2/` is 200, a manifest PUT is 201, and the manifest HEAD-resolves after a rebuilt persisted app — impossible for a default router. |
| 33 | `upload.rs:148:18` `>` → `>=` in `ContentRange::parse` | `>=` | `upload::tests::equal_start_end_is_a_valid_single_byte_range` | `5-5` is a valid single byte (length 1); `>=` rejects equal bounds. `6-5` stays rejected. |
| 34 | `blob_upload.rs:49:38` `>` → `>=` in `would_exceed_cap` | `>=` | `handlers::blob_upload::tests::would_exceed_cap_boundary_is_inclusive_under_strict_greater` | At `current+incoming == cap` (exact fit) it must NOT exceed (`>` false); `>=` rejects the exact-fit chunk. Under/over cases pin the rest. |
| 35 | `blob_upload.rs:118:26` `==` → `!=` in `init_upload` | `!=` | `monolithic_upload_digest_mismatch_returns_400` (existing) + `monolithic_upload_matching_digest_is_created` (new) | The integrity check `actual.algo()==digest.algo() && hex!=`; `!=` (algos always sha256) skips verification and accepts mismatched bytes. The mismatch test asserts 400; the matching test asserts 201 + byte round-trip. |
| 36 | `blob_upload.rs:320:8` delete `!` in `finish_upload` | `if body.is_empty()` | `chunked_finish_appends_final_chunk_body` | The whole blob is delivered as the final-PUT body; dropping the `!` skips appending it, so the recomputed digest mismatches and the PUT fails. Asserts 201 + exact byte round-trip via GET. |
| 37 | `blob_upload.rs:350:24` `==` → `!=` in `finish_upload` | `!=` | `chunked_finish_with_wrong_digest_returns_400` | Finalize digest check; `!=` skips verification (both sha256) and accepts wrong bytes. Asserts 400 `DIGEST_INVALID`. |
| 38 | `blob_upload.rs:365:8` delete `!` in `finish_upload` | `if already_present` | `chunked_finish_increments_blob_gauge_for_new_blob` | A brand-new blob finished via chunked PUT must increment the distinct-blobs gauge to 1; dropping the `!` increments only for duplicates, leaving the gauge at 0. Asserts `ferrooci_storage_blobs 1` on `/metrics`. |
| 39 | `metrics.rs:74:9` `<impl Debug for Metrics>::fmt` → `Ok(Default::default())` | empty output | `metrics::tests::metrics_debug_impl_names_the_struct` | `{:?}` must contain `"Metrics"`; the mutant writes nothing. |
| 40 | `metrics.rs:171:13` delete arm `Some("/live")` in `handler_for` | (arm removed) | `metrics::tests::handler_for_probe_routes_are_labelled` | `/live` → `"live"`; the path tail matches no suffix keyword, so a deleted arm degrades to `"other"`. |
| 41 | `metrics.rs:172:13` delete arm `Some("/ready")` in `handler_for` | (arm removed) | same as #40 | `/ready` → `"ready"`; deleted arm → `"other"`. |
| 42 | `metrics.rs:278:9` delete arm `Method::HEAD` in `method_label` | (arm removed) | `metrics::tests::method_label_maps_every_known_method` | `HEAD` → `"HEAD"`; deleted folds into `_ => "OTHER"`. |
| 43 | `metrics.rs:279:9` delete arm `Method::POST` | (arm removed) | same as #42 | `POST` → `"POST"`. |
| 44 | `metrics.rs:280:9` delete arm `Method::PUT` | (arm removed) | same as #42 | `PUT` → `"PUT"`. |
| 45 | `metrics.rs:281:9` delete arm `Method::PATCH` | (arm removed) | same as #42 | `PATCH` → `"PATCH"`. |
| 46 | `metrics.rs:282:9` delete arm `Method::DELETE` | (arm removed) | same as #42 | `DELETE` → `"DELETE"`. |
| 47 | `metrics.rs:283:9` delete arm `Method::OPTIONS` | (arm removed) | same as #42 | `OPTIONS` → `"OPTIONS"`. |
| 48 | `manifest.rs:128:36` `>` → `>=` in `range_or_full_response` | `>=` | `handlers::manifest::tests::range_request_for_last_byte_is_partial_not_unsatisfiable` | Requesting the LAST byte (`start == last`) must be 206; `>=` makes `start > last` true → 416. A `start == total` request stays 416. |
| 49 | `manifest.rs:132:41` `-` → `+` in `range_or_full_response` | `+` | `range_request_for_last_byte_is_partial_not_unsatisfiable` + `multi_byte_range_content_length_is_exact` | `slice_len = clamped_end - start + 1` drives `Content-Length`; `+` gives `clamped_end + start + 1`. Asserts CL `1` for `4-4` and `6` for `2-7`. |
| 50 | `manifest.rs:220:14` `>` → `>=` in `parse_byte_range` | `>=` | `handlers::manifest::tests::parse_byte_range_equal_bounds_is_satisfiable` | `bytes=5-5` must parse as `Range{5,5}` (satisfiable); `>=` makes equal bounds `Unsatisfiable`. `bytes=6-5` stays `Unsatisfiable`. |

> Counting note: rows tally per **missed.txt entry**, so the multi-line
> operator groups (e.g. `reference.rs:81:13` appears twice for `== ` and
> `>`; `registry.rs:518:41` / `532:41` each have three; `router.rs:150:9`
> three) are listed as separate rows, mapping all **50 reducible** missed
> mutants to a killing test.

## By-design un-killable (4)

| # | Mutant (file:line) | Replacement | Category | Why no test |
|---|--------------------|-------------|----------|-------------|
| A | `bin/ferro-oci-server.rs:39:5` `main` → `Ok(())` | success | lifecycle / binary-entry | The `#[tokio::main]` entry boots a long-running server; it has no in-process unit surface and its `Ok`/`Err` forwarding cannot be exercised deterministically without spawning the process and driving it to a fatal config. The library error path it forwards (`serve()`) **is** covered by `serve::tests::serve_rejects_invalid_listen_before_binding`. |
| B | `serve.rs:46:72` delete `!` in `Config::from_env` (`filter(\|v\| !v.is_empty())`) | invert empty-filter | env-mutation-only | This branch reads `std::env::var_os(ENV_STORAGE_DIR)`. Distinguishing the mutant requires setting that process-global env var to an empty string, and `set_var` is `unsafe` — forbidden by the crate's `unsafe_code = forbid`. The pure-logic equivalent (`Config::from_raw`, which applies the same empty→in-memory normalisation) is fully tested by `from_raw_treats_empty_storage_dir_as_inmemory`. |
| C | `serve.rs:173:5` `init_tracing` → `()` | no-op | tracing-only / lifecycle | `init_tracing` already returns `()`; its only effect is installing a global `tracing` subscriber. Asserting installation requires mutating process-global subscriber state, which is flaky and a known test-isolation hazard (MEMORY `tokio_test_tracing_global_state`). The existing `init_tracing_is_idempotent` test pins no-panic only. |
| D | `serve.rs:183:5` `shutdown_signal` → `()` | resolve immediately | lifecycle | The real body awaits `SIGINT`/`SIGTERM`; the mutant only changes *when* `axum::serve` returns. Distinguishing it requires binding a real socket and delivering an OS signal mid-serve — non-deterministic and not expressible in-process under `unsafe_code = forbid`. |

### Categories
- **lifecycle / binary-entry** (A, D): the binary `main` and the
  shutdown-signal wait have no deterministic in-process observable.
- **env-mutation-only** (B): reachable only by mutating process-global
  environment, which `unsafe_code = forbid` blocks; the pure-logic twin
  (`from_raw`) is tested instead.
- **tracing-only** (C): mutant alters only global subscriber installation,
  never a returned value or HTTP response.

## Expected post-wave kill rate

Killing 50 of 54 previously-missed (the remaining 4 by-design):
`(292 - 54 + 50) / 292 = 288 / 292 ≈ 98.6%`, with the 4 residual mutants
documented above as irreducible. (Orchestrator re-runs `cargo mutants` to
confirm the actual figure.)
