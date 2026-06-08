ROUND 5 (final GA-gate verification) of an adversarial DD review of `ferro-protocols`, ahead of v1.0.0 GA. Round 4 found 2 P1 + 1 P2 (atomicity/rollback edge cases); they have been fixed:
- R4-1: oci manifest+tag+referrer is now ONE transaction (`RegistryMeta::put_manifest_with_referrer`, registry.rs ~617) under one lock + one snapshot write, with full rollback on persist failure.
- R4-2: oci rollback now prunes empty repo/tag/referrer maps (`prune_empty_repo`).
- R4-3: cargo publish-rollback now holds the index write lock across the digest-reference check AND the blob delete (sync `digest_referenced_in`), closing the orphan-delete TOCTOU.

## PART A — Verify R4 fixes (CONFIRMED-FIXED or RESIDUAL per item)
- R4-1: Is manifest+tag+referrer truly atomic now? Any path that still does two separate persists? Does rollback restore ALL three correctly (including prior tag value if overwriting an existing tag)? 
- R4-2: Are empty maps pruned on ALL rollback paths without pruning non-empty ones?
- R4-3: Is the check+delete now race-free (lock held across both)? Any deadlock from holding the write guard across `blobs.delete().await`? Does it still delete genuinely-orphan blobs?

## PART B — Final wide pass (GA gate)
Look across all 6 crates for any remaining P0/P1 (memory unsafety, auth bypass, panic-DoS from untrusted input, data-loss, spec violation). Focus especially on anything NOT yet reviewed and on interactions between the fixes. Confirm: OCI content-addressing invariant holds (PUT + load), cargo version immutability holds, cargo canonical-name keying holds, upload session caps hold, body limits hold, no reachable panic from untrusted HTTP input or malformed persisted files.

## OUTPUT
Same format. PART A: per R4-N. PART B: actionable findings only (no padding). End: "PART A: <n> confirmed, <n> residual. PART B: P0=<n> P1=<n> P2=<n>. ACTIONABLE TOTAL: P0=<n> P1=<n> P2=<n>". 
THIS IS THE GA GATE: if there are 0 actionable P0 and 0 actionable P1 (P2 hardening is acceptable to document as known-limitations), state explicitly "GA GATE: PASS (0 P0, 0 P1)". Otherwise list what blocks. Read-only.
