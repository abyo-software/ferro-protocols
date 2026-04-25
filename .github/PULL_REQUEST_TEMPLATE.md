<!-- SPDX-License-Identifier: Apache-2.0 -->
## Summary

<!-- 1–3 sentences. What does this PR change and why? -->

## Linked issues

<!-- "Fixes #123" / "Refs #456" -->

## Crate(s) touched

<!-- e.g. ferro-lumberjack, ferro-airflow-dag-parser -->

## Type of change

- [ ] Bug fix (non-breaking)
- [ ] New feature (non-breaking)
- [ ] Breaking change
- [ ] Docs only
- [ ] Internal refactor / dependency update / CI

## Spec compliance

<!-- If this PR adjusts protocol or wire-format behaviour, link the spec
section it now adheres to (RFC, PEP, OCI, KIP, etc.) -->

## Checklist

- [ ] All commits are signed off (`git commit -s`) — see CONTRIBUTING.md
- [ ] `cargo fmt --all -- --check` passes locally
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes locally
- [ ] `cargo test --workspace` passes locally
- [ ] `cargo deny check` passes locally
- [ ] Relevant tests added / updated (unit, proptest, fuzz, integration)
- [ ] CHANGELOG entry added under `[Unreleased]` for the affected crate
- [ ] If parser changed: at least one fuzz target covers the change and was run for ≥60s
