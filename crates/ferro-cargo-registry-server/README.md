<!-- SPDX-License-Identifier: Apache-2.0 -->
# ferro-cargo-registry-server

[![License](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](../../LICENSE)
[![crates.io](https://img.shields.io/crates/v/ferro-cargo-registry-server.svg)](https://crates.io/crates/ferro-cargo-registry-server)
[![docs.rs](https://docs.rs/ferro-cargo-registry-server/badge.svg)](https://docs.rs/ferro-cargo-registry-server)
[![CI](https://github.com/abyo-software/ferro-protocols/actions/workflows/ci.yml/badge.svg)](https://github.com/abyo-software/ferro-protocols/actions/workflows/ci.yml)
[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](../../rust-toolchain.toml)

**Embeddable** server-side primitives for the
[Cargo Alternative Registry Protocol](https://doc.rust-lang.org/cargo/reference/registries.html#registry-protocols),
sparse-index variant
([RFC 2789](https://rust-lang.github.io/rfcs/2789-sparse-index.html),
extending the alternative-registries
[RFC 2141](https://rust-lang.github.io/rfcs/2141-alternative-registries.html)).
Ships both an **embeddable Axum router** and a **runnable server
binary**.

> [Cargo's Alternative Registry RFC 2141](https://rust-lang.github.io/rfcs/2141-alternative-registries.html)
> was accepted in 2018. The widely-used full-server implementation,
> [`alexandrie`](https://github.com/Hirevo/alexandrie) (Apache-2.0),
> is a complete standalone application — you run it, you don't embed
> it. The cargo team's own [`crates.io`](https://github.com/rust-lang/crates.io)
> codebase is tightly coupled to the public registry site and isn't
> packaged as a library either. This crate is the embeddable middle
> ground: a `BlobStore`-backed Axum router you mount under any URL
> prefix, inside whatever service you already run.

> 🟢 **Stable (`v1.0.0`).** The public API is committed under strict
> semver: breaking changes require a major bump.

Part of the **Ferro ecosystem**. Extracted from FerroRepo, a
private Rust artifact repository.

## What this crate does

- **`config`** — `/config.json` response (registry API host,
  alternate registries list)
- **`index`** — sparse-index format: line-per-version JSON entries
  (`IndexEntry`, `IndexDep`), `entry_from_manifest` / `parse_lines` /
  `render_lines`
- **`name`** — canonical crate name validation (lowercase ASCII,
  hyphens, length cap)
- **`publish`** — binary publish-request parser (length-prefixed
  metadata JSON + tarball bytes) per spec §"Publish"
- **`version`** — semver validation
- **`owners`** — owners API request / response types
- **`yank`** — yank/unyank response
- **`handlers` / `router`** — Axum router for `/config.json`,
  `/index/{*path}`, `/api/v1/crates/**`. Mounts a
  `BlobStore` from
  [`ferro-blob-store`](https://crates.io/crates/ferro-blob-store).

## What this crate does **not** do

- **Git index format** — sparse only. `cargo` 1.68+ defaults to
  sparse, controllable per-registry via
  `CARGO_REGISTRIES_*_PROTOCOL=sparse`. Git-index support is on the
  roadmap behind a separate feature once the in-tree git
  primitives stabilise.
- **Authentication** — handlers are open. Layer your auth in the
  Axum middleware stack above this router.
- **TUF v2 metadata** — the spec's TUF metadata layer is a
  separate crate (Phase 3).
- **crates.io federation / mirroring** — this is for *alternative*
  registries (private registries / mirrors / corp internal). The
  server here does not pull from upstream crates.io.

## Quick start

```rust,no_run
use std::sync::Arc;
use axum::Router;
use ferro_blob_store::FsBlobStore;
use ferro_cargo_registry_server::{router, CargoState};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let store = Arc::new(FsBlobStore::new("/var/lib/cargo-registry")?);
let state = CargoState::new(store, "https://my-registry.example.com");
let app: Router = router(state);
let listener = tokio::net::TcpListener::bind("0.0.0.0:8082").await?;
axum::serve(listener, app).await?;
# Ok(()) }
```

### Run the bundled server binary

No glue code required — the crate ships a runnable binary:

```bash
FERRO_CARGO_REGISTRY_LISTEN=0.0.0.0:8081 \
FERRO_CARGO_REGISTRY_DATA=./registry-data \
FERRO_CARGO_REGISTRY_API=http://127.0.0.1:8081 \
  cargo run --bin ferro-cargo-registry-server
```

| Env var | Default | Purpose |
|---|---|---|
| `FERRO_CARGO_REGISTRY_DATA`   | `./registry-data` | Blob-store data directory |
| `FERRO_CARGO_REGISTRY_LISTEN` | `0.0.0.0:8081`    | Listen socket address |
| `FERRO_CARGO_REGISTRY_API`    | `http://<listen>` | API host advertised in `/config.json` |

`FERRO_CARGO_REGISTRY_API` is **required** whenever the listen host is a
wildcard (`0.0.0.0` / `::`) or the port is `0`: a derived origin such as
`http://0.0.0.0:8081` (or a `:0` port) is advertised in `config.json`
but is not fetchable by a remote cargo client. With a wildcard or
port-`0` listen and no explicit API host, the binary **refuses to boot**
and prints a message naming `FERRO_CARGO_REGISTRY_API`. A concrete
non-wildcard listen (for example `127.0.0.1:8081`) derives a usable
`http://<addr>` origin automatically.

The filesystem-backed binary persists the crate index durably: tarballs
live in the content-addressed blob store under `<data>/sha256/`, and the
per-crate index state (versions, `cksum`s, `yanked` flags, owners) is
mirrored to `<data>/index-state.json`, written through on every publish /
yank / owner change and loaded on boot. A restart therefore re-serves
every previously published crate. A missing or corrupt snapshot starts
the index empty (logged) and never blocks startup. (The in-process
library `CargoState::new` is ephemeral; `CargoState::with_persistence`
opts into the durable path.)

The binary mounts Kubernetes-style probes alongside the protocol
routes: `GET /live`, `GET /ready`, and `GET /healthz`
(`{"status":"ok"}`). Shutdown is graceful on `SIGTERM` / Ctrl-C.

Client-side: point `cargo` at it via `~/.cargo/config.toml`. The index
base is the **server root** for the bundled binary:

```toml
[registries.my-registry]
index = "sparse+https://my-registry.example.com/"

# Optional — make this the default for `cargo publish`:
# [registry]
# default = "my-registry"
```

Then:

```bash
$ cargo publish --registry=my-registry
   Updating `my-registry` index
   Packaging some-crate v0.1.0 (...)
   Verifying some-crate v0.1.0 (...)
   Uploading some-crate v0.1.0 (...)
$ cargo install some-crate --registry=my-registry
   Updating `my-registry` index
  Downloaded some-crate v0.1.0 (registry `my-registry`)
   ...
```

The publish flow lands at `/api/v1/crates/new`; the index fetch is
served from `/index/{*path}`; the tarball download is at
`/api/v1/crates/{name}/{version}/download` — all of those are wired
into the `router()` returned by this crate.

## Status

| Aspect | Status |
|---|---|
| API stability | **stable** (`v1.x`) — strict semver from `1.0.0` |
| Runnable binary | yes (`ferro-cargo-registry-server`) with `/live` `/ready` `/healthz` |
| Real-`cargo` e2e | publish / fetch / yank / unyank verified (`tests/cargo_e2e.rs`) |
| Sparse index | working (root-relative and `/index/` prefix layouts) |
| Git index | not yet |
| Authentication | scaffold only — wire your own middleware |
| TUF metadata | not in this crate (Phase 3, separate) |
| MSRV | rustc **1.88** |

## Used in production by

- **FerroRepo** (private) — Rust artifact repository.

## License

Apache-2.0. See [`LICENSE`](../../LICENSE).
