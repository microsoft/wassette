# Installation

Wassette can be installed on multiple platforms using various package managers and methods.

## Binary Releases

The easiest way to install Wassette is to download a pre-built binary from the [GitHub releases page](https://github.com/microsoft/wassette/releases).

## Package Managers

### Homebrew (macOS and Linux)

For detailed Homebrew installation instructions, see the [Homebrew guide](./homebrew.md).

### Nix

For Nix users, see the [Nix installation guide](./nix.md).

### Winget (Windows)

For Windows users with Winget, see the [Winget installation guide](./winget.md).

## Building from Source

### Prerequisites

- Rust toolchain (latest stable)
- Git

### Build Steps

```bash
# Clone the repository
git clone https://github.com/microsoft/wassette.git
cd wassette

# Build the project
cargo build --release

# The binary will be available at target/release/wassette
```

### Using Just

If you have [Just](https://github.com/casey/just) installed, you can use the provided Justfile:

```bash
# Build in debug mode
just build

# Build in release mode
just build release

# Run tests
just test
```

## Verification

After installation, verify that Wassette is working correctly:

```bash
wassette --version
```

## Next Steps

Once Wassette is installed, check out the [MCP Clients guide](./mcp-clients.md) to learn how to connect different MCP clients to Wassette.