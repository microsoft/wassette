# Changelog

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added `just install` command for convenient local installation to ~/.local/bin
- Configurable bind address for HTTP-based transports (SSE and StreamableHttp) via CLI flag `--bind-address`, environment variable `WASSETTE_BIND_ADDRESS`, or configuration file field `bind_address`. Default remains `127.0.0.1:9001` for backward compatibility
- Cookbook tutorial for publishing Wasm components to OCI registries (GHCR) using `wkg` CLI tool and GitHub Actions, including local development workflow, automated CI/CD publishing, signing with Cosign, version management strategies, and troubleshooting guide
- Prepared policy crate for publication to crates.io with comprehensive README.md and complete package metadata (description, repository, documentation, homepage, keywords, categories, authors). The policy crate is now ready to be published as a standalone library for other projects like [policy-mcp](https://github.com/microsoft/policy-mcp)
- Automated CHANGELOG synchronization with release pipeline: Release workflow extracts changelog content for release notes and automatically updates CHANGELOG.md post-release. Implemented using Python scripts with unit tests. The update-changelog job now checks that the release job succeeded before running.
- Migration guide documentation for converting JavaScript-based MCP servers to Wassette WebAssembly components in the cookbook section, with step-by-step instructions, code examples, and a complete weather service migration walkthrough
- Configuration Files reference documentation covering Wassette server configuration files (config.toml) and build/toolchain configuration files (Cargo.toml, rust-toolchain.toml, rustfmt.toml, etc.) with detailed schemas, examples, and best practices
- Concepts documentation page explaining MCP fundamentals (server vs client, tools, prompts, resources), WebAssembly Component Model (components, WIT, bindings), how Wassette translates components to MCP tools, and the policy/capability model
- Developer documentation section with comprehensive "Getting Started" guide covering prerequisites, building, testing, code formatting, development workflow, CI/CD, and project structure for contributors
- Documentation reference for commonly used HTTP domains in network permissions, providing a comprehensive list of frequently needed domains organized by category (package registries, version control systems, cloud providers, container registries, AI/ML APIs, CDNs, documentation sites, and CI/CD services) to help users make informed decisions when granting network permissions ([#368](https://github.com/microsoft/wassette/pull/368))
- Added `--disable-builtin-tools` flag to the `serve` command that allows disabling all built-in tools (load-component, unload-component, list-components, get-policy, grant/revoke permissions, search-components, reset-permission). When enabled, only loaded component tools will be available through the MCP server
- Comprehensive Docker documentation and Dockerfile for running Wassette in containers with enhanced security isolation, including examples for mounting components, secrets, configuration files, and production deployment patterns with Docker Compose
- `rust-toolchain.toml` file specifying Rust 1.90 as the stable toolchain version, ensuring consistent Rust version across development environments and CI/CD pipelines
- AI agent development guides (`AGENTS.md` and `Claude.md`) that consolidate development guidelines from `.github/instructions/` into accessible documentation for AI agents working on the project
- Comprehensive installation guide page consolidating all installation methods (one-liner script, Homebrew, Nix, WinGet) organized by platform (Linux, macOS, Windows) with verification steps and troubleshooting sections
- Cookbook section in documentation with language-specific guides for building Wasm components in JavaScript/TypeScript, Python, Rust, and Go ([#328](https://github.com/microsoft/wassette/pull/328))
- Multi-version documentation support with version dropdown, hosting at `/wassette/latest/` (main) and `/wassette/vX.Y/` (tags)
- Automated release preparation and package manifest update workflows to eliminate manual version bump PRs ([#320](https://github.com/microsoft/wassette/pull/320))
- User-focused permissions documentation under new "Using Wassette" section, providing practical how-to guides for managing permissions
- Added `$schema` field to all policy YAML files referencing the policy-mcp schema for better IDE support and validation
- Documentation for GitHub Agentic Workflows explaining how the repository uses @githubnext/gh-aw for automated issue triage and research tasks
- Comprehensive documentation in RELEASE.md for releasing example component images to GHCR, including automatic publishing workflow, manual release process, and instructions for adding new examples
- GitHub Actions workflow `.github/workflows/copilot-setup-steps.yml` that provides reusable setup steps for GitHub Copilot coding agents to prepare a complete development environment with Rust, just, protobuf, wasm-tools, and other essential tools
- Added `rules/agent.md` instruction file for AI agents emphasizing use of `grant-xxx-permission` tools instead of manually editing policy files, with installation instructions in the installation guide
- Comprehensive documentation on wit-docs-inject usage for embedding WIT documentation into WASM components and translating it to AI agent tool descriptions
- Agentic workflow for automatic CHANGELOG PR link addition: When PRs modify CHANGELOG.md, the workflow automatically adds PR links to new entries in the Unreleased section, ensuring consistent formatting and making it easier to track changes back to their source PRs
- Release branch strategy to prevent development blockages: Release pipeline now creates and preserves dedicated release branches (e.g., `release/vX.Y.Z`) for the entire release process, ensuring that ongoing development on main is not blocked by release activities

### Changed

- Improved CLI help text for transport flags: changed from "Enable XXX transport" to "Serving with XXX transport" for better clarity on what the flags do
- Updated wasmtime dependencies from version 36 to 38.0.2 (wasmtime, wasmtime-wasi, wasmtime-wasi-http, wasmtime-wasi-config)
- Refactored duplicated tool name string constants in `src/main.rs` by introducing centralized `const` definitions, eliminating duplication between `TryFrom` and `AsRef` implementations
- Updated publish examples workflow to include new examples: brave-search-rs, context7-rs, and get-open-meteo-weather-js
- Streamlined README.md by removing detailed sections on "Built-in Tools", "Building WebAssembly Components", "Community Components", and "Documentation" in favor of linking to comprehensive documentation pages
- Removed duplicate built-in tools listing from `docs/design/permission-system.md` and replaced with reference link to `docs/reference/built-in-tools.md` ([#379](https://github.com/microsoft/wassette/pull/379))
- Removed duplicate built-in tools listing from `docs/faq.md` and replaced with reference link to `docs/reference/built-in-tools.md` ([#379](https://github.com/microsoft/wassette/pull/379))
- Added cross-references throughout documentation (`docs/concepts.md`, `docs/overview.md`, `docs/faq.md`) to link to detailed reference documentation for permissions and built-in tools, reducing content duplication while improving navigation ([#379](https://github.com/microsoft/wassette/pull/379))
- Simplified README "Installation" section to show only quick start and link to full installation guide
- Updated "Using Wassette" section in README to remove installation instructions and focus on component loading workflow
- Created new documentation pages: `docs/reference/built-in-tools.md` for tool reference and `docs/reference/community-components.md` for community projects
- Removed redundant "Docker Deployment" section from README.md; users are directed to the comprehensive Docker deployment guide via the installation methods list
- Moved permissions documentation from "Using Wassette" section to "Reference" section, placing it after CLI reference for better organization and discoverability
- Reorganized documentation structure by moving CLI reference to a new `reference` section in the mdBook for better organization
- **BREAKING CHANGE**: Release workflow no longer pushes CHANGELOG updates directly to main; instead creates a PR from the release branch for review and merging, preventing development blockages during release process
- Updated README.md to reference the new dedicated installation guide for complete installation instructions
- Removed separate homebrew.md, nix.md, and winget.md pages to eliminate duplication; all installation content is now consolidated in installation.md
- Added tabbed interface for installation instructions organized by platform (Linux, macOS, Windows, Nix) using mdbook-tabs preprocessor
- Updated README.md, docs/faq.md, and RELEASE.md to include all 9 examples in the examples directory: brave-search-rs, context7-rs, eval-py, fetch-rs, filesystem-rs, get-open-meteo-weather-js, get-weather-js, gomodule-go, and time-server-js
- Configure `prepare-release` workflow to use `RELEASE_TOKEN` secret for creating pull requests, allowing custom PAT authentication

### Fixed

- Fixed post-release workflows not triggering properly: Release workflow now uses `RELEASE_TOKEN` instead of `GITHUB_TOKEN` to allow triggering downstream workflows, and Publish Examples workflow corrected event type from `publish` to `published`
- `wassette secret set` now returns a clear error message when the component ID is not found, preventing silent failures and providing better user feedback
- Fixed invalid `workflows` permission in dependabot-automerge workflow file that caused GitHub Actions validation error
- Fixed Mermaid sequence diagram rendering in documentation by adding mdbook-mermaid preprocessor configuration
- Copyright check script now skips auto-generated `bindings.rs` files containing wit-bindgen markers, preventing incorrect license header additions to generated code while still checking custom bindings.rs files
- Made dependabot automerge workflow non-blocking by adding `continue-on-error: true` to the auto-merge step, preventing workflow failures from blocking PRs when automerge cannot be enabled

## [v0.3.0] - 2025-10-03

### Added

- **Component Discovery**: Added `search-components` tool that lists all known components available for loading from the component registry, making it easier for users to discover and load new WebAssembly tools ([#236](https://github.com/microsoft/wassette/pull/236))
- Simple per-component secret management system with CLI commands `wassette secret list|set|delete <component-id>` ([#199](https://github.com/microsoft/wassette/pull/199))
  - Stores secrets in OS-appropriate directories with proper permissions (0700/user-only)
  - YAML format with flat String->String mappings for easy editing and auditing
  - Lazy loading with mtime-based cache invalidation for performance
  - Integrates with environment variable precedence system (policy > secrets > inherited env)
  - No server restart required, persists across runs
  - Automatic component ID sanitization for safe filenames
- GitHub Actions workflow to automatically build and deploy mdBook documentation to GitHub Pages ([#196](https://github.com/microsoft/wassette/pull/196))
- Dependabot automerge workflow for automated dependency updates when CI passes
- Documentation for built-in tools in README, listing all 12 available tools with descriptions for better discoverability
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
- Zero code duplication by reusing existing MCP tool handler functions
- CLI-specific wrapper functions (`handle_load_component_cli`, `handle_unload_component_cli`) that work without MCP server peer notifications

### Changed

- Updated Wasmtime dependencies from version 33 to 36 ([#265](https://github.com/microsoft/wassette/pull/265))
- Updated documentation to clarify Wassette as a runtime rather than a platform, with improved wording for creating WebAssembly components that can be used as Tools for AI Agents with Wassette
- Disabled the security audit job from GitHub Actions workflow to reduce CI noise
- **BREAKING CHANGE**: Upgraded rmcp dependency from v0.2 to v0.5.0 to enable native structured output support ([#181](https://github.com/microsoft/wassette/pull/181))
- Copyright header instructions to Rust development guidelines
- Enhanced environment variable CLI experience with `--env` and `--env-file` options for better configuration management
- Memory resource granting functionality for components with k8s-style memory limits ([#171](https://github.com/microsoft/wassette/pull/171))
- Memory resource revocation functionality allowing removal of memory limits from component policies ([#171](https://github.com/microsoft/wassette/pull/171))
- Comprehensive Go development guide for authoring Wasm components ([#163](https://github.com/microsoft/wassette/pull/163))
- Comprehensive documentation for authoring Wasm Components with Python ([#161](https://github.com/microsoft/wassette/pull/161))
- Detailed documentation for authoring WebAssembly Components from JavaScript/TypeScript ([#159](https://github.com/microsoft/wassette/pull/159))
- Comprehensive documentation for authoring Wasm Components from Rust ([#157](https://github.com/microsoft/wassette/pull/157))
- Support for Streamable HTTP transport in addition to existing SSE transport ([#100](https://github.com/microsoft/wassette/pull/100))
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

- Fixed test coverage CI failing on PRs from forked repositories by switching from PR comments to job summaries ([#237](https://github.com/microsoft/wassette/pull/237))
- Fixed inconsistent spelling of "wasette" to "wassette" in configuration paths and documentation comments
- Fixed broken links in README.md pointing to documentation files in wrong directory paths
- Add cargo audit configuration to acknowledge unmaintained `paste` dependency warning ([#169](https://github.com/microsoft/wassette/pull/169))
- Environment variables allowed by policy were only stored as config variables and not visible to std::env::var inside components; they are now injected into the WASI environment at instantiation ([#261](https://github.com/microsoft/wassette/pull/261))
- Fixed security audit issue by adding RUSTSEC-2025-0057 to ignore list for unmaintained fxhash crate that is transitively required by wasmtime
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
- Memory resource limiter implementation by properly applying limits to Wasmtime store using the correct `limiter()` API ([#171](https://github.com/microsoft/wassette/pull/171))
- Memory resource limits now properly applied to component execution using Wasmtime's ResourceLimiter ([#171](https://github.com/microsoft/wassette/pull/171))
- Component loading across different filesystems (EXDEV error handling) ([#109](https://github.com/microsoft/wassette/pull/109))
- Component names in README files for consistency ([#115](https://github.com/microsoft/wassette/pull/115))
- Installation instructions for Linux and Windows in README ([#120](https://github.com/microsoft/wassette/pull/120))
- Proper error handling with clear error messages for non-existent components
- CLI patterns and conventions for intuitive user experience



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

[Unreleased]: https://github.com/microsoft/wassette/compare/v0.3.0...HEAD
[v0.3.0]: https://github.com/microsoft/wassette/compare/v0.2.0...v0.3.0
[v0.2.0]: https://github.com/microsoft/wassette/compare/v0.1.0...v0.2.0
[v0.1.0]: https://github.com/microsoft/wassette/releases/tag/v0.1.0
