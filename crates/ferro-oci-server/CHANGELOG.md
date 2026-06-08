<!-- SPDX-License-Identifier: Apache-2.0 -->
# Changelog — ferro-oci-server

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). From
`v1.0.0` onward this crate follows strict
[Semantic Versioning](https://semver.org/): breaking changes to the
public API require a major bump.

## [Unreleased]

## [1.0.0] - 2026-06-08

First semver-stable release; the public API is committed under semver.
This release turns the OCI Distribution Spec v1.1 server primitives into
a runnable server: a `ferro-oci-server` binary, Prometheus `/metrics`,
Kubernetes probes, and **durable registry metadata persistence**
(manifests, tags, and referrers survive a restart, with content-
addressing verified on load). It adds digest-verified manifest `PUT`,
bounded upload sessions (count cap + idle TTL), an explicit 512 MiB body
limit, atomic manifest+referrer transactions, and referrer-descriptor
pruning on delete. **The server passes the official
`opencontainers/distribution-spec` v1.1 conformance suite: 75/75 specs
(Push, Pull, Content Discovery, Content Management), 0 failures.**
Backed by mutation/coverage hardening and a 6-round adversarial design-
review pass (GA gate, 0 P0/P1).

### Stabilization
- API stabilized at `1.0.0` under strict semver. The `router` /
  `build_app` / `build_app_persisted` surface, the `RegistryMeta` trait
  and `InMemoryRegistryMeta`, the manifest / reference / media-type
  types, and `OciError` are now committed. Test suite hardened to a
  ≥95% mutation kill rate and ≥85% line coverage; workspace clippy
  pedantic + nursery clean under `-D warnings` with
  `unsafe_code = forbid`; `cargo audit` / `cargo deny` clean.

### Security
- **Manifest `PUT` by digest now verifies the digest matches the body.**
  `PUT /v2/{name}/manifests/sha256:<D>` previously computed the manifest
  digest but never compared it against a digest *reference* in the URL,
  so a client could push bytes hashing to `<D2>` under the key `<D1>`
  and corrupt content-addressing. A mismatch is now rejected with
  `400 DIGEST_INVALID`; a matching by-digest push still returns `201`.
  Additionally, a non-`sha256` digest reference (e.g. `sha512:<128-hex>`)
  is now rejected with `400 DIGEST_INVALID` instead of silently skipping
  verification — the registry canonicalises manifests with SHA-256 only,
  and the old "compare only when the algorithms match" guard let a
  `sha512:` reference fall through and return `201` for arbitrary bytes.
- **Per-session upload size cap bounds a memory-exhaustion DoS.** Chunked
  blob uploads accumulated in an unbounded in-memory buffer, so an
  unauthenticated client could open sessions and append sub-limit chunks
  until RAM was exhausted. Each upload session is now bounded at
  `MAX_UPLOAD_SESSION_BYTES` (4 GiB default; overridable per `AppState`
  via `with_max_upload_session_bytes`). A chunk that would exceed the cap
  is rejected with `413 Payload Too Large` (`BLOB_UPLOAD_INVALID`) and the
  session buffer is dropped immediately.
- **Concurrent upload-session count cap + idle-session TTL.** The byte cap
  above bounded *one* session, but a client could still open an unbounded
  *number* of sessions (or many near-cap ones) to pin memory. The registry
  now caps concurrent in-flight upload sessions (`DEFAULT_MAX_UPLOAD_SESSIONS`,
  1024; configurable via `InMemoryRegistryMeta::with_session_limits`) —
  a `POST .../uploads/` over the cap returns `429 Too Many Requests` — and
  evicts sessions idle past a TTL (`DEFAULT_UPLOAD_SESSION_TTL`, 1 h): a new
  upload lazily sweeps expired sessions, and an access to an expired session
  evicts it (the handler then answers `404 BLOB_UPLOAD_UNKNOWN`).
- **PATCH `Content-Range` length is now validated against the body.** A
  chunk PATCH validated only the range *start*, ignoring the *end*, so a
  request claiming `Content-Range: 0-999999` while sending one byte was
  accepted. The inclusive range length (`end - start + 1`) must now equal
  the body length, else `416 Range Not Satisfiable` (`BLOB_UPLOAD_INVALID`).
  The degenerate `Content-Range: 0-18446744073709551615` (`0-u64::MAX`),
  which overflowed that arithmetic (debug panic / release wrap-to-0), is
  now rejected outright rather than crashing or matching an empty body.

### Changed
- **Explicit request body limit (`MAX_BODY_BYTES`, 512 MiB) on the
  `/v2/**` surface.** Axum's 2 MiB `DefaultBodyLimit` silently rejected
  manifests and blob chunks larger than 2 MiB; the OCI spec expects
  registries to accept manifests of at least 4 MiB. The limit is raised
  to 512 MiB (consistent with, and below, the per-session upload cap).
- **`/metrics` scrapes no longer trigger an O(blobs) filesystem scan.**
  The `ferrooci_storage_blobs` gauge was refreshed by calling
  `BlobStore::list()` on every scrape, turning an open `/metrics`
  endpoint into O(number-of-blobs) FS work per request. The gauge is now
  fed by an O(1) atomic blob counter on `AppState`, incremented on blob
  put and decremented on blob delete. The gauge honestly measures "blobs
  written via this server instance". The increment now fires only when the
  digest is *newly* inserted (a `contains` check precedes the put), so a
  duplicate `PUT` of an existing digest no longer over-counts and a later
  single delete cannot drive the gauge below the true distinct-blob count.
- **`/metrics` requests are now themselves instrumented.** The tracking
  middleware is layered *over* the merged `/metrics` route (previously
  under it), so a `/metrics` scrape is counted under the `metrics`
  handler label that `Metrics::handler_for` already emitted.

### Added
- **Durable registry metadata for the filesystem deployment.** Previously
  the binary stored blob *bytes* on disk but kept the metadata plane
  (manifests, tag aliases, referrer index) purely in memory, so a restart
  stranded blobs whose manifests/tags had vanished. When
  `FERRO_OCI_STORAGE_DIR` is set, the metadata is now mirrored
  write-through to a `metadata.json` snapshot under that directory
  (atomic temp-file + rename) and reloaded on boot, so manifests/tags/
  referrers survive a restart together with the blobs. A missing/corrupt
  snapshot is tolerated (start empty + log). In-flight upload sessions are
  intentionally *not* persisted. New `build_app_persisted()` and
  `InMemoryRegistryMeta::with_persistence()` constructors; the in-memory
  (no `FERRO_OCI_STORAGE_DIR`) deployment stays ephemeral. *Follow-up:* an
  external SQLite/Postgres metadata backend remains on the roadmap; the
  JSON snapshot is the single-node durable mirror for now.
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

[Unreleased]: https://github.com/abyo-software/ferro-protocols/compare/ferro-oci-server-v1.0.0...HEAD
[1.0.0]: https://github.com/abyo-software/ferro-protocols/compare/ferro-oci-server-v0.1.0...ferro-oci-server-v1.0.0
[0.1.0]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-oci-server-v0.1.0
[0.0.1]: https://github.com/abyo-software/ferro-protocols/releases/tag/ferro-oci-server-v0.0.1
