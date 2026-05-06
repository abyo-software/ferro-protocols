# OCI conformance fixtures

Real upstream-derived examples used by `tests/conformance.rs`.

## Sources

- `oci-image-manifest.json` — OCI Image Spec v1.1 §6 "Image Manifest"
  canonical example.
  Source: <https://github.com/opencontainers/image-spec/blob/v1.1.0/manifest.md#example-image-manifest>
  License: Apache-2.0 (OCI image-spec).
- `oci-image-index.json` — OCI Image Spec v1.1 §7 "Image Index"
  canonical example.
  Source: <https://github.com/opencontainers/image-spec/blob/v1.1.0/image-index.md#example-image-index>
  License: Apache-2.0 (OCI image-spec).

The `opencontainers/distribution-spec/conformance` Go test suite emits
manifests that match these shapes byte-for-byte (modulo the digests the
suite freshly computes against the just-uploaded layer bytes); the
fixtures here are the canonical templates the conformance reference
implementation seeds before each run.

License compliance: both upstream specs are Apache-2.0 and explicitly
permit verbatim reuse of example fragments. The crate itself is
Apache-2.0, matching.
