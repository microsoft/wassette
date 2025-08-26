# Changelog

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Updated documentation to clarify Wassette as a runtime rather than a platform, with improved wording for creating WebAssembly components that can be used as Tools for AI Agents with Wassette ([#TBD](https://github.com/microsoft/wassette/pull/TBD))
- Disabled the security audit job from GitHub Actions workflow to reduce CI noise ([#TBD](https://github.com/microsoft/wassette/pull/TBD))

### Fixed

- Fixed test coverage CI failing on PRs from forked repositories by switching from PR comments to job summaries ([#237](https://github.com/microsoft/wassette/pull/237))
- Fixed dependabot auto-merge workflow failing with "workflows permission" error by adding `workflows: write` permission ([#TBD](https://github.com/microsoft/wassette/pull/TBD))
- Fixed inconsistent spelling of "wasette" to "wassette" in configuration paths and documentation comments ([#TBD](https://github.com/microsoft/wassette/pull/TBD))
- Fixed broken links in README.md pointing to documentation files in wrong directory paths ([#TBD](https://github.com/microsoft/wassette/pull/TBD))
- Add cargo audit configuration to acknowledge unmaintained `paste` dependency warning ([#169](https://github.com/microsoft/wassette/pull/169))

### Added

- GitHub Actions workflow to automatically build and deploy mdBook documentation to GitHub Pages ([#196](https://github.com/microsoft/wassette/pull/196))
- Dependabot automerge workflow for automated dependency updates when CI passes ([#TBD](https://github.com/microsoft/wassette/pull/TBD))
- Documentation for built-in tools in README, listing all 11 available tools with descriptions for better discoverability ([#TBD](https://github.com/microsoft/wassette/pull/TBD))
- Dependabot automerge workflow for automated dependency updates when CI passes
- **Major CLI UX Enhancement**: Expanded Wassette from a simple server launcher to a comprehensive CLI tool for managing WebAssembly components and permissions directly from the command line
- **Component Management Commands**:
  - `wassette component load <path>` - Load WebAssembly components from file paths or OCI registries
  - `wassette component unload <id>` - Unload components by ID
  - `wassette component list` - List all loaded components with metadata
- **Policy Management Commands**:
  - `wassette policy get <component_id>` - Retrieve policy information for components
- **Permission Management Commands**:
  - `wassette permission grant storage <component_id> <uri> --access read,write` - Grant storage permissions
  - `wassette permission grant network <component_id> <host>` - Grant network permissions  
  - `wassette permission grant environment-variable <component_id> <key>` - Grant environment variable permissions
  - `wassette permission grant memory <component_id> <limit>` - Grant memory resource permissions
  - `wassette permission revoke storage <component_id> <uri>` - Revoke storage permissions
  - `wassette permission revoke network <component_id> <host>` - Revoke network permissions
  - `wassette permission revoke environment-variable <component_id> <key>` - Revoke environment variable permissions
  - `wassette permission revoke resource <component_id> --memory` - Revoke memory resource permissions
  - `wassette permission reset <component_id>` - Reset all permissions for a component
- **Output Formatting**: Added support for multiple output formats (JSON, YAML, table) using `--output-format` flag
- **CLI Documentation**: Comprehensive CLI reference documentation in `docs/cli.md`
- Support for MCP Tool structured output as defined in the MCP specification ([#181](https://github.com/microsoft/wassette/pull/181))
- End-to-end integration test for MCP structured output feature verification ([#181](https://github.com/microsoft/wassette/pull/181))

### Changed  

- **BREAKING CHANGE**: Upgraded rmcp dependency from v0.2 to v0.5.0 to enable native structured output support ([#181](https://github.com/microsoft/wassette/pull/181))
- Copyright header instructions to Rust development guidelines ([#TBD](https://github.com/microsoft/wassette/pull/TBD))
- Enhanced environment variable CLI experience with `--env` and `--env-file` options for better configuration management
- Memory resource granting functionality for components with k8s-style memory limits ([#171](https://github.com/microsoft/wassette/pull/171))
- Memory resource revocation functionality allowing removal of memory limits from component policies ([#171](https://github.com/microsoft/wassette/pull/171))
- Comprehensive Go development guide for authoring Wasm components ([#163](https://github.com/microsoft/wassette/pull/163))
- Comprehensive documentation for authoring Wasm Components with Python ([#161](https://github.com/microsoft/wassette/pull/161))
- Detailed documentation for authoring WebAssembly Components from JavaScript/TypeScript ([#159](https://github.com/microsoft/wassette/pull/159))
- Comprehensive documentation for authoring Wasm Components from Rust ([#157](https://github.com/microsoft/wassette/pull/157))
- Support for Streamable HTTP transport in addition to existing SSE transport ([#100](https://github.com/microsoft/wassette/pull/100))

### Fixed

- Fixed permission parsing to support "environment-variable" permission type alias for environment permissions
- Fixed storage permission revocation to work with URI-only specification (removes all access types for the given URI)
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

### Fixed
- Memory resource limiter implementation by properly applying limits to Wasmtime store using the correct `limiter()` API ([#171](https://github.com/microsoft/wassette/pull/171))

### Changed
- CLI now supports both server mode (`wassette serve`) and direct management mode for component operations
- Component load/unload operations can now work independently without requiring a running MCP server
- Enhanced help text and command structure with logical grouping of related functionality
- **BREAKING CHANGE**: Renamed `--http` flag to `--sse` for clarity, distinguishing SSE transport from streamable HTTP transport ([#100](https://github.com/microsoft/wassette/pull/100))
- **BREAKING CHANGE**: Component registry struct renamed for consistency ([#112](https://github.com/microsoft/wassette/pull/112))
- Pre-instantiated components now used for faster startup time and better performance under load ([#124](https://github.com/microsoft/wassette/pull/124))
- Refactored lib.rs into smaller, more manageable modules for better code organization ([#112](https://github.com/microsoft/wassette/pull/112))
- Optimized examples.yml workflow triggers to only run on example changes ([#102](https://github.com/microsoft/wassette/pull/102))
- Optimized resource limit parsing with caching using `OnceLock` to avoid repeated string parsing ([#166](https://github.com/microsoft/wassette/pull/166))
- Removed policy configuration section from JavaScript/TypeScript WebAssembly Component authoring guide as it's not related to component authoring ([#159](https://github.com/microsoft/wassette/pull/159))

### Fixed

- Add cargo audit configuration to acknowledge unmaintained `paste` dependency warning ([#169](https://github.com/microsoft/wassette/pull/169))
- Memory resource limits now properly applied to component execution using Wasmtime's ResourceLimiter ([#171](https://github.com/microsoft/wassette/pull/171))
- Component loading across different filesystems (EXDEV error handling) ([#109](https://github.com/microsoft/wassette/pull/109))
- Component names in README files for consistency ([#115](https://github.com/microsoft/wassette/pull/115))
- Installation instructions for Linux and Windows in README ([#120](https://github.com/microsoft/wassette/pull/120))

### Technical Details
- Zero code duplication by reusing existing MCP tool handler functions
- CLI-specific wrapper functions (`handle_load_component_cli`, `handle_unload_component_cli`) that work without MCP server peer notifications
- Maintains full backward compatibility with existing `serve` command
- Proper error handling with clear error messages for non-existent components
- Follows common CLI patterns and conventions for intuitive user experience

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
