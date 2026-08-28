# Operating Wassette

This guide covers operational aspects of running Wassette in production, including logging, monitoring, and troubleshooting.

## Invocation Logging

Wassette provides comprehensive invocation logging for all tool and component calls, enabling you to monitor, debug, and audit your AI agent workflows.

### What Gets Logged

Every tool invocation in Wassette is logged with structured information including:
- Tool name and component ID
- Sanitized arguments (sensitive data automatically redacted)
- Execution timing (total duration, instantiation time, execution time)
- Success or failure outcome
- Error details when applicable

### Log Format

Wassette uses Rust's `tracing` crate for structured logging, producing logs in a consistent format:

```
TIMESTAMP LEVEL span_info: message field1=value1 field2=value2
```

**Example - Successful Invocation (with DEBUG level enabled):**
```
2025-11-02T18:32:15.123Z DEBUG tool_name="fetch" arguments="{\"url\":\"example.com\"}" Tool invocation started
2025-11-02T18:32:15.124Z DEBUG component_id="fetch_rs" function_name="fetch" Component function invocation started
2025-11-02T18:32:15.125Z DEBUG component_id="fetch_rs" instantiation_ms=5 Component instance created
2025-11-02T18:32:15.245Z DEBUG component_id="fetch_rs" function_name="fetch" total_duration_ms=125 instantiation_ms=5 execution_ms=120 WebAssembly component execution completed
2025-11-02T18:32:15.246Z DEBUG component_id="fetch_rs" function_name="fetch" Component function invocation completed successfully
2025-11-02T18:32:15.247Z DEBUG tool_name="fetch" duration_ms=125 outcome="success" Tool invocation completed successfully
```

**Example - Component Lifecycle (INFO level):**
```
2025-11-02T18:32:10.123Z INFO path="oci://ghcr.io/microsoft/fetch-rs:latest" component_id="fetch_rs" operation="load-component" Component loaded successfully
2025-11-02T18:35:20.456Z INFO component_id="fetch_rs" operation="unload-component" Component unloaded successfully
```

**Example - Failed Invocation:**
```
2025-11-02T18:32:20.123Z DEBUG tool_name="fetch" arguments="{\"url\":\"blocked.com\"}" Tool invocation started
2025-11-02T18:32:20.125Z ERROR tool_name="fetch" duration_ms=2 outcome="error" error="Network access denied: host 'blocked.com' not in allow list" Tool invocation failed
```

### Configuring Log Levels

Control the verbosity of logs using the `RUST_LOG` environment variable:

```bash
# Show only INFO and above (recommended for production)
# Shows component lifecycle events (load/unload) and errors only
RUST_LOG=info wassette serve

# Show DEBUG logs for detailed invocation tracking
# Shows all tool invocations, component calls, and timing information
RUST_LOG=debug wassette serve

# Show TRACE logs for maximum verbosity
RUST_LOG=trace wassette serve

# Filter logs by crate
RUST_LOG=mcp_server=debug,wassette=info wassette serve
```

**Log Level Breakdown:**
- **INFO**: Component lifecycle events (load/unload success), errors
- **DEBUG**: Tool invocations, component calls, execution timing, detailed operation tracking
- **ERROR**: All failures and error conditions
- **WARN**: Non-critical issues (e.g., built-in tools disabled)

### Log Output Location

Both Streamable HTTP and stdio write logs to stderr. For stdio, this avoids
interfering with the MCP protocol on stdout.

### Sensitive Data Protection

Wassette automatically sanitizes arguments to prevent logging sensitive information:

- **Redacted fields**: Any argument key containing "password", "secret", "token", or "key" is replaced with `<redacted>`
- **Length limits**: Long string values are truncated to 200 characters
- **Total size limits**: Total logged arguments are capped at 1000 characters

**Example:**
```
# Original arguments:
{"url": "api.example.com", "api_key": "sk-1234567890abcdef"}

# Logged arguments:
{"url": "api.example.com", "api_key": "<redacted>"}
```

### Log Fields Reference

| Field | Description | Log Level | Example |
|-------|-------------|-----------|---------|
| `tool_name` | Name of the tool being invoked | DEBUG | `"fetch"` |
| `component_id` | ID of the component being executed | DEBUG/INFO | `"fetch_rs"` |
| `function_name` | Name of the function being called | DEBUG | `"fetch"` |
| `arguments` | Sanitized tool arguments | DEBUG | `"{\"url\":\"example.com\"}"` |
| `duration_ms` | Total execution time in milliseconds | DEBUG | `125` |
| `instantiation_ms` | Time to instantiate the component | DEBUG | `5` |
| `execution_ms` | Time to execute the function | DEBUG | `120` |
| `outcome` | Result of invocation | DEBUG/ERROR | `"success"` or `"error"` |
| `error` | Error message (only present on failure) | ERROR | `"Network access denied"` |
| `operation` | Type of lifecycle operation | DEBUG/INFO | `"load-component"` or `"unload-component"` |
| `path` | Component path for load operations | DEBUG/INFO | `"oci://ghcr.io/..."` |

### Common Operations

