# arXiv Research Component

A WebAssembly component that provides arXiv research capabilities, including searching for papers, downloading PDFs, and reading paper metadata.

## Features

- **Search Papers**: Search arXiv database with query, date filters, and category filters
- **Download Papers**: Download PDF files of research papers
- **Read Paper Metadata**: Fetch detailed metadata and abstracts for specific papers

## Building

```bash
cargo build --target wasm32-wasip2 --release
```

Or use the Justfile:

```bash
just build release
```

The compiled `.wasm` file will be in `target/wasm32-wasip2/release/arxiv_rs.wasm`.

## Usage

This component exports three functions:

### 1. search-papers

Search for papers on arXiv with optional filters.

```rust
search-papers(
    query: string,          // Search query (e.g., "machine learning", "cat:cs.AI")
    max-results: u32,       // Maximum number of results
    date-from: string,      // Optional date filter in YYYYMMDD format (empty to skip)
    categories: string      // Optional comma-separated categories (empty to skip)
) -> result<string, string>
```

**Query Syntax Examples:**
- `"machine learning"` - General search
- `"cat:cs.AI"` - Search in AI category
- `"ti:neural networks"` - Search in titles
- `"au:LeCun"` - Search by author
- `"all:deep learning"` - Search all fields

**Common Categories:**
- `cs.AI` - Artificial Intelligence
- `cs.LG` - Machine Learning
- `cs.CV` - Computer Vision
- `cs.CL` - Computation and Language
- `physics.data-an` - Data Analysis

### 2. download-paper

Download a paper PDF from arXiv.

```rust
download-paper(id: string) -> result<list<u8>, string>
```

**Example:**
```rust
download-paper("2301.00001")  // Downloads PDF for paper 2301.00001
```

### 3. read-paper

Read paper metadata and abstract from arXiv.

```rust
read-paper(id: string) -> result<string, string>
```

**Example:**
```rust
read-paper("2301.00001")  // Returns formatted metadata and abstract
```

## Testing

This component uses the WebAssembly Component Model and exports functions according to the WIT specification. Since this is a WebAssembly Component (not a core module), it cannot be tested directly with `wasmtime run --invoke` as that command only works with core WebAssembly modules.

To test this component, use one of the following methods:

### Testing with Wassette CLI

The recommended way to test the component is through Wassette:

```bash
# Build the component
just build release

# Load the component with Wassette
wassette component load file://$(pwd)/target/wasm32-wasip2/release/arxiv_rs.wasm

# Grant network permissions
wassette permission grant network arxiv_rs "http://export.arxiv.org/"
wassette permission grant network arxiv_rs "http://arxiv.org/"

# The component functions are now available as MCP tools
```

### Component Validation

You can verify the component structure and exports using `wasm-tools`:

```bash
# Validate the component
wasm-tools validate target/wasm32-wasip2/release/arxiv_rs.wasm

# View component interface
wasm-tools component wit target/wasm32-wasip2/release/arxiv_rs.wasm
```

### Testing via MCP Server

Start the MCP server and interact with it via the MCP Inspector:

```bash
# Start wassette MCP server (from project root)
cargo run --release -- serve mcp sse

# In another terminal, use MCP inspector
npx @modelcontextprotocol/inspector --cli http://127.0.0.1:9001/sse
```

## Examples

### Search for AI papers from 2024

```rust
search-papers("artificial intelligence", 10, "20240101", "cs.AI")
```

### Search for machine learning papers by specific author

```rust
search-papers("au:Hinton machine learning", 5, "", "")
```

### Download a specific paper

```rust
download-paper("2301.00001")
```

### Read metadata for a paper

```rust
read-paper("2301.00001")
```

## API Reference

This component uses the [arXiv API](https://arxiv.org/help/api) to access research papers. The API is free and does not require authentication.

## License

This component is licensed under the MIT License.
