ROUND 6 (GA-gate verification) of the adversarial DD review of `ferro-protocols`, ahead of v1.0.0 GA. Round 5 found 1 P1 + 2 P2; all fixed:
- R5-1 (P1): maven `handle_delete` now holds the layout write guard across remove + reference-scan + blob delete (closes the TOCTOU data-loss race). `crates/ferro-maven-layout/src/handlers.rs`.
- R5-2 (P2): maven router now sets an explicit `DefaultBodyLimit` (256 MiB const). `crates/ferro-maven-layout/src/router.rs`.
- R5-3 (P2): oci `delete_manifest` now prunes referrer descriptors matching the deleted digest (across all subjects), under the same delete transaction, restored on persist-failure rollback. `crates/ferro-oci-server/src/registry.rs`.

## PART A — Verify R5 fixes (CONFIRMED-FIXED or RESIDUAL per item)
- R5-1: Is the maven delete check+delete now race-free (lock held across all three)? Deadlock from holding the guard across `blobs.delete().await`? Still deletes genuine orphans?
- R5-2: Is the maven body limit applied to the right route(s)?
- R5-3: Are referrer descriptors pruned correctly (all subjects, multi-subject), under one transaction, restored on rollback? Any over-pruning (removing a descriptor that points to a DIFFERENT manifest that happens to share... no — descriptors are keyed by digest, confirm correctness)?

## PART B — Final convergence pass (THE GA GATE)
This is round 6. The same classes of issue (check-delete TOCTOU, missing body limit, non-atomic multi-step persist, content-addressing on load) have now been fixed across oci, cargo, and maven. Do a FINAL sweep specifically checking these patterns are closed EVERYWHERE and there are no remaining instances in any of the 6 crates:
- Any other check-then-mutate-then-delete race on a shared/content-addressed resource in ANY crate?
- Any other untrusted-input-reachable parser that can panic/OOM without a guard? (lumberjack frames, maven POM/metadata XML, airflow DAG, oci reference/manifest, cargo publish body, blob digest)
- Any other axum `Bytes`/body extractor without an explicit limit?
- Any remaining non-atomic persist or swallowed persist error?
- Any auth/authorization gap that isn't documented?
Report ONLY real, actionable findings (no padding, no speculative).

## OUTPUT
Same format. PART A per R5-N. PART B actionable only. End: "PART A: <n> confirmed, <n> residual. PART B: P0=<n> P1=<n> P2=<n>. ACTIONABLE TOTAL: P0=<n> P1=<n> P2=<n>".
THIS IS THE GA GATE: if 0 actionable P0 and 0 actionable P1, state explicitly "GA GATE: PASS (0 P0, 0 P1)" and list any P2 as acceptable-to-document. Otherwise "GA GATE: FAIL" + the blockers. Read-only.
