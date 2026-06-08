<!-- SPDX-License-Identifier: Apache-2.0 -->
# OCI Distribution Spec v1.1 — Conformance Results

This file records the **real** result of running the official
[`opencontainers/distribution-spec`](https://github.com/opencontainers/distribution-spec/tree/main/conformance)
conformance suite against the `ferro-oci-server` binary. It is
produced by [`run_conformance.sh`](./run_conformance.sh); re-run that
script to refresh the numbers.

## Latest run

| Field | Value |
|---|---|
| Date | 2026-06-08 |
| Suite | `opencontainers/distribution-spec` conformance `v1.1.0` (prebuilt image `ghcr.io/opencontainers/distribution-spec/conformance:v1.1.0`) |
| Server under test | `ferro-oci-server` binary, FS-backed blob store + `InMemoryRegistryMeta` |
| Workflows enabled | Push, Pull, Content Discovery, Content Management (all four) |
| **Result** | **75 Passed / 0 Failed / 5 Skipped** (75 of 80 specs ran) |
| Suite exit code | `0` (SUCCESS) |
| Runner | Docker (prebuilt conformance image), `--network host` |

JUnit summary line from the run:

```
testsuites tests="81" disabled="5" errors="0" failures="0"
```

### Per-workflow status

All four conformance workflow categories pass:

- **Push** — blob (monolithic + chunked) and manifest push. ✅
- **Pull** — blob/manifest GET + HEAD, 404 paths. ✅
- **Content Discovery** — tag listing, referrers API + `artifactType`
  filter, image-index references. ✅
- **Content Management** — manifest/blob delete, teardown. ✅

### About the 5 skipped specs

The 5 skipped specs are the suite's own optional/teardown cases that
it disables unless extra environment toggles are set (e.g. delete
behaviour variants the harness only runs in specific configurations).
They are skipped by the upstream suite itself, not failures, and do
not count against the pass total. This is the standard skip set for a
default four-workflow run.

## How this number was reached (honest changelog)

The first end-to-end run against the unmodified crate scored
**71 / 80** with **4 Content-Discovery failures**, all cascading from
one root cause. Two real (non-fixture) server fixes — both landed in
`crates/ferro-oci-server/src/handlers/manifest.rs` and each covered by
a new regression test in `tests/conformance_smoke.rs` — brought it to
**75 / 75**:

1. **Image-index push rejected registered child manifests.**
   `verify_referenced_blobs` only consulted the blob store, so an
   `application/vnd.oci.image.index.v1+json` push that referenced child
   manifests pushed via the metadata plane (the normal flow) got
   `404 MANIFEST_BLOB_UNKNOWN`. The "References setup" step failed and
   3 discovery assertions cascaded. Fix: accept a referenced index
   child if it resolves as a registered manifest **or** a stored blob.
   → 71 → 74.

2. **Referrers `artifactType` filter ignored the `config.mediaType`
   fallback.** Per OCI Image Spec v1.1, a referrer manifest with no
   top-level `artifactType` derives it from `config.mediaType`. The
   handler only recorded the explicit field, so
   `GET /referrers/{d}?artifactType=<config media type>` under-counted
   (returned 1, suite expected 2). Fix: fall back to `config.mediaType`
   when `artifactType` is empty/absent when building the referrer
   descriptor. → 74 → 75 (clean pass).

## Reproducing

```bash
# Auto-detects a Go toolchain or Docker; uses whichever is available.
crates/ferro-oci-server/tests/conformance/run_conformance.sh

# Force a specific runner:
CONFORMANCE_RUNNER=docker crates/ferro-oci-server/tests/conformance/run_conformance.sh
CONFORMANCE_RUNNER=go     crates/ferro-oci-server/tests/conformance/run_conformance.sh
```

The script builds the server, boots it on an ephemeral port over a
temp FS blob store, points `OCI_ROOT_URL` at it, runs all four
workflows, and writes the JUnit + HTML report to `./report/` (which is
git-ignored). Its exit code mirrors the suite's, so CI can gate on it.

### Environment notes for this run

- The suite was executed via the **prebuilt Docker image** because no
  Go toolchain is installed in this environment (`which go` → not
  found). The `go` runner path in `run_conformance.sh` is implemented
  and ready for a CI image that ships Go ≥ 1.21.
- `docker` and `socat` were used during diagnosis to capture the wire
  traffic and pinpoint the two root causes above.
