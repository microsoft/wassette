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

The log output location depends on the transport mode:

- **SSE and StreamableHttp**: Logs go to stdout
- **Stdio**: Logs go to stderr (to avoid interfering with the MCP protocol on stdout)

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

**Note**: Health endpoints are only available with `--streamable-http` transport. SSE transport (`--sse`) also uses HTTP but is designed solely for event streaming and does not provide a general HTTP request/response interface. For stdio or SSE transports, monitor the process status instead.

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

- [Docker Deployment](./docker.md) - Running Wassette in containers
- [CLI Reference](../reference/cli.md) - Command-line interface
- [Environment Variables](../reference/environment-variables.md) - Configuration options
- [Permissions](../reference/permissions.md) - Security and access control
