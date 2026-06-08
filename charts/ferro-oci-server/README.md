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

## Metrics / ServiceMonitor (roadmap)

ferro-oci-server does **not** yet expose a Prometheus `/metrics` endpoint.
The chart ships a `ServiceMonitor` template but it is **disabled by
default** (`serviceMonitor.enabled=false`). Enable it only after the
server build you run ships `/metrics`, otherwise prometheus-operator will
log scrape errors. The bundled Grafana dashboard
(`dashboards/grafana/ferro-oci-server.json`) is authored against the
intended metric names for the same reason.

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
| `serviceMonitor.enabled` | `false` | enable once `/metrics` exists |
