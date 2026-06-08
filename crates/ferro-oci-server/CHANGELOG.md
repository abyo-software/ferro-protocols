<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-oci-server

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The crate
is on the `v0.1.x` beta track; additive changes only between minor
releases. Breaking changes will be released as a separate `v0.2.0`.

## [Unreleased]

### Added
- **Prometheus `/metrics` endpoint + request instrumentation.** New
  `metrics` module exposes a `GET /metrics` route (Prometheus text
  exposition format) and a tower/axum middleware that records, by
  `method` + matched-route `handler` + `status`, a request counter
  (`ferrooci_http_requests_total`), a latency histogram
  (`ferrooci_http_request_duration_seconds`), an in-flight gauge
  (`ferrooci_uploads_in_flight`), a `ferrooci_build_info` gauge, and a
  storage gauge `ferrooci_storage_blobs` (exact blob count;
  `ferrooci_storage_bytes` is registered but reads 0 until a
  size-reporting backend is wired). Labels use the matched route pattern,
  never raw digests/names, to keep cardinality bounded. Wired into both
  `instrument()` (library) and the serve binary; the chart's
  `ServiceMonitor` is now enabled by default.
- **Runnable `ferro-oci-server` binary** (`src/bin/ferro-oci-server.rs`).
  Boots the Axum `router` over a filesystem-backed (`FERRO_OCI_STORAGE_DIR`)
  or in-memory blob store and the in-memory metadata plane, binds a
  configurable `host:port` (`FERRO_OCI_LISTEN`, default `0.0.0.0:8080`),
  and shuts down gracefully on `SIGTERM`/`SIGINT`. Environment-driven
  `Config`.
- **Kubernetes health-probe routes** — new `probe_routes()` router and
  `AppState::new` constructor. `GET /live` and `GET /ready` return
  `200 OK` with body `OK`; `GET /healthz` returns `200 OK` with JSON
  `{"status":"ok"}`. Merge into the OCI router with
  `router(state).merge(probe_routes())`.
- **OCI Distribution Spec v1.1 conformance harness**
  (`tests/conformance/run_conformance.sh` + `RESULTS.md`). Builds and
  boots the server, runs the official
  `opencontainers/distribution-spec` conformance suite (Go toolchain
  *or* prebuilt Docker image) against it across all four workflow
  categories, and records the real pass count. **Latest run: 75/75
  specs pass (Push, Pull, Content Discovery, Content Management), 0
  failures.** Generated reports under `report/` are git-ignored.
- `tests/fixtures/` — vendored canonical OCI Image Spec v1.1 examples
  (`oci-image-manifest.json`, `oci-image-index.json`) sourced from
  `opencontainers/image-spec` (Apache-2.0).
- `tests/conformance.rs` — 6 conformance tests parsing the upstream
  fixtures into the typed `ImageManifest` / `ImageIndex` structs and
  asserting round-trip stability and media-type classification. Closes
  the v0.1.0 "vendor real-protocol fixtures" gate that was deferred to
  the 0.1.x minor track in the 0.0.1 → 0.1.0 promotion notes.

### Fixed
- **Image-index push now accepts registered child manifests.**
  `PUT` of an `application/vnd.oci.image.index.v1+json` whose
  `manifests[]` reference child manifests that live in the metadata
  plane (the normal push flow — children pushed before the index) was
  rejected with `404 MANIFEST_BLOB_UNKNOWN` because validation only
  consulted the blob store. It now accepts a child digest that resolves
  as a registered manifest *or* a stored blob. Caught by the upstream
  conformance Content-Discovery "References setup" step.
- **Referrers `artifactType` filter honours the `config.mediaType`
  fallback.** Per OCI Image Spec v1.1, a referrer manifest with no
  top-level `artifactType` derives it from `config.mediaType`. The
  referrer descriptor now records this fallback so
  `GET /referrers/{digest}?artifactType=<config media type>` returns
  the correct set. Caught by the conformance Content-Discovery filter
  test.

## [0.1.0] — 2026-05-04

First beta release. Promotes the crate from the `v0.0.x` alpha track
to the `v0.1.x` beta track to signal a higher level of API stability
commitment.

### Added
- Beta track. `0.1.x` semver: minor bumps may add additive items;
  removals or signature changes will be flagged in the CHANGELOG and
  released as a separate `0.2.0`.
- `examples/parse_reference.rs` — codec-only walkthrough covering
  repository name validation, reference (tag / digest) parsing,
  manifest media-type classification, and an `ImageManifest`
  round-trip.

### Changed
- Bumped `ferro-blob-store` dependency from `0.0.3` to `0.1`. The
  `Digest` type's serde wire form is unchanged (`<algo>:<hex>`).

### Notes
- The `v0.1.0` "formal conformance harness vendoring" gate (see
  `v0.0.1` notes) lands as a separate `0.1.x` minor.

## [0.0.1] — initial alpha

Initial extraction from FerroRepo's OCI protocol crate.

### Added
- `error` — `OciError` + `OciErrorCode` rendering the spec's
  `{ "errors": [...] }` envelope
- `manifest` — `ImageManifest`, `ImageIndex`, `Descriptor` serde types
- `media_types` — Docker v2 / OCI v1 media-type classification
- `reference` — image-name + tag + digest parsing with spec-compliant
  validation (max length, no `..`, no `//`, lowercase rule)
- `registry` — `RegistryMeta` trait (manifest + tag + upload +
  referrer plane); `InMemoryRegistryMeta` reference impl
- `upload` — `UploadState` + `ContentRange` parsing
- `router` / `handlers` — Axum router for `/v2/**` covering base
  version check, catalog, tag listing, blob CRUD, blob upload
  state machine, manifest CRUD, referrers
- 67 tests pass (42 unit + 3 + 22 integration smoke). Smoke tests
  exercise full request walks; formal upstream conformance harness
  vendoring is the `v0.1.0` gate.

### Notes
- Persistent metadata backend (SQLite / Postgres) deferred to
  `v0.0.x` follow-ups; the trait is stable enough that you can
  implement your own today.

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-oci-server-v0.1.0...HEAD
[0.1.0]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-oci-server-v0.1.0
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-oci-server-v0.0.1
