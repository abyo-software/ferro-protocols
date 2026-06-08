<!-- SPDX-License-Identifier: Apache-2.0 -->
# Due-Diligence Review Rounds — ferro-protocols

Adversarial DD review history for the `ferro-protocols` workspace ahead of
v1.0.0 GA. Reviews are run with **Codex CLI (`gpt-5.5`, reasoning effort
high)** in a read-only sandbox, wide-scope across the public API layer,
internal logic, and fuzz/robustness gaps. Each round's prompt is preserved
in `dd-pack/codex-rN-prompt.md`; raw transcripts in the session log.

A finding is **closed** only when a regression test (review → failing test →
fix) demonstrates the bug and then passes. Severity scale: P0 (memory unsafety
/ RCE / auth bypass / trivial-input panic-DoS), P1 (likely-exploitable DoS,
spec violation causing corruption, significant correctness bug), P2
(hardening, edge-case correctness, spec/doc drift).

## Round 1 (2026-06-08) — wide-scope, 6 crates + new server surface

**Result: P0=0, P1=5, P2=4. All 9 fixed with regression tests.**

| # | Sev | Location | Finding | Closure |
|---|-----|----------|---------|---------|
| F1 | P1 | oci `handlers/manifest.rs` | PUT-by-digest never verified URL digest == sha256(body) | `44282db` digest equality check + 2 tests |
| F2 | P1 | oci `upload.rs` / `registry.rs` | chunked uploads unbounded `BytesMut` → memory DoS | `3f68337` per-session size cap (4 GiB) + drop-on-overflow + test |
| F3 | P1 | oci `router.rs` / cargo `handlers.rs` | no explicit `DefaultBodyLimit`; Axum 2 MiB default rejects ≥2 MiB manifests / `.crate` | `3f68337` (oci 512 MiB on `/v2/**`) + `9c6dbba` (cargo 20 MiB on publish) + tests |
| F4 | P1 | cargo `index.rs` | publish metadata deserialized straight into `IndexEntry` (wrong `version_req`→`req`, rename, `cksum`) | `89dfa49` `PublishManifest`→`IndexEntry` conversion + tests |
| F5 | P1 | cargo `handlers.rs` | records keyed by original-case name, index paths lowercase → mixed-case fetch 404; no `-`/`_`/case collision guard | `d4d7aef` `canonical_name` keying on all paths + 409 collision + tests |
| F6 | P2 | oci `handlers/blob_upload.rs` | PATCH ignored `Content-Range` end / body-length | `3f68337` require `range.length()==body.len()` else 416 + test |
| F7 | P2 | oci+cargo `metrics.rs` | `/metrics` merged after middleware → not self-instrumented | `bcdd730` (oci merge-then-layer) — **cargo missed, reopened R2 #3** |
| F8 | P2 | oci `metrics.rs` | each scrape did O(blobs) `store.list()` | `bcdd730` O(1) `AtomicI64` blob gauge inc/dec on put/delete |
| F9 | P2 | maven `metadata.rs` | `maven-metadata.xml` parse not panic-shielded, no fuzz target | `852d2b7`+`48d33d0` shared `from_str_panic_safe` + `parse_metadata_xml` fuzz target |

## Round 2 (2026-06-08) — verify R1 fixes + fresh wide-scope on new code

**Result: PART A = 5 confirmed-fixed, 4 residual. PART B = 5 new.
ACTIONABLE TOTAL: P0=0, P1=3, P2=6.**

Confirmed-fixed: F2, F3, F4, F5, F9. Residual: F1 (partial), F6, F7, F8.

