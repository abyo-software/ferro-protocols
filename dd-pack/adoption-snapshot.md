<!-- SPDX-License-Identifier: Apache-2.0 -->
# Adoption snapshot — ferro-protocols published crates

**Snapshot date:** 2026-06-08
**Source:** crates.io public API (`https://crates.io/api/v1/crates/<name>` and
`.../reverse_dependencies`), fetched live with a descriptive `User-Agent`
per the crates.io data-access policy.

> These are **new** crates (first published 2026-04-25/26). Low or near-zero
> external downloads is the expected, honest baseline. The purpose of this
> record is to establish a **time-series baseline** so later snapshots can show
> a trend, not to claim traction we don't have.

## crates.io download counts (fetched 2026-06-08)

| Crate | Published version (crates.io) | Total downloads | Recent (90d) | Reverse deps | First published |
|-------|-------------------------------|-----------------|--------------|--------------|-----------------|
| ferro-blob-store           | 0.0.3 | 92 | 92 | 3 (intra-workspace, see note) | 2026-04-26 |
| ferro-lumberjack           | 0.1.0 | 12 | 12 | 0 | 2026-04-25 |
| ferro-airflow-dag-parser   | 0.0.1 | 15 | 15 | 0 | 2026-04-25 |
| ferro-maven-layout         | 0.0.1 | 13 | 13 | 0 | 2026-04-26 |
| ferro-cargo-registry-server| 0.0.1 | 12 | 12 | 0 | 2026-04-26 |
| ferro-oci-server           | 0.0.1 | 15 | 15 | 0 | 2026-04-26 |

`recent_downloads` equals `downloads` for every crate because all six were
first published within the last ~6 weeks, so the entire lifetime download
count falls inside the crates.io 90-day "recent" window.

### ferro-blob-store version breakdown (crates.io)

| Version | Downloads | Published |
|---------|-----------|-----------|
| 0.0.3 | 34 | 2026-04-26T02:36:16Z |
| 0.0.2 | 15 | 2026-04-26T02:30:13Z |
| 0.0.1 | 43 | 2026-04-26T02:15:10Z |

## Reverse-dependency note (honest reading)

The crates.io reverse-dependency endpoint reports **3 dependents for
`ferro-blob-store` and 0 for every other crate**. The three dependents are
**not external adopters** — they are the other published ferro-protocols
crates that consume the blob store:

- `ferro-maven-layout`
- `ferro-cargo-registry-server`
- `ferro-oci-server`

(confirmed via the `.versions[].crate` field of the reverse-dependency
response). So the honest characterisation is: **intra-workspace adoption of
`ferro-blob-store` as a shared storage layer is real and visible on
crates.io, but external (third-party) adoption is currently zero across all
six crates.** No external crate depends on any ferro-protocols crate yet.

## Version drift caveat (do not mis-read the table)

The crates.io "Published version" column above (mostly `0.0.1` / `0.0.3`)
is **lower** than the in-repo `Cargo.toml` versions
(`ferro-blob-store` 0.1.0, `ferro-lumberjack` 0.2.0, others 0.1.0). The
published `0.0.x` releases were the initial seed publishes; the working tree
has since advanced toward the upcoming `0.x → 1.0.0` GA but those bumps have
**not been published to crates.io as of this snapshot**. The
`semver-checks` CI gate (added 2026-06-08, see `.github/workflows/ci.yml`) is
intended to police the `0.x → 1.0.0` publish when it happens.

## Methodology / reproducibility

```bash
UA="ferro-protocols-dd-pack/0.1 (adoption-snapshot; youichi.uda@gmail.com)"
for c in ferro-blob-store ferro-lumberjack ferro-airflow-dag-parser \
         ferro-maven-layout ferro-cargo-registry-server ferro-oci-server; do
  curl -s -H "User-Agent: $UA" "https://crates.io/api/v1/crates/$c" \
    | jq -r '.crate | "\(.id) dl=\(.downloads) recent=\(.recent_downloads) v=\(.max_version)"'
  curl -s -H "User-Agent: $UA" "https://crates.io/api/v1/crates/$c/reverse_dependencies" \
    | jq -r '.meta.total'
done
```

A bare `curl` without a `User-Agent` is rejected by crates.io with an
HTTP-200 JSON error pointing at <https://crates.io/data-access>; the
descriptive UA above is required for the numbers in this file to be fetchable.

## Next snapshot

Re-run the methodology block above and append a new dated table. Watch for:
the first **external** reverse-dependency (currently 0), and any uplift in
download counts after the GA `1.0.0` publish.
