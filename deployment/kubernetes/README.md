# Kubernetes Deployment for Wassette

This directory contains Kubernetes manifests for deploying Wassette as an MCP server with streamable-http protocol.

## Quick Start

### Prerequisites

- Kubernetes cluster (1.19+)
- `kubectl` configured to access your cluster
- (Optional) Docker registry access for custom images

### Basic Deployment

Deploy Wassette with default settings:

```bash
kubectl apply -f deployment.yaml
```

This creates:
- A `wassette` namespace
- A Deployment with 1 replica
- A ClusterIP Service on port 9001

### Verify Deployment

Check the deployment status:

```bash
kubectl get pods -n wassette
kubectl get svc -n wassette
```

View logs:

```bash
kubectl logs -n wassette deployment/wassette
```

### Access the Service

From within the cluster:
```
http://wassette.wassette.svc.cluster.local:9001
```

For external access, use port-forwarding:
```bash
kubectl port-forward -n wassette svc/wassette 9001:9001
```

Then connect to `http://localhost:9001`

## Configuration

### Environment Variables

You can customize Wassette behavior by modifying the `env` section in `deployment.yaml`:

```yaml
env:
- name: RUST_LOG
  value: "debug"  # Set log level: trace, debug, info, warn, error
```

### Secrets

To provide API keys or other secrets to components:

1. Create a secret:
```bash
kubectl create secret generic wassette-secrets \
  -n wassette \
  --from-literal=OPENWEATHER_API_KEY=your_api_key
```

2. The secret is automatically mounted at `/home/wassette/.config/wassette/secrets`

### Persistent Component Storage

To persist components across restarts, replace the `emptyDir` volume with a PersistentVolumeClaim:

```yaml
volumes:
- name: components
  persistentVolumeClaim:
    claimName: wassette-components
```

Create the PVC:
```bash
kubectl apply -f - <<EOF
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: wassette-components
  namespace: wassette
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: 1Gi
EOF
```

## Advanced Configuration

### Ingress

To expose Wassette via Ingress:

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: wassette
  namespace: wassette
  annotations:
    nginx.ingress.kubernetes.io/rewrite-target: /
spec:
  rules:
  - host: wassette.example.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: wassette
            port:
              number: 9001
```

### Resource Limits

Adjust CPU and memory limits in the deployment:

```yaml
resources:
  requests:
    cpu: 200m
    memory: 256Mi
  limits:
    cpu: 1000m
    memory: 1Gi
```

### Multiple Replicas

For high availability, increase replicas:

```yaml
spec:
  replicas: 3
```

**Note:** When using multiple replicas, ensure components are loaded consistently across all pods, or use a shared volume for component storage.

## Integration with Kagent

Wassette can be integrated with [kagent](https://github.com/kagent-dev/kagent) as a remote MCP server. See [kagent-integration.md](kagent-integration.md) for details.

## Troubleshooting

### Pod not starting

Check pod events:
```bash
kubectl describe pod -n wassette -l app=wassette
```

### Connection issues

Verify the service endpoints:
```bash
kubectl get endpoints -n wassette wassette
```

Test connectivity from within the cluster:
```bash
kubectl run -it --rm debug --image=curlimages/curl --restart=Never -n wassette -- \
  curl http://wassette:9001/health
```

### Permission issues

Ensure the pod is running as the correct user (UID 1000):
```bash
kubectl exec -it -n wassette deployment/wassette -- id
```

## Cleanup

Remove all resources:

```bash
kubectl delete namespace wassette
```

Or just the deployment:

```bash
kubectl delete -f deployment.yaml
```

## Next Steps

- Configure [Helm deployment](../helm/wassette/README.md) for production
- Integrate with [kagent](kagent-integration.md)
- Set up monitoring and observability
- Configure network policies for security
