# ferro-oci-server Helm chart

Helm chart for [ferro-oci-server](https://github.com/abyo-software/ferro-protocols),
a Rust-native OCI Distribution (`/v2/`) registry server.

## Install

```bash
helm install ferro-oci-server charts/ferro-oci-server \
    --namespace registry --create-namespace
```

## Persistence

ferro-oci-server stores blob bytes on a filesystem directory
(`FERRO_OCI_STORAGE_DIR`). The chart provisions a `PersistentVolumeClaim`
(default `20Gi`, `ReadWriteOnce`) and mounts it at
`persistence.mountPath` (default `/var/lib/ferro-oci-server`). Point at an
existing volume with `persistence.existingClaim`, or disable durability
for ephemeral test installs with `persistence.enabled=false` (emptyDir —
blobs are lost on restart).

Because the default PVC is `ReadWriteOnce`, the Deployment uses the
`Recreate` strategy and `replicaCount` should stay at `1`. Scale out only
with a `ReadWriteMany` storage class or an external shared blob backend.

## Probes

Liveness is wired to `/live` and readiness to `/ready` (both return
`200 OK`). `/healthz` returns `{"status":"ok"}` and backs the image's
Docker `HEALTHCHECK`.

## Metrics / ServiceMonitor

ferro-oci-server exposes a Prometheus `/metrics` endpoint on the `http`
port: per-route request counters (`ferrooci_http_requests_total`), a
request-latency histogram (`ferrooci_http_request_duration_seconds`), an
in-flight gauge, a build-info gauge, and a storage gauge
(`ferrooci_storage_blobs` — the exact blob count;
`ferrooci_storage_bytes` is registered but reads `0` until a
size-reporting blob backend is wired). The chart's `ServiceMonitor` is
**enabled by default** (`serviceMonitor.enabled=true`) and scrapes that
port at `path: /metrics`. Import the bundled Grafana dashboard from
`dashboards/grafana/ferro-oci-server.json`.

## Quality posture

- Non-root (UID 65534), read-only rootfs, dropped capabilities,
  seccomp `RuntimeDefault`
- `automountServiceAccountToken: false` — the server needs no Kubernetes
  API access
- Durable blob store via PVC with `fsGroup` so the non-root UID can write
- `terminationGracePeriodSeconds: 30` covers in-flight upload drain

## Common overrides

| Key | Default | Purpose |
|---|---|---|
| `image.repository` / `image.tag` | `ghcr.io/abyo-software/ferro-oci-server` / `0.1.0` | image coordinates |
| `persistence.size` | `20Gi` | PVC size |
| `persistence.storageClass` | `""` | storage class (`-` forces `""`) |
| `persistence.existingClaim` | `""` | reuse a pre-provisioned PVC |
| `resources.limits.memory` | `512Mi` | size to concurrent-upload working set |
| `service.port` | `8080` | listen + service port |
| `serviceMonitor.enabled` | `true` | scrape `/metrics` on the `http` port |
