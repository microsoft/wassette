# Kubernetes Deployment

This guide covers deploying Wassette on Kubernetes clusters, including integration with the [kagent](https://github.com/kagent-dev/kagent) framework.

## Overview

Wassette can be deployed to Kubernetes using either:
- **Raw Kubernetes manifests** - For simple deployments
- **Helm charts** - For production deployments with advanced features

Both deployment methods are production-ready and include security best practices.

## Quick Start

### Using Kubernetes Manifests

```bash
kubectl apply -f https://raw.githubusercontent.com/microsoft/wassette/main/deployment/kubernetes/deployment.yaml
```

This creates a namespace, deployment, and service for Wassette.

### Using Helm Chart

```bash
# From the repository root
helm install wassette deployment/helm/wassette/ \
  --namespace wassette \
  --create-namespace
```

## Deployment Options

### Kubernetes Manifests

**Location**: [`deployment/kubernetes/`](https://github.com/microsoft/wassette/tree/main/deployment/kubernetes)

Simple YAML-based deployment suitable for:
- Quick deployments
- Development and testing
- When you need full control over YAML
- Simple production deployments

[View detailed guide →](https://github.com/microsoft/wassette/blob/main/deployment/kubernetes/README.md)

### Helm Chart

**Location**: [`deployment/helm/wassette/`](https://github.com/microsoft/wassette/tree/main/deployment/helm/wassette)

Production-ready Helm chart with:
- Autoscaling (HPA)
- Ingress configuration
- Network policies
- Persistent storage
- Pod disruption budgets
- ConfigMaps and Secrets management

[View detailed guide →](https://github.com/microsoft/wassette/blob/main/deployment/helm/wassette/README.md)

## Integration with Kagent

Wassette integrates seamlessly with [kagent](https://github.com/kagent-dev/kagent), a Kubernetes-native framework for building AI agents. Kagent uses the `RemoteMCPServer` custom resource to connect to MCP servers like Wassette.

### Architecture

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

### Quick Integration

1. **Deploy Wassette**:
   ```bash
   kubectl apply -f https://raw.githubusercontent.com/microsoft/wassette/main/deployment/kubernetes/deployment.yaml
   ```

2. **Create RemoteMCPServer resource**:
   ```bash
   kubectl apply -f https://raw.githubusercontent.com/microsoft/wassette/main/deployment/kubernetes/wassette-remotemcp.yaml
   ```

3. **Verify connection**:
   ```bash
   kubectl get remotemcpservers -n kagent
   kubectl describe remotemcpserver wassette-mcp -n kagent
   ```

4. **Use in an Agent**:
   ```yaml
   apiVersion: kagent.dev/v1alpha2
   kind: Agent
   metadata:
     name: my-agent
     namespace: kagent
   spec:
     systemPrompt: "You have access to WebAssembly-based tools."
     modelConfigRef:
       name: default-model
     toolServers:
       - name: wassette-mcp
   ```

[Complete integration guide →](https://github.com/microsoft/wassette/blob/main/deployment/kubernetes/kagent-integration.md)

## Transport Protocols

Wassette supports multiple MCP transport protocols in Kubernetes:

### Streamable HTTP (Recommended)

Default transport for Kubernetes deployments. Best performance and compatibility with kagent.

```bash
# Service URL
http://wassette.wassette.svc.cluster.local:9001
```

### SSE (Server-Sent Events)

Alternative HTTP-based transport:

```bash
# With Helm
helm install wassette deployment/helm/wassette/ \
  --set wassette.transport=sse

# Service URL
http://wassette.wassette.svc.cluster.local:9001/sse
```

## Configuration

### Resource Limits

Adjust CPU and memory limits for your workload:

```bash
# Using Helm
helm install wassette deployment/helm/wassette/ \
  --set resources.requests.cpu=200m \
  --set resources.requests.memory=256Mi \
  --set resources.limits.cpu=1000m \
  --set resources.limits.memory=1Gi
```

### Persistent Storage

Enable persistent storage for components:

```bash
helm install wassette deployment/helm/wassette/ \
  --set persistence.enabled=true \
  --set persistence.size=5Gi
```

### Secrets

Provide API keys and credentials:

```bash
# Create a values file
cat > values.yaml <<EOF
secrets:
  create: true
  data:
    OPENWEATHER_API_KEY: "your-api-key"
    GITHUB_TOKEN: "your-token"
EOF

# Install with secrets
helm install wassette deployment/helm/wassette/ \
  --values values.yaml
```

## High Availability

Deploy Wassette with high availability:

```bash
helm install wassette deployment/helm/wassette/ \
  --set replicaCount=3 \
  --set autoscaling.enabled=true \
  --set autoscaling.minReplicas=2 \
  --set autoscaling.maxReplicas=5 \
  --set podDisruptionBudget.enabled=true
```

## Network Access

### Internal (Cluster-Only)

Wassette is accessible within the cluster at:

```
http://wassette.wassette.svc.cluster.local:9001
```

Or from the same namespace:

```
http://wassette:9001
```

### External Access via Port Forwarding

For development:

```bash
kubectl port-forward -n wassette svc/wassette 9001:9001
```

Then access at `http://localhost:9001`

### External Access via Ingress

For production:

```bash
helm install wassette deployment/helm/wassette/ \
  --set ingress.enabled=true \
  --set ingress.className=nginx \
  --set ingress.hosts[0].host=wassette.example.com
```

## Monitoring

### Health Checks

Check the health endpoint:

```bash
kubectl run -it --rm debug --image=curlimages/curl --restart=Never -n wassette -- \
  curl http://wassette:9001/health
```

### Logs

View logs:

```bash
# Current logs
kubectl logs -n wassette deployment/wassette

# Follow logs
kubectl logs -n wassette deployment/wassette -f

# All replicas
kubectl logs -n wassette -l app.kubernetes.io/name=wassette --all-containers
```

### Metrics

Set log level:

```bash
# Using Helm
helm upgrade wassette deployment/helm/wassette/ \
  --set wassette.env.RUST_LOG=debug

# Using kubectl
kubectl set env -n wassette deployment/wassette RUST_LOG=debug
```

## Security

### Security Features

Both deployment methods include:
- Non-root user (UID 1000)
- Dropped Linux capabilities
- Security context configuration
- Resource limits
- Health checks

### Network Policies

Enable network policies to restrict traffic:

```bash
helm install wassette deployment/helm/wassette/ \
  --set networkPolicy.enabled=true
```

### Read-Only Secrets

Secrets are always mounted as read-only for security.

## Troubleshooting

### Pod Not Starting

```bash
kubectl get pods -n wassette
kubectl describe pod -n wassette -l app.kubernetes.io/name=wassette
kubectl logs -n wassette deployment/wassette
```

### Service Not Accessible

```bash
# Check endpoints
kubectl get endpoints -n wassette wassette

# Test from within cluster
kubectl run -it --rm debug --image=curlimages/curl --restart=Never -n wassette -- \
  curl http://wassette:9001/health
```

### Kagent Integration Issues

See the [kagent integration troubleshooting guide](https://github.com/microsoft/wassette/blob/main/deployment/kubernetes/kagent-integration.md#troubleshooting).

## Comparison: Manifests vs Helm

| Feature | Kubernetes Manifests | Helm Chart |
|---------|---------------------|------------|
| Ease of Setup | ✅ Very Simple | ⚠️ Requires Helm |
| Customization | ✅ Direct YAML | ✅ Values-based |
| Upgrades | ⚠️ Manual | ✅ helm upgrade |
| Rollbacks | ❌ Manual | ✅ helm rollback |
| Autoscaling | ❌ Not included | ✅ Built-in |
| Ingress | ❌ Not included | ✅ Built-in |
| Persistent Storage | ⚠️ Manual | ✅ Built-in |
| Network Policies | ❌ Not included | ✅ Built-in |
| Production Ready | ⚠️ Basic | ✅ Full features |

## Next Steps

- 📖 [Kubernetes Deployment Guide](https://github.com/microsoft/wassette/blob/main/deployment/kubernetes/README.md)
- 📦 [Helm Chart Documentation](https://github.com/microsoft/wassette/blob/main/deployment/helm/wassette/README.md)
- 🔗 [Kagent Integration Guide](https://github.com/microsoft/wassette/blob/main/deployment/kubernetes/kagent-integration.md)
- 🐳 [Docker](./docker.md)
- 🚀 [Operations Guide](./operations.md)

## Resources

- [Kagent Documentation](https://kagent.dev/docs/kagent)
- [Model Context Protocol](https://modelcontextprotocol.io/)
- [Kubernetes Documentation](https://kubernetes.io/docs/)
- [Helm Documentation](https://helm.sh/docs/)
