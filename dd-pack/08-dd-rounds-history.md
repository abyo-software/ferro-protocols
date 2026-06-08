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

_Round 3 (verification) pending after R2 fixes land._
