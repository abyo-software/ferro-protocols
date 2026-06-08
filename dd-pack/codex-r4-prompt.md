ROUND 4 (final verification) of an adversarial DD review of `ferro-protocols`, ahead of v1.0.0 GA. Round 3 found 2 P1 + 3 P2, all in the new durable-persistence code; they have been fixed. Verify closure and do a final focused pass on the NEW rollback/durability code.

## PART A — Verify Round-3 fixes (state CONFIRMED-FIXED or RESIDUAL per item)
- R3-1: oci `registry.rs` load_snapshot now recomputes sha256 over each persisted manifest body and DROPS entries whose key digest mismatches. Any bypass (non-sha256, decode error, partial)?
- R3-2 (oci + cargo): persistence now returns Result; mutating handlers roll back the in-memory change (cargo also deletes the orphan tarball blob when unreferenced) and return 500 on persist failure. Check: is the rollback complete and correct for EVERY mutating path (oci manifest PUT/DELETE/referrer; cargo publish/yank/unyank/owners)? Any path that still returns success on persist failure? Any rollback that corrupts state (e.g. deletes a shared blob still referenced by another version; restores the wrong prior value)? Any lock/ordering bug?
- R3-3: oci serializes contains→put→increment under a mutex so concurrent same-digest uploads increment the gauge once. Correct? Deadlock risk (lock held across await)?
- R3-4: cargo load drops index versions with invalid cksum (and the crate if all versions corrupt). Correct? Could it drop valid data?
- R3-5 (oci + cargo): snapshot temp write uses O_EXCL unique temp + sync_all + rename + parent-dir fsync. Symlink-safe? Crash-durable? Temp leak on error?

## PART B — Final pass on the rollback/durability code (this is the GA gate)
The rollback logic is new and correctness-critical (it mutates then conditionally un-mutates under a lock). Look hard for: rollback that leaves state inconsistent, the cargo "delete orphan blob only if unreferenced" check being wrong (deletes a still-referenced blob, or leaks a truly-orphan one), restoring a stale prior value under concurrency, returning 500 but having already partially persisted, lock-across-await/blocking-in-async issues, and any panic reachable from a malformed persisted file or a persist error. Also confirm no regression to the OCI content-addressing or cargo immutability invariants.

## OUTPUT
Same format. PART A: CONFIRMED-FIXED/RESIDUAL per R3-N. PART B: actionable findings only. End: "PART A: <n> confirmed, <n> residual. PART B: P0=<n> P1=<n> P2=<n>. ACTIONABLE TOTAL: P0=<n> P1=<n> P2=<n>". If 0 P0/P1 and only minor P2 hardening remains, SAY SO PLAINLY — this is the GA gate and I need to know if it's clean. Read-only.
