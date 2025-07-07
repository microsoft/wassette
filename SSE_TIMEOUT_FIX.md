# SSE Connection Timeout Fix

This document explains the fix for the SSE connection timeout issue where connections were terminated after 10-15 minutes.

## Problem

When using HTTP transport mode (`--http` flag), VSCode would display the error "Connection state: Error Error reading SSE stream: TypeError: terminated" after approximately 10-15 minutes of inactivity.

## Solution

The fix implements a keep-alive mechanism that:

1. **Monitors connection activity** - Tracks when the last MCP request was made
2. **Sends periodic pings** - If no activity for 8+ minutes, sends a keep-alive notification every 5 minutes
3. **Only affects HTTP transport** - stdio transport is unaffected
4. **Minimal overhead** - Only activates when needed, no constant background activity

## Implementation Details

- **KeepAliveServer wrapper**: Wraps the existing MCP server to add activity tracking
- **Activity monitoring**: Updates timestamp on each MCP method call
- **Keep-alive notifications**: Sends `notifications/ping` messages to maintain connection
- **Background task**: Runs asynchronously without blocking normal operations

## Usage

The fix is automatically applied when using HTTP transport:

```bash
# Keep-alive is automatically enabled for HTTP transport
weld-mcp-server serve --http

# stdio transport works as before (no keep-alive needed)
weld-mcp-server serve --stdio
```

## Verification

To verify the fix works:

1. Start the server with HTTP transport: `weld-mcp-server serve --http`
2. Connect a client and let it idle for 15+ minutes
3. The connection should remain active instead of timing out

The server will log keep-alive pings at debug level:
```
DEBUG weld_mcp_server::sse_server_wrapper: Sent keep-alive ping to maintain SSE connection
```

## Technical Notes

- Keep-alive interval: 5 minutes (when inactive for 8+ minutes)
- Notification method: `notifications/ping` (non-standard but harmless)
- Activity tracking: Uses atomic timestamps for thread safety
- Zero overhead when connection is active