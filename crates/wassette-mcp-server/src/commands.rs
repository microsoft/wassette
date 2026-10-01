// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! CLI command definitions for wassette

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::format::OutputFormat;

/// Supported shell types for completion generation
#[derive(ValueEnum, Clone, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum Shell {
    /// Bash shell
    Bash,
    /// Zsh shell
    Zsh,
    /// Fish shell
    Fish,
    /// PowerShell
    PowerShell,
    /// Elvish shell
    Elvish,
}

#[derive(Parser, Debug)]
#[command(
    name = "wassette-mcp-server",
    about = "A security-oriented runtime that runs WebAssembly Components via MCP",
    long_about = None
)]
pub struct Cli {
    /// Print version information
    #[arg(long, short = 'V')]
    pub version: bool,

    /// Directory where components are stored (ignored when using --version)
    #[arg(long)]
    pub component_dir: Option<std::path::PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run locally with stdio transport (for local development and testing).
    Run(Run),
    /// Serve remotely over Streamable HTTP.
    Serve(Serve),
    /// [EXPERIMENTAL] Run as an ACP agent over stdio; may change or be removed.
    Acp(wassette_acp::AcpArgs),
    /// Manage WebAssembly components.
    Component {
        #[command(subcommand)]
        command: ComponentCommands,
    },
    /// Manage component policies.
    Policy {
        #[command(subcommand)]
        command: PolicyCommands,
    },
    /// Manage component permissions.
    Permission {
        #[command(subcommand)]
        command: PermissionCommands,
    },
    /// Manage component secrets.
    Secret {
        #[command(subcommand)]
        command: SecretCommands,
    },
    /// Inspect a WebAssembly component and display its JSON schema (for debugging).
    Inspect {
        /// Component ID to inspect
        component_id: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
    /// Manage tools (list, read, invoke).
    Tool {
        #[command(subcommand)]
        command: ToolCommands,
    },
    /// Search and fetch components from the registry.
    Registry {
        #[command(subcommand)]
        command: RegistryCommands,
    },
    /// Generate shell completion scripts.
    Autocomplete {
        /// Shell type to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}

/// Configuration for running locally with stdio transport
#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_dir: Option<PathBuf>,

    /// Trusted operator JSON profile for isolated component generation (disabled when unset).
    #[cfg(feature = "component-generation")]
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<PathBuf>,

    /// Directory to scan for locally built components.
    #[arg(long)]
    #[serde(skip)]
    pub local_component_dir: Option<PathBuf>,

    /// Local component discovery: off, startup, or watch.
    #[arg(long, value_enum)]
    #[serde(skip)]
    pub local_components: Option<LocalComponentsMode>,

    /// Set environment variables (KEY=VALUE format). Can be specified multiple times.
    #[arg(long = "env", value_parser = crate::parse_env_var)]
    #[serde(skip)]
    pub env_vars: Vec<(String, String)>,

    /// Load environment variables from a file (supports .env format)
    #[arg(long = "env-file")]
    #[serde(skip)]
    pub env_file: Option<PathBuf>,

    /// Disable built-in tools (load-component, unload-component, list-components, etc.)
    #[arg(long)]
    #[serde(default)]
    pub disable_builtin_tools: bool,
}

/// Configuration for serving remotely over HTTP transports
#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct Serve {
    /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_dir: Option<PathBuf>,

    /// Trusted operator JSON profile for isolated component generation (disabled when unset).
    #[cfg(feature = "component-generation")]
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<PathBuf>,

    /// Directory to scan for locally built components.
    #[arg(long)]
    #[serde(skip)]
    pub local_component_dir: Option<PathBuf>,

    /// Local component discovery: off, startup, or watch.
    #[arg(long, value_enum)]
    #[serde(skip)]
    pub local_components: Option<LocalComponentsMode>,

    #[command(flatten)]
    pub transport: HttpTransportFlags,

    /// Set environment variables (KEY=VALUE format). Can be specified multiple times.
    #[arg(long = "env", value_parser = crate::parse_env_var)]
    #[serde(skip)]
    pub env_vars: Vec<(String, String)>,

    /// Load environment variables from a file (supports .env format)
    #[arg(long = "env-file")]
    #[serde(skip)]
    pub env_file: Option<PathBuf>,

    /// Disable built-in tools (load-component, unload-component, list-components, etc.)
    #[arg(long)]
    #[serde(default)]
    pub disable_builtin_tools: bool,

    /// Bind address for Streamable HTTP. Defaults to 127.0.0.1:9001
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_address: Option<String>,

    /// Path to provisioning manifest for headless deployment mode
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<PathBuf>,

    /// Start the server even when some manifest components fail to provision, serving
    /// only the components that did load. Failures are still logged at error level.
    /// Without this flag a single provisioning failure aborts startup.
    #[arg(long)]
    #[serde(default)]
    pub continue_on_provisioning_failure: bool,

    /// Hostname or host:port authority to accept in the inbound `Host` header for
    /// Streamable HTTP. Repeat the flag to allow several. Replaces the default allowlist
    /// rather than adding to it, so include `localhost` and `127.0.0.1` explicitly if
    /// loopback clients must keep working. When unset, only loopback is accepted.
    // Serialization is skipped and the value applied explicitly in `Config::from_serve`,
    // as `env_vars` and `env_file` are, because figment's `admerge` would concatenate
    // this list with a configured one rather than overriding it.
    #[arg(long = "allowed-host", value_name = "HOST")]
    #[serde(skip)]
    pub allowed_hosts: Option<Vec<String>>,
    /// Keep serving the pre-2026-07-28 session lifecycle (default: true).
    ///
    /// Requests that negotiate protocol revision 2026-07-28 or later are always
    /// served statelessly, so turning this off only removes the session
    /// lifecycle (and the GET SSE stream) that older clients depend on.
    #[arg(long, value_name = "BOOL")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_sessions: Option<bool>,

    /// Reply to a simple stateless request with `application/json` instead of a
    /// request-scoped `text/event-stream` (default: false)
    #[arg(long, num_args = 0..=1, default_missing_value = "true", value_name = "BOOL")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_response: Option<bool>,
}

/// When to reconcile locally built components from the drop directory.
#[derive(ValueEnum, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LocalComponentsMode {
    /// Do not scan the drop directory.
    Off,
    /// Reconcile once at startup.
    Startup,
    /// Reconcile at startup and on subsequent source changes.
    Watch,
}