**Find all failed invocations:**
```bash
wassette serve 2>&1 | grep "outcome=\"error\""
```

**Track slow invocations (>1000ms):**
```bash
wassette serve 2>&1 | grep "duration_ms" | awk -F'duration_ms=' '{print $2}' | awk '{if ($1+0>1000) print}'
```

**Count invocations by tool:**
```bash
wassette serve 2>&1 | grep "Tool invocation started" | grep -oP 'tool_name="\K[^"]+' | sort | uniq -c
```

## Monitoring

### Integration with Monitoring Tools

Wassette's structured logs can be integrated with common monitoring and observability platforms:

#### Prometheus/Grafana

Use log parsers to extract metrics from logs and expose them for Prometheus scraping.

#### ELK Stack (Elasticsearch, Logstash, Kibana)

Configure Logstash to parse Wassette's structured logs:

```ruby
filter {
  grok {
    match => { "message" => "%{TIMESTAMP_ISO8601:timestamp} %{LOGLEVEL:level} %{GREEDYDATA:log_data}" }
  }
  kv {
    source => "log_data"
    field_split => " "
    value_split => "="
  }
}
```

#### Splunk

Wassette's key-value format is automatically parsed by Splunk:

```
index=wassette tool_name=* | stats count by tool_name
```

### Health Checks

When running with StreamableHttp transport, Wassette provides health and readiness endpoints:

#### Endpoints

- **`/health`**: Returns HTTP 200 OK if the server is running
- **`/ready`**: Returns HTTP 200 with JSON `{"status":"ready"}` when the server is ready to accept requests
- **`/info`**: Returns version and build information as JSON

**Example Usage:**

```bash
# Check if server is running
curl -f http://localhost:9001/health

# Check readiness
curl http://localhost:9001/ready

# Get version and build info
curl http://localhost:9001/info | jq .
```

**Example Response from `/info`:**
```json
{
  "version": "0.3.5",
  "build_info": "0.3.5 version.BuildInfo{RustVersion:\"1.90.0\", BuildProfile:\"release\", BuildStatus:\"Clean\", GitTag:\"v0.3.5\", Version:\"abc1234\", GitRevision:\"abc1234\"}"
}
```

*Note: The version and build_info fields reflect the actual build and may differ from this example.*

#### Integration with Container Orchestration

Use health endpoints with Docker, Kubernetes, or other orchestration platforms:

**Docker:**
```bash
docker run --rm -p 9001:9001 \
  --health-cmd="curl -f http://localhost:9001/health || exit 1" \
  --health-interval=30s \
  --health-timeout=10s \
  --health-retries=3 \
  wassette:latest
```

**Kubernetes:**
```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 9001
  initialDelaySeconds: 10
  periodSeconds: 30

readinessProbe:
  httpGet:
    path: /ready
    port: 9001
  initialDelaySeconds: 5
  periodSeconds: 10
```

**Note**: Health endpoints are only available with `--streamable-http`
transport. For stdio transport, monitor the process status instead.

## Streamable HTTP Protocol Modes

Wassette serves both the session-based MCP lifecycle used before protocol
revision `2026-07-28` and the stateless lifecycle introduced by that revision.
Which one a request gets is decided per request, from the revision the request
declares, so no configuration is needed to support stateless clients.

Two options let an operator narrow that behaviour:

| Option | Environment variable | Config key | Default |
|--------|----------------------|------------|---------|
| `--legacy-sessions <BOOL>` | `WASSETTE_LEGACY_SESSIONS` | `legacy_sessions` | `true` |
| `--json-response [<BOOL>]` | `WASSETTE_JSON_RESPONSE` | `json_response` | `false` |

`--legacy-sessions=false` drops the older lifecycle: `initialize` no longer
mints an `Mcp-Session-Id`, and `GET /mcp` and `DELETE /mcp` return `405 Method
Not Allowed`. Use it only where every client speaks `2026-07-28` or later,
since older clients cannot fall back.

`--json-response` returns `application/json` for a request that produces a
single reply, instead of a request-scoped `text/event-stream`. This suits
proxies and gateways that buffer responses. A request that produces more than
one message (a subscription stream, or a handler that sends a notification
first) still uses an event stream, so nothing is dropped.

### Notifying Stateless Clients

A stateless client has no long-lived connection for the server to push to. To
hear about tool changes it opens a `subscriptions/listen` request and keeps the
response stream open; Wassette sends `notifications/tools/list_changed` on that
stream whenever a component is loaded or unloaded, including by another client.
Session-based clients keep receiving the same notification on their session
stream.

### Horizontal Scaling Is Not Supported

Do not run multiple Wassette instances behind a load balancer as if they were
interchangeable. Statelessness at the protocol layer does not make the server
stateless: `LifecycleManager` holds loaded components and their policies in
memory, so a `load-component` or a `grant-*` handled by instance A is invisible
to instance B, even when both instances share a component directory. A client
whose requests are balanced across instances will see the tool list and the
permission set change from request to request.

For a scaled deployment, run a single writable instance, or serve a fixed,
read-only tool set from every instance:

```bash
# Provision components at startup, then refuse runtime changes
wassette serve --streamable-http --manifest /etc/wassette/manifest.yaml --disable-builtin-tools
```

