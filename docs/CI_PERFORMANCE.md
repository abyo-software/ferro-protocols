<!-- SPDX-License-Identifier: Apache-2.0 -->
# CI Performance & Strategy

This document explains the per-PR vs nightly testing structure for the
`ferro-protocols` workspace and the rationale behind the job sizing.

## TL;DR

* PR CI on the self-hosted KVM runner pool: **6 jobs**, ~2-5 min wall
  with cache, ~10 min cold.
* Cross-OS tests (ubuntu / macos / windows): **weekly cron**, not per
  PR. Linux KVM is the canonical target and gates every PR.
* Coverage: **push-to-main only**, not per PR.
* Fuzz: **nightly cron at 03:17 UTC**, 5 min per target.
* Release publish: tag-driven, full pre-publish gate.

## Per-PR jobs (`.github/workflows/ci.yml`)

| job   | purpose                                    | typical wall (cached) |
| ----- | ------------------------------------------ | --------------------- |
| clippy| `cargo clippy --all-targets --all-features -- -D warnings` (supersedes a separate `cargo check`) | ~30 s |
| fmt   | `cargo fmt --all -- --check`               | ~5 s                  |
| test  | `cargo test --workspace --all-features` on KVM | ~45 s             |
| msrv  | `cargo check --workspace --all-targets` on Rust 1.88 | ~25 s       |
| deny  | `cargo-deny check` (license / advisory / source / bans) | ~20 s    |
| audit | `cargo audit --deny warnings` (RustSec)    | ~15 s                 |
| docs  | `cargo doc --workspace --no-deps --all-features` with `RUSTDOCFLAGS="-D warnings"` | ~25 s |

All `cargo install <tool>` invocations were replaced with
`taiki-e/install-action@v2`, which fetches a prebuilt binary from
GitHub Releases instead of compiling from source. This saves ~60-180 s
per `cargo audit` / `cargo-deny` / `cargo-llvm-cov` / `cargo-fuzz`
job, every run.

## Why coverage is not on PR

Coverage runs `cargo llvm-cov --workspace --all-features`, which
re-instruments the entire compile graph and is 2-5 x slower than a
plain `cargo test`. Codecov reports coverage on **merged main**, not
on PR-WIP commits, so per-PR coverage runs produce numbers that no one
acts on. Push-to-main + manual dispatch is sufficient.

## Why cross-OS is weekly, not per PR

Linux KVM is the canonical CI target and gates every PR. The
`ubuntu-latest` / `macos-latest` / `windows-latest` matrix in
`.github/workflows/cross-os.yml` catches the rare platform-specific
regression (path separators, signal handling, file-locking semantics)
on a weekly Sunday cron. None of the production
deployments of these crates target Windows, and macOS support is
best-effort.

## Why fuzz is nightly, not per PR

`cargo fuzz run` is unbounded by default; even a 5-minute budget per
target across 2 crates × N targets adds 10-30 minutes that has no
gating signal value (a 5-min fuzz run rarely finds new bugs that a
24h cycle wouldn't catch first). The nightly schedule (03:17 UTC) is
off-peak vs the runner pool and runs on its own concurrency group.

## What does NOT live in this repo

The 24h fuzz cycle, OMB benchmarks, soak tests, and KVM cluster
infrastructure live in operator scripts elsewhere in the org
(`infra/kvm-cluster/`, `~/fuzz-runner/`). They run independently of
this repo's CI and do not contend with the per-PR runner pool.

## Capacity diagnosis (Wave 13)

The 16h-queued nightly CI we observed was caused by **runner pool
saturation across 13 ferro-* org repos sharing one self-hosted KVM
host**, not by individual job runtime. Local profile on a 12 vCPU
host: full `cargo test --workspace` runs in **9 seconds wall clock**
of test execution after a 41 s build. Wave 45.BBB conformance
fixtures are tens of KB total and execute in sub-millisecond per
test.

The fix is therefore at the **workflow shape** layer (fewer concurrent
jobs per PR, drop redundant work, avoid `cargo install` from source),
not at the test-content layer. No fixtures were gated or skipped.