| # | Sev | Location | Finding | Status |
|---|-----|----------|---------|--------|
| R2-1 | P1 | oci `manifest.rs:336` | F1 residual: digest check only fires when `declared.algo()==sha256`; `sha512:<valid-len-hex>` reference bypasses → 201 for arbitrary bytes | fixing |
| R2-2 | P2 | oci `upload.rs:139` | F6 residual: `ContentRange::length()` = `end-start+1` overflows on `0-u64::MAX` (debug panic / release wrap-to-0) | fixing |
| R2-3 | P2 | cargo `metrics.rs:325` | F7 residual: cargo `/metrics` still merged after middleware (oci was fixed, cargo wasn't); docs claim every request recorded | fixing |
| R2-4 | P2 | oci `blob_upload.rs:127` | F8 residual: `storage_blobs` gauge double-increments on duplicate-digest PUT; drifts above true distinct count | fixing |
| R2-5 | P1 | cargo `handlers.rs:240` | publishing an existing `(name, vers)` overwrites the tarball+entry — violates cargo version immutability | fixing |
| R2-6 | P1 | oci+cargo `serve.rs` | reference binaries keep registry metadata/index in memory; FS holds blobs/tarballs but manifests/tags/index lost on restart = data loss | fixing (durable metadata persistence or honest ephemeral scoping) |
| R2-7 | P2 | oci `registry.rs:278` | upload sessions have no TTL / global count / global byte cap (F2 bounds one session only) | fixing |
| R2-8 | P2 | cargo `handlers.rs:209` | publish writes tarball before collision/duplicate checks → orphan blobs on rejected publish | fixing |
| R2-9 | P2 | cargo `serve.rs:160` | default `config.json` advertises `http://0.0.0.0:8081` (wildcard / port-0 unusable by remote clients) | fixing |

All 9 fixed (oci `c290144`/`d67ef2b`/`c8f9404`/`3f68337`/`d725481`/`5ccb00a`/`b03bdf1`; cargo `f2d82b5`/`eaafdbd`/`b42b8c9`/`b4fd33b`/`c49e764`; maven via shared helper). The R2 fixes introduced **durable metadata persistence** for both server binaries (previously in-memory only) — a new feature that closed the data-loss-on-restart gap but opened a new trust boundary reviewed in R3+.

## Round 3 (2026-06-08) — verify R2 + scan new persistence code

**PART A: 6 confirmed, 3 residual. PART B: 5 new. ACTIONABLE: P0=0, P1=2, P2=3.** All fixed.

| # | Sev | Finding | Closure |
|---|-----|---------|---------|
| R3-1 | P1 | oci `metadata.json` load trusted digest key without recomputing → crafted snapshot serves arbitrary bytes under a digest | `b03bdf1` recompute sha256 on load, drop mismatched entries |
| R3-2 | P1 | persistence failures swallowed → acknowledged mutations lost on restart (oci+cargo) | `b03bdf1`+`c49e764` persist returns Result; handlers roll back in-memory (+delete orphan blob) and return 500 |
| R3-3 | P2 | concurrent same-digest upload overcounts `storage_blobs` gauge | `921b35f` serialize contains→put→inc under a mutex |
| R3-4 | P2 | cargo load kept invalid-cksum entries (undownloadable, blocks republish) | `8c7991c` drop corrupt versions on load |
| R3-5 | P2 | snapshot temp write not crash-durable + symlink-unsafe | `b03bdf1`+`75bea2a` O_EXCL unique temp + sync_all + rename + parent-dir fsync |

## Round 4 (2026-06-08) — verify R3 + final pass on rollback/durability

**PART A: 4 confirmed, 1 residual. PART B: 3. ACTIONABLE: P0=0, P1=2, P2=1.** All fixed.

| # | Sev | Finding | Closure |
|---|-----|---------|---------|
| R4-1 | P1 | oci manifest+tag persisted separately from referrer → partial state if 2nd persist fails | `5d184f9` single `put_manifest_with_referrer` transaction (one lock, one snapshot, full rollback) |
| R4-2 | P2 | manifest-PUT rollback left empty repo/tag maps (repo appears in catalog) | `5d184f9` prune empty maps on rollback |
| R4-3 | P1 | cargo publish-rollback orphan-delete TOCTOU (concurrent identical publish → deletes referenced blob) | `892ac54` hold index write lock across reference-check + blob delete |

## Round 5 (2026-06-08) — final GA-gate sweep

**PART A: 3 confirmed, 0 residual. PART B: 3 (the wide sweep generalized known patterns to maven). ACTIONABLE: P0=0, P1=1, P2=2.** All fixed.

| # | Sev | Finding | Closure |
|---|-----|---------|---------|
| R5-1 | P1 | maven `handle_delete` had the SAME check-delete TOCTOU as cargo R4-3 | `8486e56` hold layout write guard across remove + ref-scan + delete |
| R5-2 | P2 | maven PUT used implicit Axum 2 MiB default body limit | `8989133` explicit 256 MiB `DefaultBodyLimit` |
| R5-3 | P2 | oci `delete_manifest` left dangling referrer descriptors | `0db4838` prune matching descriptors (all subjects) under delete transaction + rollback |

## Round 6 (2026-06-08) — convergence verification

Wide sweep confirming the recurring patterns (check-delete TOCTOU, missing body limit, non-atomic persist, content-addressing on load) are closed across all 6 crates. _Verdict recorded on completion._

## Convergence summary

| Round | Findings | P1 | P2 |
|-------|----------|----|----|
| R1 | 9 | 5 | 4 |
| R2 | 9 | 3 | 6 |
| R3 | 5 | 2 | 3 |
| R4 | 3 | 2 | 1 |
| R5 | 3 | 1 | 2 |

Monotonically converging. Every finding closed with a review→failing-test→fix regression test (each fix verified to fail before / pass after). 29 findings total across 5 rounds, 0 left open at R5. The DD process found real shipped bugs the original extracted crates carried (digest-verification bypass, unbounded-upload DoS, missing body limits, mixed-case crate 404s, version-overwrite) plus every issue in the durability feature added mid-review.
