# Deploying Wassette

This directory contains deployment configurations for running Wassette on Kubernetes.

## Overview

Wassette can be deployed to Kubernetes using either raw manifests or Helm charts. Both options provide production-ready configurations with security best practices.

## Deployment Options

### 1. Kubernetes Manifests

Use the raw Kubernetes manifests for simple deployments or when you need full control over the configuration.

📁 **Location**: [`kubernetes/`](kubernetes/)

**Quick Start:**
```bash
kubectl apply -f kubernetes/deployment.yaml
```

**Features:**
- Simple YAML-based deployment
- Single file for easy management
- Namespace isolation
- Health checks and resource limits
- Security context configuration

**Use When:**
- You need a quick deployment
- You don't need advanced features like autoscaling
- You prefer direct kubectl management
- You want to customize the YAML directly

[Learn more →](kubernetes/README.md)

### 2. Helm Chart

Use the Helm chart for production deployments with advanced features and easier upgrades.

📁 **Location**: [`helm/wassette/`](helm/wassette/)

**Quick Start:**
```bash
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace
```

**Features:**
- Templated configuration with values
- Easy upgrades and rollbacks
- Autoscaling support
- Ingress configuration
- Network policies
- Persistent storage options
- Pod disruption budgets

**Use When:**
- You need production-ready deployment
- You want easy configuration management
- You need advanced features (autoscaling, ingress, etc.)
- You prefer Helm for package management
- You need to maintain multiple environments

[Learn more →](helm/wassette/README.md)

## Integration with Kagent

