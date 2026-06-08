<!-- SPDX-License-Identifier: Apache-2.0 -->
# Upstream OSS contributions & disclosures

**Date:** 2026-06-08
**Scope:** Real, citable upstream-bug findings surfaced by the ferro-protocols
fuzz farm and how each was isolated/disclosed. Every row below is backed by a
**commit hash in this repo** plus an upstream reference. No speculative or
"intended" disclosures are listed.

## Summary table

| # | Upstream project | Issue / bug | How disclosed / isolated | Our commit(s) | Status |
|---|------------------|-------------|--------------------------|---------------|--------|
| 1 | `littrs-ruff-python-parser` (vendors astral-sh/ruff) | `unreachable!()` at `expression.rs:1633:25` — t-string/f-string "middle" token routing panic; **process-abort DoS** on attacker-controlled Python source (175 B repro) | Re-discovered via 24h fuzz of `airflow_dag_extract` (first crash in 35 s, 7 distinct inputs). Found **already fixed upstream in ruff** (issue #23198 / PR #23232, landed `6ded4bed` 2026-02-12); littrs 0.6.2 had vendored ruff `56eb6b62`, 5 days *before* the fix. Action item = re-vendor in next littrs release. **No new ruff disclosure needed** (already fixed Feb 2026). Defence-in-depth isolation added on our side. | `fuzz/known-crash/airflow_dag_extract/DISCLOSURE_TIMELINE.md`, `fuzz/known-crash/airflow_dag_extract/FINDING.md` | **Upstream fixed (Feb 2026); awaiting littrs re-vendor.** Isolated our side. |
| 2 | astral-sh/ruff (parser) | Unbounded recursion in `parse_unary_expression` / `parse_lhs_expression` — `~~~~…` chains (1961 consecutive `~`) overflow the 2 MiB thread stack → **SIGSEGV** (`catch_unwind` cannot catch it) | Fuzz finding `crash-ba2528b6…` (1980 B). Cross-linked to **existing OPEN upstream PR astral-sh/ruff #24810** ("Parser recursion limit", `max_recursion_depth` default 202). Verified locally against the PR branch (ruff `7b61191`): the repro surfaces as `RecursionLimitExceeded` instead of SIGSEGV. **No duplicate PR filed** (do not duplicate active upstream work). Isolated via caller-side pre-screen cap. | `8d0ee28` (unary-op-run cap), `060bc5d` (bracket-depth cap), `3c47473` (cross-link PR #24810 in `panic_safe.rs`) | **Upstream PR #24810 OPEN.** Isolated our side as defence-in-depth. |
| 3 | `quick-xml` 0.39.2 (`quick_xml::de`) | `unreachable!()` at `de/mod.rs:2903:37` (`entered unreachable code`) on malformed POM (173 B repro: `<><groupId\tp…<!DOCTYPe…`) — **process-abort DoS** through the maven registry PUT handler | 2026-05-15 fuzz wave. Isolated by wrapping the `quick_xml::de::from_str` call in `std::panic::catch_unwind(AssertUnwindSafe(…))` so the panic becomes a recoverable `MavenError::InvalidPom` instead of aborting the process. Regression test `quick_xml_unreachable_panic_caught_2026_05_15` pins the behaviour. | `3833cb2` (catch_unwind wrap + regression test in `crates/ferro-maven-layout/src/pom.rs`) | **Isolated our side.** No upstream quick-xml issue filed yet (candidate for disclosure — see below). |

## Detail

### 1. littrs / ruff t-string `unreachable!()` (already fixed upstream)

The strongest "credibility" point here is the **disclosure timeline
discipline**: rather than file a noisy duplicate, the fuzz finding was traced
to ruff issue **#23198** / PR **#23232** (closed `completed`, fix
`6ded4bed`), and the gap was correctly attributed to littrs vendoring ruff at
`56eb6b62` — five days *before* the upstream fix. The full audit trail is in
`fuzz/known-crash/airflow_dag_extract/DISCLOSURE_TIMELINE.md`. Our side keeps
the 7 crash inputs as permanent regression seeds and applies a
`catch_unwind` boundary as defence-in-depth (ruff main still has 14+
`unreachable!()` arms in `expression.rs`).

### 2. ruff parser recursion limit (active upstream PR #24810)

`catch_unwind` does **not** catch a Linux SIGSEGV stack overflow, so the
unary-operator-chain class can't be handled the same way as #1 and #3. Our
mitigation is a caller-side byte-scan pre-screen (`MAX_UNARY_OP_RUN = 64`,
`MAX_BRACKET_DEPTH = 32`) in `panic_safe.rs`. Commit `3c47473` annotates both
caps with a cross-reference to **astral-sh/ruff PR #24810**, which adds an
`enter_recursion`-instrumented `max_recursion_depth` parameter wrapping
`parse_lhs_expression` (the recursion entry point). Verified on the PR branch
that the exact 2026-05-09 fuzz artifact (`~ × 1961`) surfaces as
`ParseErrorType::RecursionLimitExceeded`. Once #24810 lands and ships in a
littrs release, our pre-screen demotes from sole-defence to defence-in-depth.

### 3. quick-xml deserialiser `unreachable!()` (isolated; disclosure candidate)

A 173-byte malformed POM hits `quick-xml` 0.39.2's `unreachable!()` at
`de/mod.rs:2903:37`. Before `3833cb2`, `parse_pom` called
`quick_xml::de::from_str` directly with no panic isolation, so the panic
propagated past the `MavenError::InvalidPom` conversion and aborted the
process — reachable from the `ferro-maven-server` registry PUT handler with
attacker-supplied POM bodies. Fixed by wrapping the deserialiser in
`catch_unwind` and mapping all three outcomes (`Ok(Ok)` / `Ok(Err)` /
`Err(panic)`) to typed results.

> **Honest status:** this is currently **isolated on our side only**. We have
> a minimal 173-byte reproducer and the exact panic site
> (`de/mod.rs:2903:37`), which is everything needed to file an upstream
> `quick-xml` issue, but **no upstream issue/PR has been filed yet** — listing
> it as "disclosed upstream" would be inaccurate. It is recorded here as the
> top disclosure candidate.

## Credibility note for DD

- All three findings came from the project's **own fuzz farm**
  (`fuzz/`, cargo-fuzz harnesses on the Tier-1 crates), demonstrating a
  working continuous-fuzzing capability rather than one-off audits.
- Each finding has a **byte-level reproducer** (175 B / 1980 B / 173 B) and a
  **pinned panic site**, and each is gated by a **regression test or
  known-crash corpus entry** so it can't silently regress.
- Disclosure hygiene is conservative: we **do not duplicate** active upstream
  work (#24810) and **do not claim** disclosures we haven't filed (#3).