See the [provisioning manifest reference](https://github.com/microsoft/wassette/blob/main/examples/manifests/README.md)
for the format and complete examples.

With `--disable-builtin-tools` the management plane (loading, unloading and
permission grants) is rejected, so every instance keeps serving exactly the
tools it was provisioned with and the instances stay equivalent.

### Provisioning Failure Behaviour

By default `--manifest` provisioning is fail-fast at the level of the whole
manifest: every declared component is attempted, and if any of them fails the
server logs a summary of the failures and exits non-zero without listening.
This is the right behaviour when the manifest is a contract: an instance that
cannot serve its full tool set never takes traffic, and an orchestrator's
restart backoff surfaces the problem instead of hiding it.

`--continue-on-provisioning-failure` changes only what happens after the
failures are logged. The failure summary is still emitted at error level, a
warning records how many of the declared components provisioned and names each
one that did not, and the server starts with the components that did load:

```bash
wassette serve --streamable-http --manifest /etc/wassette/manifest.yaml \
  --disable-builtin-tools --continue-on-provisioning-failure
```

Use it when partial service is better than no service, for example under
Kubernetes with `--disable-builtin-tools`, where the manifest is the only way a
component can enter the server and one unreachable registry would otherwise turn
into a crash loop that also takes down the tools that were fine. Keep the
default when a partial tool set would be worse than an outage, such as when a
client cannot tell a missing tool from a tool that legitimately declined, or
when the deployment is expected to fail its rollout on a bad manifest.

Note that a degraded instance serves fewer tools than its peers, which
reintroduces exactly the divergence described above; combine it with a
single-instance deployment, or accept that the tool list varies between
instances until the failing component is reachable again.

## Performance Tuning

### Resource Limits

When running in containers, set appropriate resource limits:

```bash
docker run --memory="512m" --cpus="2" wassette:latest
```

### Component Precompilation

Wassette caches compiled WebAssembly components for faster startup. Ensure the component directory has write permissions for the wassette user to enable caching.

### Concurrent Requests

Wassette handles concurrent tool invocations efficiently using Tokio's async runtime. Monitor your system resources to determine optimal concurrency levels.

## Troubleshooting

### High Memory Usage

If you notice high memory usage:

1. Check for memory leaks in loaded components
2. Review component memory limits in policy files
3. Monitor the number of concurrent invocations

### Slow Tool Invocations

If tools are running slowly:

1. Check `instantiation_ms` and `execution_ms` in logs to identify bottlenecks
2. Review network permissions and latency for network-dependent tools
3. Ensure components are being cached (check for repeated compilation logs)

### Permission Errors

If tools fail with permission errors:

1. Review the component's policy file using `wassette policy get <component-id>`
2. Check logs for specific permission denials
3. Grant necessary permissions using `wassette permission grant` commands

### MCP Requests Return 403

If `/mcp` answers `403` while the server is plainly running and listening, the `Host`
header is being rejected before MCP dispatch. By default only loopback values are
accepted, as protection against DNS rebinding, so this is the expected result of
addressing the server by a container name, service name or DNS name.

Note that `/health`, `/ready` and `/info` sit outside `/mcp` and are not subject to this
check, so they answer normally while `/mcp` refuses. A reachability probe against
`/health` therefore proves the process is up and tells you nothing about whether MCP
clients can connect.

Add the name the clients actually use:

```bash
wassette serve --streamable-http --bind-address 0.0.0.0:9001 \
  --allowed-host wassette.internal
```

Changing `--bind-address` alone does not help, as the bind address and the `Host`
allowlist are independent.

Note also that a configured allowlist **replaces** the loopback default rather than
extending it. If `/mcp` started returning `403` for `localhost` right after you added
`--allowed-host`, that is why: list loopback explicitly alongside the deployment name.

Confirm what the server accepts by sending a real `initialize` request and comparing
status codes:

```bash
BODY='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"1"}}}'

# 200: this Host is on the allowlist
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:9001/mcp \
  -H 'Host: 127.0.0.1:9001' \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d "$BODY"

# 403: this Host is not
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:9001/mcp \
  -H 'Host: wassette.internal:9001' \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d "$BODY"
```

The headers and body matter for reading the result. `403` always means the `Host` check
rejected the request. Any other status means the `Host` was accepted and you are seeing
the MCP layer respond, so a bare `curl -X POST` with no body returns `406` rather than
`200` even when the `Host` is fine.

## Best Practices

1. **Use INFO level in production** for a good balance between visibility and performance
2. **Enable DEBUG level when debugging** specific issues
3. **Monitor log size** in long-running deployments
4. **Rotate logs** to prevent disk space issues
5. **Set up alerts** on error patterns for proactive monitoring
6. **Use centralized logging** in production environments
7. **Review invocation patterns** regularly to optimize component usage
8. **Archive logs** for audit and compliance purposes

## Related Documentation

- [CLI Reference](../reference/cli.md) - Command-line interface
- [Environment Variables](../reference/environment-variables.md) - Configuration options
- [Permissions](../reference/permissions.md) - Security and access control
