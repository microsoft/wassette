# Wassette Helm Chart

This Helm chart deploys Wassette MCP server on a Kubernetes cluster.

## Prerequisites

- Kubernetes 1.19+
- Helm 3.0+
- (Optional) PersistentVolume provisioner support

## Installation

### Add the Helm Repository (Future)

```bash
# This will be available once the chart is published
# helm repo add wassette https://microsoft.github.io/wassette/helm
# helm repo update
```

### Install from Local Chart

```bash
# From the repository root
helm install wassette deployment/helm/wassette/ --namespace wassette --create-namespace
```

### Install with Custom Values

```bash
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace \
  --set image.tag=latest \
  --set resources.requests.memory=256Mi
```

### Install with Custom Values File

Create a `custom-values.yaml`:

```yaml
replicaCount: 2

resources:
  limits:
    cpu: 1000m
    memory: 1Gi
  requests:
    cpu: 200m
    memory: 256Mi

persistence:
  enabled: true
  size: 5Gi

ingress:
  enabled: true
  className: nginx
  hosts:
    - host: wassette.example.com
      paths:
        - path: /
          pathType: Prefix
```

Then install:

```bash
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace \
  --values custom-values.yaml
```

## Configuration

The following table lists the configurable parameters of the Wassette chart and their default values.

### Global Settings

| Parameter | Description | Default |
|-----------|-------------|---------|
| `replicaCount` | Number of Wassette replicas | `1` |
| `nameOverride` | Override the resource name | `""` |
| `fullnameOverride` | Override the full resource name | `""` |

### Image Settings

| Parameter | Description | Default |
|-----------|-------------|---------|
| `image.repository` | Wassette image repository | `ghcr.io/microsoft/wassette` |
| `image.pullPolicy` | Image pull policy | `IfNotPresent` |
| `image.tag` | Image tag (defaults to chart appVersion) | `""` |
| `imagePullSecrets` | Image pull secrets | `[]` |

### Service Account

| Parameter | Description | Default |
|-----------|-------------|---------|
| `serviceAccount.create` | Create a service account | `true` |
| `serviceAccount.annotations` | Service account annotations | `{}` |
| `serviceAccount.name` | Service account name | `""` |

### Security Context

| Parameter | Description | Default |
|-----------|-------------|---------|
| `podSecurityContext.runAsNonRoot` | Run as non-root user | `true` |
| `podSecurityContext.runAsUser` | User ID | `1000` |
| `podSecurityContext.fsGroup` | File system group | `1000` |
| `securityContext.allowPrivilegeEscalation` | Allow privilege escalation | `false` |
| `securityContext.readOnlyRootFilesystem` | Read-only root filesystem | `false` |

### Service

| Parameter | Description | Default |
|-----------|-------------|---------|
| `service.type` | Service type | `ClusterIP` |
| `service.port` | Service port | `9001` |
| `service.targetPort` | Container target port | `9001` |
| `service.annotations` | Service annotations | `{}` |

### Ingress

| Parameter | Description | Default |
|-----------|-------------|---------|
| `ingress.enabled` | Enable ingress | `false` |
| `ingress.className` | Ingress class name | `""` |
| `ingress.annotations` | Ingress annotations | `{}` |
| `ingress.hosts` | Ingress hosts | `[{host: wassette.example.com, paths: [{path: /, pathType: Prefix}]}]` |
| `ingress.tls` | Ingress TLS configuration | `[]` |

### Resources

| Parameter | Description | Default |
|-----------|-------------|---------|
| `resources.limits.cpu` | CPU limit | `500m` |
| `resources.limits.memory` | Memory limit | `512Mi` |
| `resources.requests.cpu` | CPU request | `100m` |
| `resources.requests.memory` | Memory request | `128Mi` |

### Wassette Configuration

| Parameter | Description | Default |
|-----------|-------------|---------|
| `wassette.transport` | Transport protocol (streamable-http, sse, stdio) | `streamable-http` |
| `wassette.env.RUST_LOG` | Logging level | `info` |
| `wassette.envFrom` | Additional env from configmap/secret | `[]` |

### Persistence

| Parameter | Description | Default |
|-----------|-------------|---------|
| `persistence.enabled` | Enable persistent storage | `false` |
| `persistence.storageClassName` | Storage class name | `""` |
| `persistence.accessMode` | Access mode | `ReadWriteOnce` |
| `persistence.size` | Volume size | `1Gi` |
| `persistence.existingClaim` | Existing PVC name | `""` |

