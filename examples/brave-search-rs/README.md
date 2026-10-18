# Brave Search Component

A WebAssembly component that performs web searches using the Brave Search API.

## Building

```bash
cargo component build --release
```

The compiled `.wasm` file will be in `target/wasm32-wasip2/release/brave_search_rs.wasm`.

## Usage

This component exports a `search` function that takes a query string:

```rust
search(query: string) -> result<string, string>
```

The component reads the `BRAVE_SEARCH_API_KEY` environment variable for authentication.

### Testing with Wasmtime

Test directly with wasmtime CLI:

```bash
# Set the API key (get yours from https://brave.com/search/api/)
export BRAVE_SEARCH_API_KEY=your_api_key_here

# Run with wasmtime (enable HTTP and network access)
wasmtime run -S http=y -S inherit-network=y \
  --env BRAVE_SEARCH_API_KEY \
  --invoke 'search("rust programming")' \
  target/wasm32-wasip2/release/brave_search_rs.wasm
```

### Testing with Wassette CLI

From the repository root, make the API key available, then explicitly grant
this component access to the key and the Brave Search endpoint:

```bash
just build-examples release
STORE="$(mktemp -d)"
export BRAVE_SEARCH_API_KEY="<developer-key>"

wassette component load \
  "file://$PWD/bin/brave-search-rs.wasm" \
  --component-dir "$STORE"

wassette permission grant network \
  brave-search-rs api.search.brave.com \
  --component-dir "$STORE"

wassette permission grant environment-variable \
  brave-search-rs BRAVE_SEARCH_API_KEY \
  --component-dir "$STORE"

wassette run --component-dir "$STORE" --disable-builtin-tools
```

Exporting `BRAVE_SEARCH_API_KEY` makes the value available to Wassette; the
environment permission is what authorizes this component to read it. The
checked-in [`policy.yaml`](policy.yaml) documents both required grants but is
not automatically attached by a local file load or included in the currently
published OCI example. See the [examples permission workflow](../README.md).

### Testing via MCP Server

Use the same component store from the workflow above. Replace the final
`wassette run` command with the HTTP server, then connect MCP Inspector:

```bash
wassette serve --streamable-http \
  --component-dir "$STORE" \
  --disable-builtin-tools

# In another terminal, use MCP Inspector.
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/mcp --transport http
```

The component will return formatted markdown with web and news results.

## API Key

Get your Brave Search API key from: https://brave.com/search/api/

Set it as an environment variable:
```bash
export BRAVE_SEARCH_API_KEY=your_api_key_here
```

See the [Environment Variables reference](../../docs/reference/environment-variables.md) for alternative methods including config files and Docker.
