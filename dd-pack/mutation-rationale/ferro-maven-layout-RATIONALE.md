<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-maven-layout mutation-kill rationale

Baseline before this wave: **80.6% kill (25 missed of 129 viable)**.
Source of truth: `dd-pack/mutation/ferro-maven-layout/mutants.out/missed.txt`.

Scope of this wave: **tests only** under `crates/ferro-maven-layout/**`
(`#[cfg(test)]` modules + the `tests/http_roundtrip.rs` integration
harness). No library logic was changed; no version bump.

Of the 25 previously-missed mutants, **23 now have a dedicated killing
test** that asserts the exact observable behaviour the mutant breaks
(concrete values, not `is_ok()`). **2 are classified by-design
un-killable** (a single log-cosmetic function, both replacement
variants) — see the by-design section.

## Mutant → killing test map

### checksum.rs

| Mutant (file:line) | Replacement | Killing test | Why it kills |
|--------------------|-------------|--------------|--------------|
| `checksum.rs:37` | delete arm `"md5"` | `checksum::tests::from_extension_maps_every_known_algo` | Asserts `from_extension("md5") == Some(Md5)`. Deleting the arm makes it fall to `None`. |
| `checksum.rs:39` | delete arm `"sha256"` | same | Asserts `from_extension("sha256") == Some(Sha256)`. |
| `checksum.rs:40` | delete arm `"sha512"` | same | Asserts `from_extension("sha512") == Some(Sha512)`. |

(`extension_round_trips_with_from_extension` and `hex_len_matches_each_algo`
add belt-and-braces coverage for the sibling `extension`/`hex_len` arms.)

### error.rs (`http` feature, `storage_status`)

| Mutant (file:line) | Replacement | Killing test | Why it kills |
|--------------------|-------------|--------------|--------------|
| `error.rs:72` | delete arm `BlobStoreError::NotFound(_)` | `error::tests::storage_not_found_maps_to_404` | Builds a `Storage(NotFound(..))` and asserts `status() == 404`. Deleting the arm falls to the `_ => 500` catch-all. |
| `error.rs:73` | delete arm `DigestMismatch{..} \| InvalidDigest(_)` | `error::tests::storage_digest_mismatch_maps_to_400` + `storage_invalid_digest_maps_to_400` | Asserts both variants map to `400`. Deleting the arm falls to `500`. The `InvalidDigest` value is built from a real `"not-a-digest".parse::<Digest>()` parse error. |

(`storage_io_maps_to_500` pins the catch-all arm so the deletions are
distinguishable from it, and `coordinate_and_pom_and_metadata_and_checksum_map_to_400`
covers the sibling `BAD_REQUEST` arms of `MavenError::status`.)

### layout.rs

| Mutant (file:line) | Replacement | Killing test | Why it kills |
|--------------------|-------------|--------------|--------------|
| `layout.rs:65` | `<` → `<=` (`segments.len() < 3`) | `layout::tests::three_segment_metadata_path_is_accepted` | A 3-segment metadata path (`com/example/maven-metadata.xml`) must parse OK. `<= 3` rejects it at the top-level guard. |
| `layout.rs:89` | `<` → `==` (`segments.len() < 4`) | `layout::tests::three_segment_artifact_path_is_rejected` | A 3-segment artifact path must fail with the exact "fewer than 4 segments" message. `== 4` (3≠4) lets it slip through to a different failure. |
| `layout.rs:89` | `<` → `<=` | `layout::tests::four_segment_artifact_path_is_accepted` | A minimal 4-segment artifact path (`g/foo/1.0/foo-1.0.jar`) must parse OK. `<= 4` rejects it. |
| `layout.rs:120` | `<` → `==` (in `classify_metadata`) | `three_segment_metadata_path_is_accepted` | The 3-segment metadata path reaches `classify_metadata`; `== 3` would error instead of classifying. |
| `layout.rs:120` | `<` → `<=` | same | `<= 3` (3≤3) likewise errors on the valid 3-segment metadata path. |
| `layout.rs:131` | `-` → `/` (`before_file.len() - 2` as index) | `layout::tests::deep_version_level_metadata_resolves_group_and_artifact` | A 7-segment version-level path makes `before_file.len()==6`, so `len-2=4 ≠ len/2=3`. Asserts `artifact_id=="foo"`; the `/` mutant picks `before_file[3]=="d"`. |
| `layout.rs:132` | `-` → `/` (`before_file[..len-2]`) | same | Asserts `group_id=="a.b.c.d"`; the `/` mutant slices `before_file[..3]=="a.b.c"`. |
| `layout.rs:233` | `\|\|` → `&&` (`classifier.is_empty() \|\| extension.is_empty()`) | `layout::tests::empty_classifier_with_extension_is_rejected` | `foo-1.0-.jar` yields an empty classifier + non-empty extension. `\|\|` rejects it; `&&` (only one operand true) would accept an empty classifier. |
| `layout.rs:243` | `layout_is_snapshot` → `false` | `layout::tests::layout_is_snapshot_reflects_version` | Asserts `true` for a `-SNAPSHOT` path. Constant `false` fails. |
| `layout.rs:243` | `layout_is_snapshot` → `true` | same | Asserts `false` for a release path. Constant `true` fails. |

