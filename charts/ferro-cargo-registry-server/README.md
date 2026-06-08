# ferro-cargo-registry-server Helm chart

Helm chart for
[ferro-cargo-registry-server](https://github.com/abyo-software/ferro-protocols),
a Rust-native Cargo sparse-index + crate publish registry server.

## Install

```bash
helm install ferro-cargo-registry-server charts/ferro-cargo-registry-server \
    --namespace registry --create-namespace
```

## Persistence

The server stores the sparse index and crate blobs on a filesystem
directory (`FERRO_CARGO_REGISTRY_DATA`). The chart provisions a
`PersistentVolumeClaim` (default `20Gi`, `ReadWriteOnce`) and mounts it at
`persistence.mountPath` (default `/var/lib/ferro-cargo-registry`). Point
at an existing volume with `persistence.existingClaim`, or disable
durability for ephemeral test installs with `persistence.enabled=false`
(emptyDir — the index + crates are lost on restart).

Because the default PVC is `ReadWriteOnce`, the Deployment uses the
`Recreate` strategy and `replicaCount` should stay at `1`. Scale out only
with a `ReadWriteMany` storage class or an external shared backend.

## External access

`apiHost` sets `FERRO_CARGO_REGISTRY_API`, the base URL advertised in
`/config.json` for cargo clients. Leave empty for in-cluster /
port-forward access; set it to your Ingress / LoadBalancer URL when
exposing the registry outside the cluster, otherwise external cargo
clients cannot resolve the advertised index/download URLs.

## Probes

Liveness is wired to `/live` and readiness to `/ready` (both return
`200 OK`). `/healthz` returns `{"status":"ok"}` and backs the image's
Docker `HEALTHCHECK`.

## Metrics / ServiceMonitor (roadmap)

ferro-cargo-registry-server does **not** yet expose a Prometheus
`/metrics` endpoint. The chart ships a `ServiceMonitor` template but it is
**disabled by default** (`serviceMonitor.enabled=false`). Enable it only
after the server build you run ships `/metrics`, otherwise
prometheus-operator will log scrape errors. The bundled Grafana dashboard
(`dashboards/grafana/ferro-cargo-registry-server.json`) is authored
against the intended metric names for the same reason.

## Quality posture

- Non-root (UID 65534), read-only rootfs, dropped capabilities,
  seccomp `RuntimeDefault`
- `automountServiceAccountToken: false` — the server needs no Kubernetes
  API access
- Durable index + crate store via PVC with `fsGroup` so the non-root UID
  can write
- `terminationGracePeriodSeconds: 30` covers in-flight publish drain

## Common overrides

| Key | Default | Purpose |
|---|---|---|
| `image.repository` / `image.tag` | `ghcr.io/abyo-software/ferro-cargo-registry-server` / `0.1.0` | image coordinates |
| `apiHost` | `""` | external base URL advertised in `/config.json` |
| `persistence.size` | `20Gi` | PVC size |
| `persistence.storageClass` | `""` | storage class (`-` forces `""`) |
| `persistence.existingClaim` | `""` | reuse a pre-provisioned PVC |
| `resources.limits.memory` | `512Mi` | size to concurrent-publish working set |
| `service.port` | `8081` | listen + service port |
| `serviceMonitor.enabled` | `false` | enable once `/metrics` exists |
