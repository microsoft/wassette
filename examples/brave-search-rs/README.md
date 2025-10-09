# Brave Search Component

A WebAssembly component that performs web searches using the Brave Search API.

## Building

```bash
cargo component build --release
```

The compiled `.wasm` file will be in `target/wasm32-wasip1/release/brave_search_rs.wasm`.

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
  target/wasm32-wasip1/release/brave_search_rs.wasm
```

### Testing with Wassette CLI

Load and test the component:

```bash
# Load the component
wassette component load file://$(pwd)/target/wasm32-wasip1/release/brave_search_rs.wasm

# Grant network permission to access Brave Search API
wassette permission grant network brave_search_rs "https://api.search.brave.com/"

# Grant environment variable access for the API key
wassette permission grant environment-variable brave_search_rs BRAVE_SEARCH_API_KEY

# Check component is loaded
wassette component list
```

### Testing via MCP Server

Start the MCP server and interact with it via the MCP Inspector:

```bash
# Set the API key in your environment
export BRAVE_SEARCH_API_KEY=your_api_key_here

# Start wassette MCP server (from project root)
cargo run --release -- serve mcp sse

# In another terminal, use MCP inspector to call the tool
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse
```

The component will return formatted markdown with web and news results.

## API Key

Get your Brave Search API key from: https://brave.com/search/api/

Set it as an environment variable:
```bash
export BRAVE_SEARCH_API_KEY=your_api_key_here
```
