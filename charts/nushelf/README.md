# NuShelf Helm chart

Deploys [NuShelf](https://github.com/MCKanpolat/NuShelf) on Kubernetes. The chart runs the registry only; PostgreSQL and object storage are provided by you.

## Install

```bash
helm install nushelf oci://ghcr.io/mckanpolat/charts/nushelf \
  --namespace nushelf --create-namespace \
  --set publicUrl=https://nuget.example.com \
  --set secrets.existingSecret=nushelf-secrets
```

The Secret holds the two required values:

```bash
kubectl -n nushelf create secret generic nushelf-secrets \
  --from-literal=ADMIN_API_KEY="$(openssl rand -hex 32)" \
  --from-literal=DATABASE_URL='postgres://nushelf:<password>@<host>:5432/nushelf'
```

Migrations run on startup. Then open `<publicUrl>/manage`, sign in with `ADMIN_API_KEY` and create a publisher profile, as described in the [deployment guide](../../docs/deployment.md).

## Storage

| `config.storageProvider` | Settings | Notes |
| --- | --- | --- |
| `local` (default) | `persistence.*` | Packages on a volume at `/data/packages`. Without `persistence.enabled` they live in an `emptyDir` and are lost on reschedule. A `ReadWriteOnce` volume limits the release to one replica and uses the `Recreate` strategy. |
| `s3` | `storage.s3.*` | AWS S3 or an S3-compatible service. Leave credentials out to use workload identity (EKS Pod Identity or IRSA); otherwise pass `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` through `extraEnvFrom`. |
| `azure` | `storage.azure.*` | Pass `AZURE_STORAGE_ACCOUNT_KEY` through `extraEnvFrom`, or use workload identity. |
| `gcs` | `storage.gcs.*` | Mount a key with `extraVolumes` / `extraVolumeMounts` and set `serviceAccountPath`, or use workload identity. |

## Replicas

Background work (cache maintenance, webhook delivery, vulnerability data sync) is coordinated through PostgreSQL advisory locks and `SKIP LOCKED` queues, so several replicas can share one database and bucket. Keep `config.databaseConnections × replicaCount` within the connection limit of your PostgreSQL server.

## Metrics and alerts

`metrics.enabled` starts the Prometheus listener (`METRICS_BIND_ADDRESS`) on `metrics.port` and adds that port to the Service. Ingress and HTTPRoute only route the registry port, so `/metrics` is never published. With the Prometheus Operator:

```yaml
metrics:
  enabled: true
  serviceMonitor:
    enabled: true
    labels:
      release: prometheus   # match your Prometheus serviceMonitorSelector
  prometheusRule:
    enabled: true
```

The default rules are `NuShelfDown` (no scrapeable replica for 5 minutes), `NuShelfHighErrorRate` (5xx ratio above `errorRateThreshold` for 10 minutes) and `NuShelfSlowRequests` (p95 per route above `latencyThresholdSeconds` for 15 minutes, uploads excluded). Add your own under `prometheusRule.additionalRules`, or set `defaultRules: false`.

## Example: EKS with S3 and a Gateway API route

```yaml
publicUrl: https://nuget.example.com
replicaCount: 2
config:
  storageProvider: s3
storage:
  s3:
    bucket: nushelf-packages
    region: eu-central-1
secrets:
  existingSecret: nushelf-secrets
serviceAccount:
  # EKS Pod Identity needs no annotation: associate the role with this
  # ServiceAccount. For IRSA, annotate it instead:
  annotations: {}
  #  eks.amazonaws.com/role-arn: arn:aws:iam::123456789012:role/nushelf
podDisruptionBudget:
  enabled: true
httpRoute:
  enabled: true
  parentRefs:
    - name: gateway
      namespace: gateway-system
      sectionName: https
  hostnames:
    - nuget.example.com
resources:
  requests:
    cpu: 100m
    memory: 128Mi
  limits:
    memory: 512Mi
```

The role needs `s3:ListBucket` on the bucket and `s3:GetObject`, `s3:PutObject` and `s3:DeleteObject` on its objects.

Proxies in front of NuShelf must accept request bodies up to `config.maxPackageBytes`; with ingress-nginx, set `nginx.ingress.kubernetes.io/proxy-body-size`.

## Values

| Key | Default | Description |
| --- | --- | --- |
| `publicUrl` | `""` | **Required.** External HTTP(S) origin without a path; NuGet documents are built from it. |
| `replicaCount` | `1` | Pods. Needs shared storage above 1. |
| `image.repository` | `ghcr.io/mckanpolat/nushelf` | Image repository. |
| `image.tag` | chart `appVersion` | Image tag. |
| `config.storageProvider` | `local` | `local`, `s3`, `azure` or `gcs`. |
| `config.maxPackageBytes` | `104857600` | Largest accepted package (1 KiB to 1 GiB). |
| `config.databaseConnections` | `20` | PostgreSQL pool size per replica. |
| `config.logLevel` | `nushelf=info,tower_http=info` | `RUST_LOG` filter. |
| `secrets.existingSecret` | `""` | Secret with `ADMIN_API_KEY` and `DATABASE_URL`. |
| `secrets.adminApiKeyKey` / `databaseUrlKey` | `ADMIN_API_KEY` / `DATABASE_URL` | Key names inside that Secret. |
| `secrets.adminApiKey` / `databaseUrl` | `""` | Inline values for trials; the chart then creates the Secret. |
| `storage.s3.bucket` / `region` / `endpoint` | `""` | S3 settings. |
| `storage.s3.allowHttp` | `false` | Allow a plain HTTP endpoint (e.g. in-cluster MinIO). |
| `storage.s3.virtualHostedStyleRequest` | `""` | `false` for path-style addressing. |
| `storage.azure.accountName` / `containerName` | `""` | Azure Blob settings. |
| `storage.gcs.bucket` / `serviceAccountPath` | `""` | GCS settings. |
| `persistence.enabled` | `false` | Volume for the `local` provider. |
| `persistence.existingClaim` / `storageClass` / `size` / `accessModes` | | Volume settings; a created claim is kept on uninstall. |
| `serviceAccount.create` / `name` / `annotations` | `true` / `""` / `{}` | ServiceAccount, e.g. for workload identity. |
| `service.type` / `port` | `ClusterIP` / `80` | Service. |
| `ingress.*` | disabled | Standard Ingress. |
| `httpRoute.enabled` / `parentRefs` / `hostnames` / `rules` | disabled | Gateway API HTTPRoute; `rules` defaults to all paths. |
| `metrics.enabled` / `port` | `false` / `9090` | Prometheus listener on a separate port. |
| `metrics.serviceMonitor.enabled` / `interval` / `scrapeTimeout` / `labels` | `false` / `30s` / `10s` / `{}` | Prometheus Operator ServiceMonitor. |
| `metrics.prometheusRule.enabled` / `defaultRules` / `errorRateThreshold` / `latencyThresholdSeconds` / `additionalRules` | `false` / `true` / `0.05` / `2` / `[]` | Prometheus Operator alert rules. |
| `livenessProbe` / `readinessProbe` / `startupProbe` | `/health/live`, `/health/ready` | Probes. |
| `podDisruptionBudget.enabled` / `minAvailable` / `maxUnavailable` | `false` / `""` / `1` | PodDisruptionBudget. |
| `strategy` | `{}` | Deployment strategy override. |
| `podSecurityContext` / `securityContext` | non-root, read-only root | Security contexts. |
| `resources`, `nodeSelector`, `tolerations`, `affinity`, `topologySpreadConstraints`, `priorityClassName` | | Scheduling. |
| `extraEnv` / `extraEnvFrom` / `extraVolumes` / `extraVolumeMounts` | `[]` | Extra configuration, e.g. upstream credentials. |

`helm test <release>` checks `/health/ready` through the Service.