Wassette can be integrated with [kagent](https://github.com/kagent-dev/kagent), a Kubernetes-native framework for building AI agents. Kagent uses the `RemoteMCPServer` custom resource to connect to MCP servers like Wassette.

**Architecture:**
```
┌─────────────────┐         ┌──────────────────┐         ┌─────────────────┐
│  Kagent Agent   │ ◄─────► │  RemoteMCPServer │ ◄─────► │    Wassette     │
│                 │         │   (Kubernetes    │         │   MCP Server    │
│                 │         │      CRD)        │         │  (streamable-   │
│                 │         │                  │         │      http)      │
└─────────────────┘         └──────────────────┘         └─────────────────┘
                                                                  │
                                                                  ▼
                                                          ┌─────────────────┐
                                                          │  WebAssembly    │
                                                          │   Components    │
                                                          └─────────────────┘
```

**Quick Integration:**
```bash
# 1. Deploy Wassette
kubectl apply -f kubernetes/deployment.yaml

# 2. Create RemoteMCPServer resource
kubectl apply -f kubernetes/wassette-remotemcp.yaml

# 3. Verify integration
kubectl get remotemcpservers -n kagent
```

[Complete integration guide →](kubernetes/kagent-integration.md)

## Choosing the Right Deployment Method

| Feature | Kubernetes Manifests | Helm Chart |
|---------|---------------------|------------|
| **Ease of Setup** | ✅ Very Simple | ⚠️ Requires Helm |
| **Customization** | ✅ Direct YAML editing | ✅ Values-based config |
| **Upgrades** | ⚠️ Manual kubectl apply | ✅ helm upgrade |
| **Rollbacks** | ❌ Manual | ✅ helm rollback |
| **Autoscaling** | ❌ Not included | ✅ Built-in |
| **Ingress** | ❌ Not included | ✅ Built-in |
| **Persistent Storage** | ⚠️ Manual configuration | ✅ Built-in |
| **Network Policies** | ❌ Not included | ✅ Built-in |
| **Production Ready** | ⚠️ Basic features | ✅ Full features |
| **Multi-Environment** | ⚠️ Difficult | ✅ Easy with values |

## Common Configurations

### Minimal Deployment

For testing or development:

```bash
# Using kubectl
kubectl apply -f kubernetes/deployment.yaml

# Using Helm
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace
```

### Production Deployment

For production with high availability:

```bash
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace \
  --set replicaCount=3 \
  --set persistence.enabled=true \
  --set autoscaling.enabled=true \
  --set podDisruptionBudget.enabled=true \
  --set networkPolicy.enabled=true
```

### With Persistent Storage

To persist components across restarts:

```bash
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace \
  --set persistence.enabled=true \
  --set persistence.size=5Gi
```

### With Secrets

To provide API keys to components:

```bash
# Create values file
cat > values.yaml <<EOF
secrets:
  create: true
  data:
    OPENWEATHER_API_KEY: "your-api-key"
    GITHUB_TOKEN: "your-token"
EOF

# Install with secrets
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace \
  --values values.yaml
```

## Transport Protocols

Wassette supports multiple MCP transport protocols:

### 1. Streamable HTTP (Default)

Recommended for Kubernetes deployments and kagent integration.

```bash
# Helm
--set wassette.transport=streamable-http

# The service is accessible at:
# http://wassette.namespace.svc.cluster.local:9001
```

### 2. SSE (Server-Sent Events)

Alternative HTTP-based transport.

```bash
# Helm
--set wassette.transport=sse

# The service is accessible at:
# http://wassette.namespace.svc.cluster.local:9001/sse
```

### 3. Stdio

For command-line integration (not recommended for Kubernetes).

## Accessing Wassette

### From Within the Cluster

Wassette is accessible via the service DNS name:

```
http://wassette.wassette.svc.cluster.local:9001
```

Or if in the same namespace:

```
http://wassette:9001
```

### From Outside the Cluster

#### Option 1: Port Forwarding (Development)

```bash
kubectl port-forward -n wassette svc/wassette 9001:9001
```

Then access at `http://localhost:9001`

#### Option 2: Ingress (Production)

Enable ingress in the Helm chart:

```bash
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace \
  --set ingress.enabled=true \
  --set ingress.className=nginx \
  --set ingress.hosts[0].host=wassette.example.com
```

## Monitoring and Observability

### Health Checks

Wassette exposes a health endpoint at `/health`:

```bash
kubectl run -it --rm debug --image=curlimages/curl --restart=Never -n wassette -- \
  curl http://wassette:9001/health
```

### Logs

View Wassette logs:

```bash
# Current logs
kubectl logs -n wassette deployment/wassette

# Follow logs
kubectl logs -n wassette deployment/wassette -f

# Logs from all replicas
kubectl logs -n wassette -l app.kubernetes.io/name=wassette --all-containers
```

### Metrics

Configure the log level:

```bash
# Helm
--set wassette.env.RUST_LOG=debug

# Kubectl (edit deployment)
kubectl set env -n wassette deployment/wassette RUST_LOG=debug
```

## Security Best Practices

1. **Run as Non-Root**: Both configurations run Wassette as user 1000
2. **Drop Capabilities**: All unnecessary Linux capabilities are dropped
3. **Network Policies**: Enable network policies to restrict traffic
4. **Resource Limits**: Set appropriate CPU and memory limits
5. **Read-Only Secrets**: Mount secrets as read-only
6. **Pod Security**: Use pod security standards

## Troubleshooting

### Pod Not Starting

```bash
kubectl get pods -n wassette
kubectl describe pod -n wassette -l app.kubernetes.io/name=wassette
kubectl logs -n wassette deployment/wassette
```

### Service Not Accessible

```bash
# Check service endpoints
kubectl get endpoints -n wassette wassette

# Test from within cluster
kubectl run -it --rm debug --image=curlimages/curl --restart=Never -n wassette -- \
  curl http://wassette:9001/health
```

### Kagent Integration Issues

See the [kagent integration guide](kubernetes/kagent-integration.md) for detailed troubleshooting.

## Next Steps

- 📖 [Kubernetes Deployment Guide](kubernetes/README.md)
- 📦 [Helm Chart Documentation](helm/wassette/README.md)
- 🔗 [Kagent Integration Guide](kubernetes/kagent-integration.md)
- 🏠 [Wassette Documentation](https://microsoft.github.io/wassette)
- 🐙 [Wassette GitHub](https://github.com/microsoft/wassette)

## Contributing

Found an issue or want to improve the deployment configurations? Please open an issue or pull request on the [Wassette GitHub repository](https://github.com/microsoft/wassette).

## License

This project is licensed under the MIT License. See [LICENSE](../LICENSE) for details.