### Secrets

| Parameter | Description | Default |
|-----------|-------------|---------|
| `secrets.create` | Create a secret | `false` |
| `secrets.name` | Secret name | `wassette-secrets` |
| `secrets.data` | Secret data | `{}` |

### Autoscaling

| Parameter | Description | Default |
|-----------|-------------|---------|
| `autoscaling.enabled` | Enable HPA | `false` |
| `autoscaling.minReplicas` | Minimum replicas | `1` |
| `autoscaling.maxReplicas` | Maximum replicas | `10` |
| `autoscaling.targetCPUUtilizationPercentage` | Target CPU utilization | `80` |

### Network Policy

| Parameter | Description | Default |
|-----------|-------------|---------|
| `networkPolicy.enabled` | Enable network policy | `false` |
| `networkPolicy.ingress` | Ingress rules | `[{from: [{namespaceSelector: {matchLabels: {name: kagent}}}], ports: [{protocol: TCP, port: 9001}]}]` |

## Examples

### Example 1: Basic Deployment

```bash
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace
```

### Example 2: With Persistent Storage

```bash
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace \
  --set persistence.enabled=true \
  --set persistence.size=5Gi
```

### Example 3: With Secrets

```bash
# Create values file
cat > values.yaml <<EOF
secrets:
  create: true
  data:
    OPENWEATHER_API_KEY: "your-api-key"
    GITHUB_TOKEN: "your-github-token"
EOF

# Install with secrets
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace \
  --values values.yaml
```

### Example 4: With Ingress

```bash
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace \
  --set ingress.enabled=true \
  --set ingress.className=nginx \
  --set ingress.hosts[0].host=wassette.example.com \
  --set ingress.hosts[0].paths[0].path=/ \
  --set ingress.hosts[0].paths[0].pathType=Prefix
```

### Example 5: High Availability Setup

```bash
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace \
  --set replicaCount=3 \
  --set persistence.enabled=true \
  --set autoscaling.enabled=true \
  --set autoscaling.minReplicas=2 \
  --set autoscaling.maxReplicas=5 \
  --set podDisruptionBudget.enabled=true \
  --set podDisruptionBudget.minAvailable=1
```

### Example 6: For Kagent Integration

```bash
# Install in the same namespace as kagent
helm install wassette deployment/helm/wassette/ \
  --namespace kagent \
  --set networkPolicy.enabled=true
```

## Upgrading

### Upgrade the Release

```bash
helm upgrade wassette deployment/helm/wassette/ \
  --namespace wassette \
  --values custom-values.yaml
```

### Upgrade with New Image

```bash
helm upgrade wassette deployment/helm/wassette/ \
  --namespace wassette \
  --set image.tag=v0.3.5
```

## Uninstallation

```bash
helm uninstall wassette --namespace wassette
```

To also delete the namespace:

```bash
kubectl delete namespace wassette
```

## Integration with Kagent

To use Wassette with [kagent](https://github.com/kagent-dev/kagent), create a RemoteMCPServer resource:

```yaml
apiVersion: kagent.dev/v1alpha2
kind: RemoteMCPServer
metadata:
  name: wassette-mcp
  namespace: kagent
spec:
  description: "Wassette WebAssembly MCP Server"
  protocol: STREAMABLE_HTTP
  url: "http://wassette.wassette.svc.cluster.local:9001"
  timeout: "30s"
```

For detailed integration instructions, see the [Kubernetes deployment guide](../../kubernetes/kagent-integration.md).

## Troubleshooting

### Check Pod Status

```bash
kubectl get pods -n wassette
kubectl describe pod -n wassette -l app.kubernetes.io/name=wassette
kubectl logs -n wassette -l app.kubernetes.io/name=wassette
```

### Test Service Connectivity

```bash
kubectl run -it --rm debug --image=curlimages/curl --restart=Never -n wassette -- \
  curl http://wassette:9001/health
```

### Verify Helm Release

```bash
helm list -n wassette
helm status wassette -n wassette
helm get values wassette -n wassette
```

## Development

### Lint the Chart

```bash
helm lint deployment/helm/wassette/
```

### Render Templates

```bash
helm template wassette deployment/helm/wassette/ --debug
```

### Dry Run

```bash
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --dry-run --debug
```

## Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](../../../CONTRIBUTING.md) for details.

## License

This project is licensed under the MIT License. See [LICENSE](../../../LICENSE) for details.
