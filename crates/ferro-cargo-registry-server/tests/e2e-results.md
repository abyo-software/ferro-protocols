<!-- SPDX-License-Identifier: Apache-2.0 -->
# Cargo registry end-to-end verification results

This file records, honestly, which legs of the Cargo Alternative
Registry Protocol (RFC 2789 / the Cargo registry reference) are
verified for `ferro-cargo-registry-server`, and by what means.

## Test harness

`tests/cargo_e2e.rs` boots the compiled `ferro-cargo-registry-server`
binary on an ephemeral loopback port (filesystem-backed
`FsBlobStore`), points a throwaway `CARGO_HOME` at it with an
alternative-registry `config.toml`
(`index = "sparse+http://127.0.0.1:<port>/"`), and drives the **real
`cargo` binary** through the developer flow. The test skips (does not
fail) when `cargo`/`curl` are absent or the port can't be bound, so
constrained CI sandboxes stay green.

`tests/http_roundtrip.rs` complements it with in-process Axum
(`tower::ServiceExt::oneshot`) tests that issue the exact byte-framed
requests cargo would, including the publish binary body framing, the
`If-None-Match` sparse-index 304 path, and the full owners
add/list/remove matrix.

## Verification matrix

| Protocol leg | Endpoint | Verified by | Status |
|---|---|---|---|
| Publish | `PUT /api/v1/crates/new` | **real `cargo publish`** | ✅ "Published ferro-e2e-throwaway" |
| Sparse index fetch | `GET /{prefix}/{name}` (root-relative) | **real `cargo`** (publish poll + `cargo fetch` resolve) + HTTP assert on index line | ✅ |
| `config.json` | `GET /config.json` | real `cargo` (index discovery) + http_roundtrip | ✅ |
| Tarball download | `GET /api/v1/crates/{name}/{version}/download` | **real `cargo fetch`** | ✅ "Downloaded ferro-e2e-throwaway" |
| Yank | `DELETE /api/v1/crates/{name}/{version}/yank` | **real `cargo yank`** + index line assert `yanked:true` | ✅ |
| Unyank | `PUT /api/v1/crates/{name}/{version}/unyank` | **real `cargo yank --undo`** + index line assert `yanked:false` | ✅ |
| Owners GET | `GET /api/v1/crates/{name}/owners` | HTTP-level (cargo has no head-less alt-registry owners subcommand) | ✅ |
| Owners add/remove | `PUT` / `DELETE .../owners` | HTTP-level (`http_roundtrip.rs`) | ✅ |
| Git index | `GET /index.git/{*path}` | http_roundtrip (501 stub assertion) | ✅ stubbed (sparse-only by design) |

## Real-cargo vs HTTP-level — honest disclosure

- **Driven by the real `cargo` binary:** publish, sparse-index resolve,
  tarball download, yank, unyank. These are the legs cargo exercises
  head-lessly given an alt-registry config + a stored credential token.
  The token is accepted unconditionally (this crate ships auth open —
  see README "What this crate does not do"); cargo still requires a
  token to *send*, so a dummy token is configured.
- **HTTP-level only:** the owners endpoints. `cargo owner` is
  interactive / crates.io-oriented and is not driven head-lessly here;
  owners are verified by issuing the exact `GET`/`PUT`/`DELETE`
  requests cargo's web-API client would, in `http_roundtrip.rs` (the
  full add/list/remove matrix) and a `GET` smoke in `cargo_e2e.rs`.

## Index-URL layout note

To make a stock `cargo publish` round-trip, two things were wired in
this campaign (both inside this crate):

1. The router now serves sparse-index line files **root-relative**
   (`/{prefix}/{name}`) in addition to the legacy `/index/{*path}`
   prefix, matching cargo's request layout when
   `index = "sparse+http://host/"`.
2. `config.json`'s `dl` template is rendered as an **absolute URL**
   rooted at the configured API host, so cargo can download tarballs
   without a host-resolution error.

## Reproduce manually

```bash
cargo run --bin ferro-cargo-registry-server &   # listens on 0.0.0.0:8081
# ~/.cargo/config.toml:
#   [registries.ferro]
#   index = "sparse+http://127.0.0.1:8081/"
cargo publish --registry ferro --allow-dirty --no-verify
cargo fetch     # from a crate depending on the published one
cargo yank --registry ferro --version <v> <name>
```
