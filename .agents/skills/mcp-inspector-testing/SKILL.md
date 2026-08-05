---
name: mcp-inspector-testing
description: Validate Wassette changes end-to-end by running the MCP server with just and driving it with the MCP Inspector CLI over Streamable HTTP — listing tools, resources, and prompts, and calling tools. Use before committing server changes or when debugging tool exposure and behavior.
allowed-tools: Bash, Read, Write, Edit, Glob, Grep
---

# mcp-inspector-testing skill

Test server-facing Wassette changes with the MCP Inspector before committing.
This confirms the server starts, tools are exposed over MCP, tool calls behave
as expected, and no regressions slipped in.

## Workflow

1. Build your changes: `just build`.
2. Start the server (Terminal 1): `just run`, or an example such as
   `just run-fetch-rs`. The server listens at `127.0.0.1:9001/mcp` over
   Streamable HTTP.
3. Drive it with the Inspector (Terminal 2) — always run `tools/list` first.
4. Capture Inspector output when it helps demonstrate or debug the change.

## Running the server

```bash
just run                       # Default server
just run RUST_LOG='debug'      # Verbose logs
just run-filesystem            # Example: filesystem component
just run-fetch-rs              # Example: fetch component
just run-memory                # Example: memory component
just run-get-weather           # Example: weather (needs OPENWEATHER_API_KEY)
```

## Inspecting with the MCP Inspector

```bash
BASE=http://127.0.0.1:9001/mcp

# List capabilities (run tools/list first)
npx @modelcontextprotocol/inspector --cli $BASE --transport http --method tools/list
npx @modelcontextprotocol/inspector --cli $BASE --transport http --method resources/list
npx @modelcontextprotocol/inspector --cli $BASE --transport http --method prompts/list

# Call a tool
npx @modelcontextprotocol/inspector --cli $BASE --transport http \
  --method tools/call --tool-name mytool --tool-arg key=value

# Multiple args, or JSON for complex parameters
npx @modelcontextprotocol/inspector --cli $BASE --transport http \
  --method tools/call --tool-name mytool --tool-arg key1=v1 --tool-arg key2=v2
npx @modelcontextprotocol/inspector --cli $BASE --transport http \
  --method tools/call --tool-name mytool --tool-arg 'options={"format":"json","max_tokens":100}'

# Custom headers (e.g. auth testing)
npx @modelcontextprotocol/inspector --cli $BASE --transport http \
  --method tools/list --header "X-API-Key: your-api-key"
```

## Capturing output

```bash
npx @modelcontextprotocol/inspector --cli $BASE --transport http \
  --method tools/list > inspector-output.txt
```

Include captured output in review notes or issue comments when it helps
demonstrate a fix.

## Troubleshooting

1. Confirm `just run` started without errors.
2. Run `tools/list` to see what is actually exposed.
3. Re-run the server with `RUST_LOG=debug` for detailed logs.
4. Start with simple tool calls, then add argument complexity.
5. Compare against a known-good example such as `fetch-rs`.