/// HTTP transport options for the Serve command
#[derive(Args, Debug, Clone, Serialize, Deserialize, Default)]
#[group(required = false, multiple = false)]
pub struct HttpTransportFlags {
    /// Serving with Streamable HTTP transport
    #[arg(long)]
    #[serde(skip)]
    pub streamable_http: bool,
}

#[derive(Debug)]
pub enum Transport {
    StreamableHttp,
}

impl From<&HttpTransportFlags> for Transport {
    fn from(_flags: &HttpTransportFlags) -> Self {
        Transport::StreamableHttp
    }
}

#[derive(Subcommand, Debug)]
pub enum ComponentCommands {
    /// Build in an isolated helper, validate, and install using the trusted operator profile.
    #[cfg(feature = "component-generation")]
    Build {
        /// Bounded generation request JSON file; never a compiler/profile path.
        request: PathBuf,
        /// Override the managed component store directory.
        #[arg(long)]
        component_dir: Option<PathBuf>,
        /// Trusted operator JSON profile (also WASSETTE_GENERATION_CONFIG or config.toml).
        #[arg(long)]
        generation_config: Option<PathBuf>,
        /// Also write a rebuildable source layout to this directory.
        #[arg(long)]
        emit_source: Option<PathBuf>,
    },
    /// Read the retained source of a generated component.
    #[cfg(feature = "component-generation")]
    Source {
        /// Installed generated component ID.
        id: String,
        /// Require this exact installed revision.
        #[arg(long)]
        revision: Option<String>,
        /// Write src/lib.rs, wit/world.wit and request.json instead of printing JSON.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Allow writing into a non-empty output directory.
        #[arg(long, requires = "out")]
        force: bool,
        /// Override the managed component store directory.
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
    /// Load a WebAssembly component from a file path or OCI registry.
    Load {
        /// Path to the component (file:// or oci://)
        path: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
    /// Unload a WebAssembly component.
    Unload {
        /// Component ID to unload
        id: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
    /// List all loaded components.
    List {
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
        /// Output format
        #[arg(short = 'o', long = "output-format", default_value = "json")]
        output_format: OutputFormat,
    },
    /// Reconcile locally built components from the drop directory.
    Sync {
        /// Override the managed component store directory.
        #[arg(long)]
        component_dir: Option<PathBuf>,
        /// Override the local component drop directory.
        #[arg(long)]
        local_component_dir: Option<PathBuf>,
        /// Link a finished local build into the drop directory before reconciling.
        #[arg(long = "link", value_name = "WASM")]
        links: Vec<PathBuf>,
        /// Adopt a matching explicit local-file installation into managed links.
        #[arg(long, requires = "links")]
        adopt_explicit_local: bool,
        /// Reinstall a previously explicitly unloaded, unchanged local component.
        #[arg(long)]
        force: bool,
        /// Output format for the reconciliation report.
        #[arg(short = 'o', long = "output-format", default_value = "json")]
        output_format: OutputFormat,
    },
}

#[cfg(test)]
mod local_source_tests {
    use super::*;

    #[test]
    fn local_source_flags_parse_for_run_serve_and_sync() {
        for command in ["run", "serve"] {
            let args = Cli::try_parse_from([
                "wassette",
                command,
                "--local-component-dir",
                "inbox",
                "--local-components",
                "startup",
            ])
            .unwrap();
            match args.command.unwrap() {
                Commands::Run(cfg) => {
                    assert_eq!(cfg.local_component_dir, Some(PathBuf::from("inbox")));
                    assert_eq!(cfg.local_components, Some(LocalComponentsMode::Startup));
                }
                Commands::Serve(cfg) => {
                    assert_eq!(cfg.local_component_dir, Some(PathBuf::from("inbox")));
                    assert_eq!(cfg.local_components, Some(LocalComponentsMode::Startup));
                }
                _ => panic!("unexpected command"),
            }
        }
        let args = Cli::try_parse_from([
            "wassette",
            "component",
            "sync",
            "--local-component-dir",
            "inbox",
            "--link",
            "one.wasm",
            "--link",
            "two.wasm",
            "--force",
        ])
        .unwrap();
        let Some(Commands::Component {
            command:
                ComponentCommands::Sync {
                    force: true,
                    adopt_explicit_local: false,
                    links,
                    ..
                },
        }) = args.command
        else {
            panic!("expected component sync");
        };
        assert_eq!(
            links,
            [PathBuf::from("one.wasm"), PathBuf::from("two.wasm")]
        );

        let args = Cli::try_parse_from([
            "wassette",
            "component",
            "sync",
            "--link",
            "one.wasm",
            "--adopt-explicit-local",
        ])
        .unwrap();
        let Some(Commands::Component {
            command:
                ComponentCommands::Sync {
                    adopt_explicit_local: true,
                    ..
                },
        }) = args.command
        else {
            panic!("expected explicit local adoption");
        };
    }
}

#[cfg(test)]
mod generation_tests {
    use super::*;

    #[test]
    fn generation_cli_is_feature_gated() {
        for command in ["run", "serve"] {
            let args =
                Cli::try_parse_from(["wassette", command, "--generation-config", "operator.json"]);
            assert_eq!(args.is_ok(), cfg!(feature = "component-generation"));
        }
        let args = Cli::try_parse_from([
            "wassette",
            "component",
            "build",
            "request.json",
            "--generation-config",
            "operator.json",
        ]);
        assert_eq!(args.is_ok(), cfg!(feature = "component-generation"));
    }

    #[cfg(feature = "component-generation")]
    #[test]
    fn generation_flags_default_to_disabled_and_accept_only_operator_paths() {
        for command in ["run", "serve"] {
            let args = Cli::try_parse_from(["wassette", command]).unwrap();
            match args.command.unwrap() {
                Commands::Run(run) => assert!(run.generation_config.is_none()),
                Commands::Serve(serve) => assert!(serve.generation_config.is_none()),
                _ => unreachable!(),
            }
        }
        assert!(Cli::try_parse_from([
            "wassette",
            "component",
            "build",
            "request.json",
            "--allow-expose",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "wassette",
            "component",
            "build",
            "request.json",
            "--compiler-flags",
            "-O",
        ])
        .is_err());
    }
}

#[derive(Subcommand, Debug)]
pub enum PolicyCommands {
    /// Get policy information for a component.
    Get {
        /// Component ID to get policy for
        component_id: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
        /// Output format
        #[arg(short = 'o', long = "output-format", default_value = "json")]
        output_format: OutputFormat,
    },
}

#[derive(Subcommand, Debug)]
pub enum PermissionCommands {
    /// Grant permissions to a component.
    Grant {
        #[command(subcommand)]
        permission: GrantPermissionCommands,
    },
    /// Revoke permissions from a component.
    Revoke {
        #[command(subcommand)]
        permission: RevokePermissionCommands,
    },
    /// Reset all permissions for a component.
    Reset {
        /// Component ID to reset permissions for
        component_id: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum GrantPermissionCommands {
    /// Grant storage permission to a component.
    #[command(after_help = "EXAMPLES:
    # Grant read-only access to a directory
    wassette permission grant storage my-component fs:///tmp/cache --access read

    # Grant read and write access to a directory
    wassette permission grant storage my-component fs:///tmp/output --access read,write

    # Grant write-only access to a workspace
    wassette permission grant storage my-component fs:///home/user/workspace --access write")]
    Storage {
        /// Component ID to grant permission to
        component_id: String,
        /// URI of the storage resource (e.g., fs:///path/to/directory)
        uri: String,
        /// Access level (read, write, or read,write)
        #[arg(long, value_delimiter = ',')]
        access: Vec<String>,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
    /// Grant network permission to a component.
    #[command(after_help = "EXAMPLES:
    # Grant access to a specific API endpoint
    wassette permission grant network my-component api.example.com

    # Grant access to a backup server
    wassette permission grant network my-component backup.example.com

    # Grant access to a CDN
    wassette permission grant network my-component cdn.example.com")]
    Network {
        /// Component ID to grant permission to
        component_id: String,
        /// Host to grant access to
        host: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
    /// Grant environment variable permission to a component.
    #[command(
        name = "environment-variable",
        after_help = "EXAMPLES:
    # Grant access to an API key environment variable
    wassette permission grant environment-variable my-component API_KEY

    # Grant access to a configuration URL
    wassette permission grant environment-variable my-component CONFIG_URL

    # Grant access to a database connection string
    wassette permission grant environment-variable my-component DATABASE_URL"
    )]
    EnvironmentVariable {
        /// Component ID to grant permission to
        component_id: String,
        /// Environment variable key
        key: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
    /// Grant memory permission to a component.
    #[command(after_help = "EXAMPLES:
    # Grant 512 MiB memory limit
    wassette permission grant memory my-component 512Mi

    # Grant 1 GiB memory limit
    wassette permission grant memory my-component 1Gi

    # Grant 2048 KiB memory limit
    wassette permission grant memory my-component 2048Ki")]
    Memory {
        /// Component ID to grant permission to
        component_id: String,
        /// Memory limit (e.g., 512Mi, 1Gi, 2048Ki)
        limit: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum RevokePermissionCommands {
    /// Revoke storage permission from a component.
    Storage {
        /// Component ID to revoke permission from
        component_id: String,
        /// URI of the storage resource (e.g., fs:///path/to/directory)
        uri: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
    /// Revoke network permission from a component.
    Network {
        /// Component ID to revoke permission from
        component_id: String,
        /// Host to revoke access from
        host: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
    /// Revoke environment variable permission from a component.
    #[command(name = "environment-variable")]
    EnvironmentVariable {
        /// Component ID to revoke permission from
        component_id: String,
        /// Environment variable key
        key: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SecretCommands {
    /// List secrets for a component.
    List {
        /// Component ID to list secrets for
        component_id: String,
        /// Show secret values (prompts for confirmation)
        #[arg(long)]
        show_values: bool,
        /// Skip confirmation prompt when showing values
        #[arg(long)]
        yes: bool,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
        /// Output format
        #[arg(short = 'o', long = "output-format", default_value = "json")]
        output_format: OutputFormat,
    },
    /// Set secrets for a component.
    Set {
        /// Component ID to set secrets for
        component_id: String,
        /// Secrets in KEY=VALUE format. Can be specified multiple times.
        #[arg(value_parser = crate::parse_env_var)]
        secrets: Vec<(String, String)>,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
    /// Delete secrets for a component.
    Delete {
        /// Component ID to delete secrets from
        component_id: String,
        /// Secret keys to delete
        keys: Vec<String>,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ToolCommands {
    /// List all available tools.
    List {
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
        /// Output format
        #[arg(short = 'o', long = "output-format", default_value = "json")]
        output_format: OutputFormat,
    },
    /// Read details of a specific tool.
    Read {
        /// Name of the tool to read
        name: String,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
        /// Output format
        #[arg(short = 'o', long = "output-format", default_value = "json")]
        output_format: OutputFormat,
    },
    /// Invoke a tool with parameters.
    Invoke {
        /// Name of the tool to invoke
        name: String,
        /// Arguments in JSON format (e.g., '{"key": "value"}')
        #[arg(long)]
        args: Option<String>,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        component_dir: Option<PathBuf>,
        /// Output format
        #[arg(short = 'o', long = "output-format", default_value = "json")]
        output_format: OutputFormat,
    },
}

#[derive(Subcommand, Debug)]
pub enum RegistryCommands {
    /// Search for components in the registry.
    Search {
        /// Search query sent to wasm.directory
        query: Option<String>,
        /// Offset into the upstream result set
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Maximum number of upstream records to fetch (1-100)
        #[arg(long, default_value_t = wassette::wasm_directory::DEFAULT_SEARCH_PAGE_SIZE)]
        limit: usize,
        /// Output format
        #[arg(short = 'o', long = "output-format", default_value = "json")]
        output_format: OutputFormat,
    },
    /// Install a package from wasm.directory without exposing its tools.
    Get {
        /// Package as registry/repository, or an exact WIT identity
        /// namespace:package[@version] matching exactly one package
        component: String,
        /// Exact indexed package version to install
        #[arg(long)]
        version: Option<String>,
        /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long = "component-dir", visible_alias = "plugin-dir")]
        plugin_dir: Option<PathBuf>,
        /// Output format
        #[arg(short = 'o', long = "output-format", default_value = "json")]
        output_format: OutputFormat,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_get_accepts_wit_selector() {
        let cli =
            Cli::try_parse_from(["wassette", "registry", "get", "yosh:wordmark@2.0.6"]).unwrap();
        let Some(Commands::Registry {
            command: RegistryCommands::Get { component, .. },
        }) = cli.command
        else {
            panic!("expected registry get");
        };
        assert_eq!(component, "yosh:wordmark@2.0.6");
    }

    #[test]
    fn registry_get_accepts_canonical_package_and_exact_version() {
        let cli = Cli::try_parse_from([
            "wassette",
            "registry",
            "get",
            "ghcr.io/owner/component",
            "--version",
            "1.2.3",
            "--component-dir",
            "/tmp/components",
        ])
        .unwrap();
        let Some(Commands::Registry {
            command:
                RegistryCommands::Get {
                    component,
                    version,
                    plugin_dir,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected registry get command");
        };
        assert_eq!(component, "ghcr.io/owner/component");
        assert_eq!(version.as_deref(), Some("1.2.3"));
        assert_eq!(
            plugin_dir.as_deref(),
            Some(std::path::Path::new("/tmp/components"))
        );

        let legacy = Cli::try_parse_from([
            "wassette",
            "registry",
            "get",
            "ghcr.io/owner/component",
            "--plugin-dir",
            "/tmp/components",
        ])
        .unwrap();
        assert!(matches!(
            legacy.command,
            Some(Commands::Registry {
                command: RegistryCommands::Get {
                    plugin_dir: Some(_),
                    ..
                }
            })
        ));
    }
}
