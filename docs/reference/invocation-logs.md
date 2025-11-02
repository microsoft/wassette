# Invocation Logs

Wassette provides comprehensive invocation logging for all tool and component calls, enabling users to monitor, debug, and audit their AI agent workflows.

## Overview

Every tool invocation in Wassette is logged with structured information including:
- Tool name and component ID
- Sanitized arguments (sensitive data redacted)
- Execution timing (total duration, instantiation time, execution time)
- Success or failure outcome
- Error details when applicable

## Log Format

Wassette uses Rust's `tracing` crate for structured logging, producing logs in a consistent format:

```
TIMESTAMP LEVEL span_info: message field1=value1 field2=value2
```

### Example Log Output

Here's a complete example of a successful tool invocation:

```
2025-11-02T18:32:15.123Z INFO tool_name="fetch" arguments="{\"url\":\"example.com\"}" Tool invocation started
2025-11-02T18:32:15.124Z INFO component_id="fetch_rs" function_name="fetch" Component function invocation started
2025-11-02T18:32:15.125Z DEBUG component_id="fetch_rs" instantiation_ms=5 Component instance created
2025-11-02T18:32:15.245Z INFO component_id="fetch_rs" function_name="fetch" total_duration_ms=125 instantiation_ms=5 execution_ms=120 WebAssembly component execution completed
2025-11-02T18:32:15.246Z INFO component_id="fetch_rs" function_name="fetch" Component function invocation completed successfully
2025-11-02T18:32:15.247Z INFO tool_name="fetch" duration_ms=125 outcome="success" Tool invocation completed successfully
```

### Failed Invocation Example

When a tool invocation fails, error details are included:

```
2025-11-02T18:32:20.123Z INFO tool_name="fetch" arguments="{\"url\":\"blocked.com\"}" Tool invocation started
2025-11-02T18:32:20.125Z ERROR tool_name="fetch" duration_ms=2 outcome="error" error="Network access denied: host 'blocked.com' not in allow list" Tool invocation failed
```

## Log Levels

Wassette uses different log levels for different types of information:

- **INFO**: Tool invocations, successful operations, component lifecycle events
- **ERROR**: Failed invocations, errors during execution
- **DEBUG**: Detailed execution information (instantiation timing, intermediate steps)
- **WARN**: Non-critical issues (e.g., builtin tools disabled)

## Configuring Log Output

### Setting Log Level

Control the verbosity of logs using the `RUST_LOG` environment variable:

```bash
# Show only INFO and above (default for production)
RUST_LOG=info wassette serve

# Show DEBUG logs for more detailed information
RUST_LOG=debug wassette serve

# Show TRACE logs for maximum verbosity
RUST_LOG=trace wassette serve

# Filter logs by crate
RUST_LOG=mcp_server=debug,wassette=info wassette serve
```

### Log Output Location

The log output location depends on the transport mode:

- **SSE and StreamableHttp**: Logs go to stdout
- **Stdio**: Logs go to stderr (to avoid interfering with the MCP protocol on stdout)

### Structured Logging

Wassette's logs are structured and can be parsed programmatically. Each log entry contains key-value pairs that can be extracted for analysis:

```bash
# Extract all tool invocations with duration
wassette serve 2>&1 | grep "Tool invocation completed" | grep -oP 'duration_ms=\K\d+'

# Count successful vs failed invocations
wassette serve 2>&1 | grep "Tool invocation completed" | grep -c "outcome=\"success\""
wassette serve 2>&1 | grep "Tool invocation" | grep -c "outcome=\"error\""
```

## Log Fields Reference

### Tool Invocation Logs

| Field | Description | Example |
|-------|-------------|---------|
| `tool_name` | Name of the tool being invoked | `"fetch"` |
| `arguments` | Sanitized tool arguments | `"{\"url\":\"example.com\"}"` |
| `duration_ms` | Total execution time in milliseconds | `125` |
| `outcome` | Result of invocation | `"success"` or `"error"` |
| `error` | Error message (only present on failure) | `"Network access denied"` |

### Component Execution Logs

| Field | Description | Example |
|-------|-------------|---------|
| `component_id` | ID of the component being executed | `"fetch_rs"` |
| `function_name` | Name of the function being called | `"fetch"` |
| `total_duration_ms` | Total execution time | `125` |
| `instantiation_ms` | Time to instantiate the component | `5` |
| `execution_ms` | Time to execute the function | `120` |

### Component Lifecycle Logs

| Field | Description | Example |
|-------|-------------|---------|
| `operation` | Type of operation | `"load-component"` or `"unload-component"` |
| `path` | Path to component (load only) | `"oci://ghcr.io/microsoft/fetch-rs:latest"` |
| `component_id` | ID of the component | `"fetch_rs"` |

## Sensitive Data Protection

Wassette automatically sanitizes arguments to prevent logging sensitive information:

- **Redacted fields**: Any argument key containing "password", "secret", "token", or "key" is replaced with `<redacted>`
- **Length limits**: Long string values are truncated to 200 characters
- **Total size limits**: Total logged arguments are capped at 1000 characters

Example:
```
# Original arguments:
{"url": "api.example.com", "api_key": "sk-1234567890abcdef", "data": "very long text..."}

# Logged arguments:
{"url": "api.example.com", "api_key": "<redacted>", "data": "very long text... (500 chars)"}
```

## Use Cases

### Debugging Tool Failures

When a tool fails, examine the logs to understand what happened:

```bash
# Find all failed invocations
wassette serve 2>&1 | grep "outcome=\"error\""

# Get error details for a specific tool
wassette serve 2>&1 | grep "tool_name=\"fetch\"" | grep "error="
```

### Performance Monitoring

Track execution times to identify performance bottlenecks:

```bash
# Find slow invocations (>1000ms)
wassette serve 2>&1 | grep "duration_ms" | awk -F'duration_ms=' '{print $2}' | awk '{print $1}' | awk -F' ' '$1+0>1000'

# Average execution time for a specific tool
wassette serve 2>&1 | grep "tool_name=\"fetch\"" | grep "duration_ms" | awk -F'duration_ms=' '{print $2}' | awk '{print $1}' | awk '{sum+=$1; count++} END {print sum/count}'
```

### Auditing Tool Usage

Track which tools are being used and when:

```bash
# Count invocations by tool
wassette serve 2>&1 | grep "Tool invocation started" | grep -oP 'tool_name="\K[^"]+' | sort | uniq -c

# List all loaded components
wassette serve 2>&1 | grep "Component loaded successfully"
```

## Best Practices

1. **Use INFO level in production** for a good balance between visibility and performance
2. **Enable DEBUG level when debugging** specific issues
3. **Monitor log size** in long-running deployments
4. **Use log aggregation tools** (e.g., ELK stack, Splunk) for centralized logging in production
5. **Set up alerts** on error patterns for proactive monitoring
6. **Rotate logs** to prevent disk space issues

## Integration with Monitoring Tools

Wassette's structured logs can be easily integrated with monitoring and observability platforms:

### Prometheus/Grafana

Use log parsers to extract metrics from logs and expose them for Prometheus scraping.

### ELK Stack (Elasticsearch, Logstash, Kibana)

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

### Splunk

Wassette's key-value format is automatically parsed by Splunk, making it easy to search and visualize:

```
index=wassette sourcetype=wassette_logs tool_name=* | stats count by tool_name
```

## Related Documentation

- [CLI Reference](./cli.md) - Command-line interface documentation
- [Environment Variables](./environment-variables.md) - Configuration via environment variables
- [Built-in Tools](./built-in-tools.md) - List of available built-in tools
