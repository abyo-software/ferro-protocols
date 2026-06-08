You are performing ROUND 2 of an adversarial due-diligence code review of the `ferro-protocols` Rust workspace, ahead of v1.0.0 GA. Round 1 found 5 P1 + 4 P2 findings which have since been fixed. Your job has TWO parts.

## PART A — Verify the Round-1 fixes are correct and complete (no regressions, no incomplete closure)
For each, check the current code and judge: is it actually fixed? Any bypass remaining? Any new bug introduced by the fix?
1. **F1** — OCI manifest PUT-by-digest now verifies the URL digest equals sha256(body). Check `crates/ferro-oci-server/src/handlers/manifest.rs`. Is the comparison correct (algorithm + hex, case-sensitivity)? Can a different digest algorithm or malformed digest bypass it?
2. **F2** — OCI chunked-upload size cap. Check `crates/ferro-oci-server/src/upload.rs` + `handlers/blob_upload.rs`. Is the cap enforced on BOTH PATCH and PUT-finish paths? Is the session buffer actually dropped on overflow? Any integer-overflow in the size accounting?
3. **F3** — explicit DefaultBodyLimit. Check `crates/ferro-oci-server/src/router.rs` and `crates/ferro-cargo-registry-server/src/router.rs`. Are the limits applied to the right routes? Is the OCI manifest limit ≥4 MiB? Is the cargo publish limit applied and sane? Does the limit layer actually take effect after `.merge()`/`instrument()` wrapping?
4. **F4** — cargo PublishManifest→IndexEntry mapping (version_req→req, renamed deps explicit_name_in_toml, registry-computed cksum). Check `crates/ferro-cargo-registry-server/src/index.rs`. Is the mapping correct per the cargo index JSON schema? Are optional/default/features/kind fields handled?
5. **F5** — cargo canonical-name keying + collision rejection. Check `crates/ferro-cargo-registry-server/src/{name,handlers}.rs`. Is canonicalization (lowercase + `-`/`_` fold) applied on EVERY lookup path (publish, serve_index, download, yank, owners)? Is the index PATH still hyphen-preserving and correct per cargo's spec? Any way mixed-case still 404s?
6. **F6** — PATCH Content-Range length validation. Check `crates/ferro-oci-server/src/handlers/blob_upload.rs`. Correct inclusive-length math (end-start+1)? Off-by-one?
7. **F7** — /metrics self-instrumentation ordering. Check both `metrics.rs`. Is /metrics now counted (or honestly documented as not)?
8. **F8** — O(1) blob-count gauge instead of O(blobs) scrape. Check `crates/ferro-oci-server/src/metrics.rs` + AppState. Is the counter correctly inc/dec on put/delete with no drift (e.g. double-count on re-put of existing blob, or under-count on delete-missing)?
9. **F9** — maven-metadata.xml panic-shield + fuzz. Check `crates/ferro-maven-layout/src/{xml,metadata,pom}.rs`. Is the catch_unwind helper correctly shared and applied to the metadata path?

## PART B — Fresh wide-scope pass on the NEW code added since Round 1
The fixes + the new server binaries (`src/bin/`, `src/serve.rs`), the `/metrics` middleware, ~120 new tests, and the lint-parity refactors are all new surface. Look for NEW issues: panics from untrusted input, auth gaps, integer overflow, race conditions / lock-across-await in the metrics counters and registry state, resource leaks (upload sessions never expired — note if still open), spec drift, and any correctness bug in the F4/F5 cargo index/name logic or the F1/F2 oci logic.

## OUTPUT
Same format as Round 1. For each finding: Severity (P0/P1/P2), Location `file:line`, what's wrong + why it matters + trigger, suggested fix. 
- For PART A: explicitly state for each F1–F9 whether it is CONFIRMED-FIXED or has a residual issue (and if residual, file it as a new finding).
- For PART B: only real, actionable findings — do not pad.
- Do not modify files (read-only). 
- End with: "PART A: <n> confirmed-fixed, <n> residual. PART B: P0=<n> P1=<n> P2=<n>. ACTIONABLE TOTAL: P0=<n> P1=<n> P2=<n>".
