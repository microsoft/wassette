# Integrating Wassette with Kagent

This guide explains how to integrate Wassette as a remote MCP server with [kagent](https://github.com/kagent-dev/kagent), a Kubernetes-native framework for building AI agents.

## Overview

Kagent supports connecting to remote MCP servers using the `RemoteMCPServer` custom resource. Wassette can be deployed as a remote MCP server that provides WebAssembly-based tools to kagent agents.

## Architecture

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

## Prerequisites

1. **Kagent installed** on your Kubernetes cluster
   - Follow the [kagent installation guide](https://kagent.dev/docs/kagent/introduction/installation)

2. **Wassette deployed** in the same cluster
   - Use the Kubernetes deployment or Helm chart from this repository

3. **kubectl configured** to access your cluster

## Deployment Options

### Option 1: Deploy Wassette in the Same Namespace as Kagent

If deploying in the `kagent` namespace:

```bash
# Deploy Wassette to the kagent namespace
kubectl apply -f deployment.yaml -n kagent
```

Modify `deployment.yaml` to remove the namespace creation and use `kagent` namespace instead.

### Option 2: Deploy Wassette in a Separate Namespace

Deploy Wassette in its own namespace and access it via service DNS:

```bash
# Deploy to wassette namespace
kubectl apply -f deployment.yaml
```

The service will be accessible at: `http://wassette.wassette.svc.cluster.local:9001`

## Creating a RemoteMCPServer Resource

Create a `RemoteMCPServer` custom resource to connect kagent to Wassette:

```yaml
apiVersion: kagent.dev/v1alpha2
kind: RemoteMCPServer
metadata:
  name: wassette-mcp
  namespace: kagent
spec:
  description: "Wassette MCP server providing WebAssembly-based tools"
  protocol: STREAMABLE_HTTP
  url: "http://wassette.wassette.svc.cluster.local:9001"
  timeout: "30s"
  terminateOnClose: true
```

Apply the resource:

```bash
kubectl apply -f wassette-remotemcp.yaml
```

### If Wassette is in the Same Namespace

```yaml
apiVersion: kagent.dev/v1alpha2
kind: RemoteMCPServer
metadata:
  name: wassette-mcp
  namespace: kagent
spec:
  description: "Wassette MCP server providing WebAssembly-based tools"
  protocol: STREAMABLE_HTTP
  url: "http://wassette:9001"
  timeout: "30s"
  terminateOnClose: true
```

## Verifying the Integration

1. Check the RemoteMCPServer status:

```bash
kubectl get remotemcpservers -n kagent
kubectl describe remotemcpserver wassette-mcp -n kagent
```

Expected output:
```
Name:         wassette-mcp
Namespace:    kagent
Protocol:     STREAMABLE_HTTP
URL:          http://wassette.wassette.svc.cluster.local:9001
Accepted:     True
```

2. Check discovered tools:

```bash
kubectl get remotemcpserver wassette-mcp -n kagent -o jsonpath='{.status.discoveredTools}' | jq
```

This should list all tools available from Wassette's loaded components.

## Using Wassette Tools in Kagent Agents

Once the RemoteMCPServer is configured, you can reference Wassette tools in your Agent definitions:

```yaml
apiVersion: kagent.dev/v1alpha2
kind: Agent
metadata:
  name: my-agent
  namespace: kagent
spec:
  systemPrompt: "You are a helpful assistant with access to web fetching capabilities."
  modelConfigRef:
    name: default-model
  toolServers:
  - name: wassette-mcp
    tools:
    - fetch  # Tool provided by Wassette's fetch-rs component
```

## Loading Components into Wassette

Wassette can load WebAssembly components from OCI registries. To make tools available to kagent:

1. Use the Wassette MCP interface to load components
2. Or pre-load components by mounting them as volumes

### Pre-loading Components

Modify the Wassette deployment to include components:

```yaml
spec:
  template:
    spec:
      containers:
      - name: wassette
        volumeMounts:
        - name: components
          mountPath: /home/wassette/.local/share/wassette/components
        - name: fetch-component
          mountPath: /home/wassette/.local/share/wassette/components/fetch-rs
          readOnly: true
      volumes:
      - name: components
        emptyDir: {}
      - name: fetch-component
        configMap:
          name: fetch-rs-component
```

## Example: Deploying Wassette with Fetch Component

1. Build the fetch-rs component:

```bash
cd examples/fetch-rs
cargo build --release --target wasm32-wasip2
```

2. Create a ConfigMap with the component:

```bash
kubectl create configmap fetch-rs-component \
  -n wassette \
  --from-file=fetch_rs.wasm=target/wasm32-wasip2/release/fetch_rs.wasm
```

3. Update the deployment to mount the component

4. Create the RemoteMCPServer resource

5. Use the tool in a kagent Agent

## Advanced Configuration

### Authentication with Headers

If Wassette requires authentication headers:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: wassette-auth
  namespace: kagent
type: Opaque
stringData:
  api-key: "your-secret-api-key"
---
apiVersion: kagent.dev/v1alpha2
kind: RemoteMCPServer
metadata:
  name: wassette-mcp
  namespace: kagent
spec:
  description: "Wassette MCP server with authentication"
  protocol: STREAMABLE_HTTP
  url: "http://wassette.wassette.svc.cluster.local:9001"
  headersFrom:
  - name: "Authorization"
    valueFrom:
      type: Secret
      name: wassette-auth
      key: api-key
```

### Custom Timeouts

Adjust timeouts for long-running tools:

```yaml
spec:
  timeout: "120s"
  sseReadTimeout: "60s"
```

## Network Policies

For security, restrict network access to Wassette:

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: wassette-access
  namespace: wassette
spec:
  podSelector:
    matchLabels:
      app: wassette
  policyTypes:
  - Ingress
  ingress:
  - from:
    - namespaceSelector:
        matchLabels:
          name: kagent
    ports:
    - protocol: TCP
      port: 9001
```

## Troubleshooting

### RemoteMCPServer shows Accepted: False

Check the status conditions:
```bash
kubectl describe remotemcpserver wassette-mcp -n kagent
```

Common issues:
- URL is not accessible from kagent namespace
- Protocol mismatch (ensure using STREAMABLE_HTTP)
- Wassette pod is not running

### No tools discovered

Verify Wassette has components loaded:
```bash
kubectl logs -n wassette deployment/wassette
```

Test the endpoint directly:
```bash
kubectl run -it --rm debug --image=curlimages/curl --restart=Never -n kagent -- \
  curl -X POST http://wassette.wassette.svc.cluster.local:9001 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc": "2.0", "method": "tools/list", "id": 1}'
```

### Agent cannot use Wassette tools

Check agent logs:
```bash
kubectl logs -n kagent -l kagent.dev/agent=my-agent
```

Verify the tool name matches the component's exported tool name.

## Complete Example

A complete example combining Wassette deployment, RemoteMCPServer, and Agent:

```bash
# 1. Deploy Wassette
kubectl apply -f deployment/kubernetes/deployment.yaml

# 2. Create RemoteMCPServer
kubectl apply -f - <<EOF
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
EOF

# 3. Verify connection
kubectl wait --for=condition=Accepted remotemcpserver/wassette-mcp -n kagent --timeout=60s

# 4. Create an agent that uses Wassette
kubectl apply -f - <<EOF
apiVersion: kagent.dev/v1alpha2
kind: Agent
metadata:
  name: wasm-agent
  namespace: kagent
spec:
  systemPrompt: "You have access to WebAssembly-based tools via Wassette."
  modelConfigRef:
    name: default-model
  toolServers:
  - name: wassette-mcp
EOF
```

## Resources

- [Kagent Documentation](https://kagent.dev/docs/kagent)
- [Wassette Documentation](https://microsoft.github.io/wassette)
- [MCP Protocol Specification](https://modelcontextprotocol.io/)
- [Kagent RemoteMCPServer API Reference](https://kagent.dev/docs/kagent/api-reference/remotemcpserver)

## Contributing

Found an issue with this integration? Please report it on the [Wassette GitHub repository](https://github.com/microsoft/wassette/issues).
