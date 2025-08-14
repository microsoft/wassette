# Changelog

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Support for Streamable HTTP transport in addition to existing SSE transport ([#100](https://github.com/microsoft/wassette/pull/100))
- Revoke commands and reset permission functionality with simplified storage revocation ([#87](https://github.com/microsoft/wassette/pull/87))
- Enhanced `--version` command to display detailed build information with cleaner clap integration ([#119](https://github.com/microsoft/wassette/pull/119))
- Parallel component loading for improved performance ([#123](https://github.com/microsoft/wassette/pull/123))
- Configuration file management for CLI settings ([#94](https://github.com/microsoft/wassette/pull/94))
- LTO (Link Time Optimization) to release builds for 27% size improvement ([#106](https://github.com/microsoft/wassette/pull/106))
- EXDEV-safe fallback for component loading across different filesystems ([#109](https://github.com/microsoft/wassette/pull/109))
- Nix flake support for reproducible builds ([#105](https://github.com/microsoft/wassette/pull/105))
- WinGet support for Windows installation ([#108](https://github.com/microsoft/wassette/pull/108))
- CI improvements including caching for Rust builds ([#98](https://github.com/microsoft/wassette/pull/98))
- Spell check, link checker, and unused dependency checker to CI workflow ([#116](https://github.com/microsoft/wassette/pull/116))

### Changed
- **BREAKING CHANGE**: Renamed `--http` flag to `--sse` for clarity, distinguishing SSE transport from streamable HTTP transport ([#100](https://github.com/microsoft/wassette/pull/100))
- **BREAKING CHANGE**: Component registry struct renamed for consistency ([#112](https://github.com/microsoft/wassette/pull/112))
- Pre-instantiated components now used for faster startup time and better performance under load ([#124](https://github.com/microsoft/wassette/pull/124))
- Refactored lib.rs into smaller, more manageable modules for better code organization ([#112](https://github.com/microsoft/wassette/pull/112))
- Optimized examples.yml workflow triggers to only run on example changes ([#102](https://github.com/microsoft/wassette/pull/102))

### Fixed
- Component loading across different filesystems (EXDEV error handling) ([#109](https://github.com/microsoft/wassette/pull/109))
- Linting and test failures related to unused imports and config field references ([#100](https://github.com/microsoft/wassette/pull/100))
- Component names in README files for consistency ([#115](https://github.com/microsoft/wassette/pull/115))
- Installation instructions for Linux and Windows in README ([#120](https://github.com/microsoft/wassette/pull/120))
- Component load instructions in README for filesystem and gomodule examples ([#97](https://github.com/microsoft/wassette/pull/97))

### Removed
- Unused dependencies from Cargo.toml ([#116](https://github.com/microsoft/wassette/pull/116))

## [v0.2.0] - 2025-08-05

### Added
- Enhanced unload-component API to delete files on disk (symmetric to load-component)
- Improved logging with structured fields for component operations
- Missing documentation warnings and comprehensive documentation
- Comprehensive release process documentation
- Integration tests for component notifications
- Installation instruction links
- Enhanced code documentation coverage
- Rust coding instructions for GitHub Copilot

### Changed
- Refactored component lifecycle management with better file cleanup
- Simplified policy cleanup and metadata path retrieval
- Enhanced developer experience with copyright headers and build scripts
- Moved design documentation to proper location

### Fixed
- Logging to stderr for stdio transport
- Removed optionality of server_peer in component handling functions
- Corrected typos and added ARM64 links for Linux and Windows

## [v0.1.0] - 2025-07-10

Initial release of Wassette - A security-oriented runtime that runs WebAssembly Components via MCP (Model Context Protocol).

### Added
- Core MCP server implementation for running WebAssembly components
- Support for SSE (Server-Sent Events) transport
- Support for stdio transport
- Component lifecycle management (load, unload, call)
- Policy-based security system for component permissions
- Permission management for network, environment, and storage access
- Built-in examples including:
  - HTTP API client for fetching web content (fetch-rs)
  - File system operations (filesystem-rs)
  - Weather API client (get-weather-js)
  - Go module information tool (gomodule-go)
  - Time server component (time-server-js)
  - Python evaluation component (eval-py)
- CLI interface with serve command
- Integration with Visual Studio Code MCP clients
- Installation support via Homebrew (macOS)
- Comprehensive documentation and setup guides

[Unreleased]: https://github.com/microsoft/wassette/compare/v0.2.0...HEAD
[v0.2.0]: https://github.com/microsoft/wassette/compare/v0.1.0...v0.2.0
[v0.1.0]: https://github.com/microsoft/wassette/releases/tag/v0.1.0