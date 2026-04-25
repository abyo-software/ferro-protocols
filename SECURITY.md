<!-- SPDX-License-Identifier: Apache-2.0 -->
# Security Policy

## Reporting a vulnerability

If you believe you have found a security issue in any crate in this
workspace, **please do not open a public GitHub issue**. Public
disclosure before a fix is published puts users at risk.

Instead, report privately via one of:

1. **GitHub Security Advisories** — preferred. Use the "Report a
   vulnerability" button on
   <https://github.com/youichi-uda/ferro-protocols/security/advisories/new>.
   This creates a private channel between you and the maintainers and
   coordinates a CVE if one is warranted.
2. **Email** the maintainers (address is in the repository profile).
   Encrypt with the maintainer key referenced from the same profile if
   the report contains exploit details.

Please include, at minimum:

- The crate and version affected
- A clear description of the issue and its impact
- A proof-of-concept or reproducer
- Your assessment of severity and any deadline considerations

## Response targets

| Phase | Target |
|---|---|
| Acknowledge receipt | within **48 hours** |
| Triage and confirm | within **5 business days** |
| Patch published (or mitigation announced) | within **30 days** for High/Critical, **90 days** for Medium/Low |

These targets are best-effort by a small maintainer team. Reports about
crates published as `v0.0.x` (alpha) follow the same timeline; alpha
status is not an excuse for slow triage.

## Disclosure

We follow a **coordinated disclosure** model:

1. We work with the reporter to confirm and develop a fix.
2. We request a CVE through GitHub if the issue is exploitable in
   common configurations.
3. We publish a patched release on crates.io and a GitHub Security
   Advisory on the same day.
4. The advisory credits the reporter (with permission) and references
   the CVE.

We do not award bug bounties. We are happy to publicly acknowledge
researchers who follow the process described here.

## Supported versions

For each crate:

- The **latest minor of the latest major** receives security fixes.
- During the alpha (`v0.0.x`) period, only the latest patch receives
  fixes. Pinning to an older `v0.0.x` is unsupported.
- Once a crate reaches `v1.0`, the prior major receives security fixes
  for **6 months** after the new major is published.

## Known-good baseline

The CI runs `cargo audit` and `cargo deny check` on every push and PR;
the workspace blocks `openssl`, `openssl-sys`, and `native-tls` via
`deny.toml`. We standardize on `rustls`. If you find a transitive
dependency that violates this baseline, please file an issue (this is
**not** confidential — only exploit details are).

## Out of scope

- Theoretical attacks that require an attacker already inside your
  trust boundary (e.g. a malicious local user with `cargo build`
  permissions).
- Issues in upstream crates we depend on — please report those to the
  upstream project; we will coordinate updates once a fix is available.
- Issues in unrelated Ferro products (FerroSearch, FerroStream, etc.)
  — those have their own security contacts.
