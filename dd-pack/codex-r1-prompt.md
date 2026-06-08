You are performing an adversarial due-diligence (DD) code review of the `ferro-protocols` Rust workspace (6 crates: ferro-blob-store, ferro-lumberjack, ferro-airflow-dag-parser, ferro-maven-layout, ferro-cargo-registry-server, ferro-oci-server) ahead of a v1.0.0 GA / acquisition due-diligence. Be skeptical and concrete. This is wide-scope.

Focus your review on THREE axes:
1. **Public API layer** — the recently added server binaries (`crates/ferro-oci-server/src/bin/`, `crates/ferro-cargo-registry-server/src/bin/`), the new `/live` `/healthz` `/ready` probe routes, the new Prometheus `/metrics` endpoint + instrumentation middleware in both server crates, and the public `router()` / `AppState` / `CargoState` surfaces. Look for: panics reachable from untrusted HTTP input, unwrap/expect on request-derived data, missing input validation, integer overflow, path traversal in the filesystem-backed stores, unbounded allocation / DoS (large uploads, large manifests, decompression), metric-label cardinality explosions, auth gaps that aren't documented.
2. **Internal logic** — recent changes: OCI image-index child-manifest validation + referrer artifactType fallback (`crates/ferro-oci-server/src/handlers/manifest.rs`), cargo-registry absolute `dl` URL + root-relative sparse index serving (`crates/ferro-cargo-registry-server/src/{config,router,index}.rs`), the metrics storage gauges (do they recompute correctly? race conditions? lock-hold across await?). Verify correctness against the specs: OCI Distribution Spec v1.1 and Cargo Alternative Registry Protocol (RFC 2789).
3. **Fuzz / robustness gaps** — which untrusted-input parsers lack a fuzz target or have a reachable panic? The crates parse: Lumberjack/Beats frames, Maven POM XML (quick-xml), Airflow Python DAGs (ruff), OCI manifests/references/digests, Cargo publish bodies / sparse index. Identify any parser reachable from the network that can panic or OOM.

Output your findings as a numbered list. For EACH finding give:
- **Severity**: P0 (critical: memory unsafety, RCE, auth bypass, guaranteed panic/DoS from trivial untrusted input) / P1 (high: likely-exploitable DoS, spec-violation causing data corruption, significant correctness bug) / P2 (medium/minor: hardening, edge-case correctness, doc/spec drift).
- **Location**: `file:line`.
- **What's wrong** and **why it matters** (concrete, with the input/sequence that triggers it).
- **Suggested fix**.

If you find NO actionable P0/P1, say so explicitly and list only P2/hardening. Do not invent findings to pad the list — false positives waste DD time. Do not modify any files; this is read-only analysis. End with a one-line summary: "ACTIONABLE: P0=<n> P1=<n> P2=<n>".