### metadata.rs

| Mutant (file:line) | Replacement | Killing test | Why it kills |
|--------------------|-------------|--------------|--------------|
| `metadata.rs:216` | `\|\|` → `&&` (`group_id.is_empty() \|\| artifact_id.is_empty()`) | `metadata::tests::missing_artifact_id_is_rejected` + `missing_group_id_is_rejected` | Each test supplies exactly one of the two fields. `\|\|` rejects (the other is empty); `&&` would accept since only one operand is true. `both_group_and_artifact_present_is_accepted` pins the passing direction so the `&&` mutant is not vacuously satisfied. |

### handlers.rs (`http` feature, integration tests in `tests/http_roundtrip.rs`)

| Mutant (file:line) | Replacement | Killing test | Why it kills |
|--------------------|-------------|--------------|--------------|
| `handlers.rs:63` | `handle_head` → `Ok(Default::default())` | `head_returns_get_headers_without_body` + `head_missing_is_404` | The default `Response` is a header-less 200. The first test asserts HEAD carries the real `Content-Type`/`Content-Length`; the second asserts HEAD of a missing path is 404, not 200. |
| `handlers.rs:182` | delete arm `"pom" \| "xml"` | `artifact_content_types_per_extension` | Asserts `.pom` and `.xml` GETs return `application/xml`. Deleting the arm falls to `application/octet-stream`. |
| `handlers.rs:183` | delete arm `"jar" \| "war" \| "ear"` | same | Asserts `application/java-archive` for each of the three. |
| `handlers.rs:184` | delete arm `"tar.gz" \| "tgz"` | same | Asserts `application/gzip` for both. |
| `handlers.rs:185` | delete arm `"zip"` | same | Asserts `application/zip`. |
| `handlers.rs:489` | `==` → `!=` (`values().any(\|d\| d == &digest)`) | `delete_keeps_blob_shared_by_another_path` | Two paths share one blob; deleting one must leave the blob (GET of the survivor returns 200 + bytes). `!=` flips the reference count so `still_referenced` becomes false → the shared blob is wrongly deleted → survivor GET fails. |
| `handlers.rs:490` | delete `!` (`if !still_referenced`) | same | Without the `!`, `if still_referenced` (true, because the second path references it) deletes the shared blob → survivor GET fails. |

## By-design un-killable

| Mutant (file:line) | Replacement | Category | Reason |
|--------------------|-------------|----------|--------|
| `handlers.rs:41` | `snapshot_metadata_path` → `String::new()` | log-cosmetic / unobservable | The returned string is bound to `md_path` (handlers.rs:384) and used **only** as a `tracing::debug!(%md_path, ...)` field at line 399. The actual metadata cache key inserted at lines 390–398 is built from an independent `(repo, group_path, artifact_id, Some(base))` tuple, not from this string. No response header, body, status, or persisted state depends on it. |
| `handlers.rs:41` | `snapshot_metadata_path` → `"xyzzy".into()` | log-cosmetic / unobservable | Same call site, same reasoning — only the debug-log field changes. Killing it would require capturing `tracing` subscriber output for a purely diagnostic string, which asserts on log formatting rather than protocol behaviour. |

Both variants are the same single function whose result never escapes a
debug log; classifying them by-design avoids a brittle log-capture test
(cf. the `#[tokio::test]` + global tracing-subscriber flake hazard noted
in project memory).

## Expected outcome

- Expected newly killed: **23 / 25**.
- Expected remaining by-design un-killable: **2 / 25** (the two
  `snapshot_metadata_path` log-cosmetic variants).
- Projected kill rate: **127 / 129 viable ≈ 98.4%** (the 2 by-design
  mutants are unobservable, so 100% of the *behaviourally observable*
  remainder is killed). The orchestrator's central `cargo mutants`
  re-run is the authority.
