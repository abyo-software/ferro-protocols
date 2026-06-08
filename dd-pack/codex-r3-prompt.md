You are performing ROUND 3 (verification + final pass) of an adversarial DD review of the `ferro-protocols` workspace, ahead of v1.0.0 GA. Round 2 produced 9 actionable findings (3 P1, 6 P2) which have since been fixed, AND substantial new code was added: durable metadata persistence for both server binaries, upload session caps + idle TTL, and ~250 new mutation-hardening tests.

## PART A — Verify the Round-2 fixes are correct and complete
Check each in the CURRENT code; state CONFIRMED-FIXED or RESIDUAL (file a new finding if residual):
- **R2-1**: oci manifest PUT-by-digest now rejects non-sha256 digest references. `crates/ferro-oci-server/src/handlers/manifest.rs`. Any remaining bypass?
- **R2-2**: oci `ContentRange::length()` overflow guarded with checked arithmetic. `crates/ferro-oci-server/src/upload.rs` + `handlers/blob_upload.rs`.
- **R2-3**: cargo `/metrics` now self-instrumented (merge-then-layer). `crates/ferro-cargo-registry-server/src/metrics.rs`.
- **R2-4**: oci `storage_blobs` gauge only counts newly-inserted blobs. `crates/ferro-oci-server/src/handlers/blob_upload.rs`. Drift on duplicate/delete-missing?
- **R2-5**: cargo rejects republishing an existing `(name, vers)` with 409. `crates/ferro-cargo-registry-server/src/handlers.rs`.
- **R2-6**: BOTH server binaries now persist registry metadata/index durably to the data dir and load on boot. `crates/ferro-oci-server/src/registry.rs` (`metadata.json`) + `crates/ferro-cargo-registry-server/src/persist.rs` (`index-state.json`). **Scrutinize the persistence carefully**: atomic write (temp+rename)? corruption/partial-write recovery? concurrent-write races (lock held across the file write)? unbounded file growth? path handling? does load-on-boot correctly reconstruct ALL state (manifests, tags, referrers / index entries, versions, yanked, owners, cksum→tarball mapping)? Can a crafted persisted file cause a panic on load?
- **R2-7**: oci upload sessions now have a concurrent-count cap + idle TTL. `crates/ferro-oci-server/src/registry.rs`. Is the sweep correct? Can the cap be evaded? Off-by-one?
- **R2-8**: cargo validates (collision + duplicate) BEFORE writing the tarball blob. `crates/ferro-cargo-registry-server/src/handlers.rs`. Any remaining orphan-blob path?
- **R2-9**: cargo refuses to advertise a wildcard/port-0 origin in config.json. `crates/ferro-cargo-registry-server/src/serve.rs`.

## PART B — Fresh pass on the NEW persistence + session code (highest-risk new surface)
The durable persistence + session lifecycle code is brand new and security/correctness-sensitive (it parses files from disk = a trust boundary if the data dir is attacker-influenced, and it holds locks across I/O). Look hard for: panics on malformed persisted state, TOCTOU/races between the in-memory map and the persisted file, lock-held-across-await deadlock/contention, disk-exhaustion from unbounded metadata, partial-write corruption leaving unloadable state, and any spec/correctness regression the persistence introduced (e.g. a restored manifest losing its media type, a restored index entry losing a field, yanked flag not surviving). Also sanity-check the new test code didn't introduce `unwrap`/`panic` reachable in non-test builds.

## OUTPUT
Same format. For PART A: CONFIRMED-FIXED or RESIDUAL per R2-N. For PART B: real actionable findings only (Severity, file:line, what/why/trigger, fix). End with: "PART A: <n> confirmed, <n> residual. PART B: P0=<n> P1=<n> P2=<n>. ACTIONABLE TOTAL: P0=<n> P1=<n> P2=<n>". If clean (0 P0/P1, ≤ a few P2 hardening), say so plainly — this is the GA gate. Read-only; do not modify files.
