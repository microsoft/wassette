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
    /// Serve remotely over HTTP transports (SSE or StreamableHttp).
    Serve(Serve),
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

    /// Bind address for HTTP-based transports (SSE and StreamableHttp). Defaults to 127.0.0.1:9001
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_address: Option<String>,

    /// Path to provisioning manifest for headless deployment mode
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<PathBuf>,
}

/// HTTP transport options for the Serve command
#[derive(Args, Debug, Clone, Serialize, Deserialize, Default)]
#[group(required = false, multiple = false)]
pub struct HttpTransportFlags {
    /// Serving with SSE transport
    #[arg(long)]
    #[serde(skip)]
    pub sse: bool,

    /// Serving with streamable HTTP transport  
    #[arg(long)]
    #[serde(skip)]
    pub streamable_http: bool,
}

#[derive(Debug)]
pub enum Transport {
    Sse,
    StreamableHttp,
}

impl From<&HttpTransportFlags> for Transport {
    fn from(f: &HttpTransportFlags) -> Self {
        match (f.sse, f.streamable_http) {
            (true, false) => Transport::Sse,
            (false, true) => Transport::StreamableHttp,
            _ => Transport::Sse, // Default case: use SSE transport for serve
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum ComponentCommands {
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
        /// Search query (matches against component name and description)
        query: Option<String>,
        /// Output format
        #[arg(short = 'o', long = "output-format", default_value = "json")]
        output_format: OutputFormat,
    },
    /// Fetch and load a component from the registry.
    Get {
        /// Component name or URI from the registry
        component: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
}
