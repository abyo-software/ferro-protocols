<!-- SPDX-License-Identifier: Apache-2.0 -->
# Contributing to ferro-protocols

Thanks for your interest in contributing! This document covers the rules
of engagement: how to sign commits, how issues are triaged, what we will
and will not accept, and the local quality bar.

## Developer Certificate of Origin

This project does not require a Contributor License Agreement (CLA).
Instead it uses the
**[Developer Certificate of Origin](https://developercertificate.org/) (DCO) v1.1**,
the same mechanism used by the Linux kernel, Docker, and GitLab.

By contributing, you certify the following:

> ```
> Developer Certificate of Origin
> Version 1.1
>
> By making a contribution to this project, I certify that:
>
> (a) The contribution was created in whole or in part by me and I have
>     the right to submit it under the open source license indicated in
>     the file; or
>
> (b) The contribution is based upon previous work that, to the best of
>     my knowledge, is covered under an appropriate open source license
>     and I have the right under that license to submit that work with
>     modifications, whether created in whole or in part by me, under
>     the same open source license (unless I am permitted to submit
>     under a different license), as indicated in the file; or
>
> (c) The contribution was provided directly to me by some other person
>     who certified (a), (b) or (c) and I have not modified it.
>
> (d) I understand and agree that this project and the contribution are
>     public and that a record of the contribution (including all
>     personal information I submit with it, including my sign-off) is
>     maintained indefinitely and may be redistributed consistent with
>     this project or the open source license(s) involved.
> ```

### Adding the sign-off

Every commit must end with a `Signed-off-by:` trailer that matches the
commit author. The simplest way is to use `git commit -s`:

```bash
git config --global user.name "Jane Doe"
git config --global user.email "jane@example.com"

git commit -s -m "ferro-lumberjack: fix ack wrap-around at u32::MAX"
```

The `-s` flag appends the trailer automatically. If you forget, amend
with `git commit --amend -s`. CI rejects PRs that contain unsigned
commits — the [DCO check](.github/workflows/dco.yml) is a hard gate.

## Issue triage policy

We are a small group of maintainers and explicitly publish our response
expectations:

| Severity | Target response | Examples |
|---|---|---|
| 🔴 **Security** | within **48 hours** | RCE, panic-on-malicious-input, auth bypass |
| 🟡 **Bug with reproducer** | best-effort within **14 days** | wrong output, leak, regression vs. spec |
| 🟢 **Feature request** | collected for the next minor; no individual response | new spec version, ergonomics |

A **reproducer is mandatory** for bug reports. Issues without a
reproducer (failing test, code snippet, or step-by-step) may be closed
with a request to add one.

We use an autocloser for stale issues: 30 days without engagement →
closed with a thank-you. Reopening is welcome if circumstances change.

## What this workspace does **not** accept

To keep maintenance load bounded and the scope coherent, we will not
merge contributions that:

- **Add a non-extracted crate.** Each crate must have a clearly
  identified Ferro product of origin. New crates are introduced by
  maintainers when an internal Ferro product is ready to publish.
- **Re-implement what an upstream crate already does well.** Duplicate
  layers (e.g. another Kafka wire-protocol crate) are out of scope —
  upstream contributions to existing crates are preferred.
- **Add async runtimes other than Tokio.** Caller code may, of course,
  bridge; but the workspace standardizes on Tokio + tokio-rustls.
- **Add OpenSSL / native-tls.** We block them in `deny.toml`.
- **Introduce `unsafe` blocks** without a `// SAFETY:` paragraph and a
  proportional test. `unsafe_code = "forbid"` is workspace-wide and is
  relaxed only with explicit justification.

## Local quality bar

A PR is mergeable when **all** of the following pass on a clean checkout:

```bash
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo test --workspace
cargo deny check
cargo audit
```

For parser changes, also exercise the relevant fuzz target for at least
60 seconds before requesting review:

```bash
cargo +nightly install cargo-fuzz --locked   # one-time
cd crates/<crate>
cargo +nightly fuzz run <target> -- -max_total_time=60
```

CI runs the same set on every PR plus a coverage and DCO job.

## Versioning

The workspace follows [Semantic Versioning](https://semver.org/) per
crate. The `v0.0.x` series explicitly **allows breaking API changes
between minor releases** — once a crate reaches `v0.1.0` we commit to
proper semver. The current stability of each crate is documented in its
own `README.md`.

Releases are cut by maintainers on a roughly bi-weekly cadence using
tag prefixes of the form `<crate>-vX.Y.Z`. Hotfixes are out-of-band only
when responding to a 🔴 security issue.

## Code of conduct

By participating, you agree to abide by the
[Contributor Covenant](CODE_OF_CONDUCT.md). Be respectful — disagree
with code, not with people.

## Questions

Open a
[GitHub Discussion](https://github.com/abyo-software/ferro-protocols/discussions)
for design or usage questions. File an issue only when you have
something a maintainer should act on.
