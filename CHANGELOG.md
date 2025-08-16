# Changelog

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Comprehensive Go development guide for authoring Wasm components ([#163](https://github.com/microsoft/wassette/pull/163))
- Comprehensive documentation for authoring Wasm Components with Python ([#161](https://github.com/microsoft/wassette/pull/161))
- Detailed documentation for authoring WebAssembly Components from JavaScript/TypeScript ([#159](https://github.com/microsoft/wassette/pull/159))
- Comprehensive documentation for authoring Wasm Components from Rust ([#157](https://github.com/microsoft/wassette/pull/157))
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
- Kubernetes-style resource limits in policy specification with `resources.limits` section supporting CPU ("500m", "1") and memory ("512Mi", "1Gi") formats ([#166](https://github.com/microsoft/wassette/pull/166))

### Changed
- **BREAKING CHANGE**: Renamed `--http` flag to `--sse` for clarity, distinguishing SSE transport from streamable HTTP transport ([#100](https://github.com/microsoft/wassette/pull/100))
- **BREAKING CHANGE**: Component registry struct renamed for consistency ([#112](https://github.com/microsoft/wassette/pull/112))
- Pre-instantiated components now used for faster startup time and better performance under load ([#124](https://github.com/microsoft/wassette/pull/124))
- Refactored lib.rs into smaller, more manageable modules for better code organization ([#112](https://github.com/microsoft/wassette/pull/112))
- Optimized examples.yml workflow triggers to only run on example changes ([#102](https://github.com/microsoft/wassette/pull/102))
- Optimized resource limit parsing with caching using `OnceLock` to avoid repeated string parsing ([#166](https://github.com/microsoft/wassette/pull/166))
- Removed policy configuration section from JavaScript/TypeScript WebAssembly Component authoring guide as it's not related to component authoring ([#159](https://github.com/microsoft/wassette/pull/159))

### Fixed
- Component loading across different filesystems (EXDEV error handling) ([#109](https://github.com/microsoft/wassette/pull/109))
- Component names in README files for consistency ([#115](https://github.com/microsoft/wassette/pull/115))
- Installation instructions for Linux and Windows in README ([#120](https://github.com/microsoft/wassette/pull/120))

## [v0.2.0] - 2025-08-05

### Added
- Enhanced component lifecycle management with improved file cleanup
- Comprehensive documentation and release process improvements
- Integration tests for component notifications

### Changed
- Refactored component lifecycle management with better file cleanup
- Enhanced developer experience improvements

### Fixed
- Logging to stderr for stdio transport
- Various typos and documentation corrections

## [v0.1.0] - 2025-08-05

Initial release of Wassette - A security-oriented runtime that runs WebAssembly Components via MCP (Model Context Protocol).

### Added
- Core MCP server implementation for running WebAssembly components
- Support for SSE and stdio transports
- Component lifecycle management (load, unload, call)
- Policy-based security system for component permissions
- Built-in examples and CLI interface
- Installation support and documentation

[Unreleased]: https://github.com/microsoft/wassette/compare/v0.2.0...HEAD
[v0.2.0]: https://github.com/microsoft/wassette/compare/v0.1.0...v0.2.0
[v0.1.0]: https://github.com/microsoft/wassette/releases/tag/v0.1.0