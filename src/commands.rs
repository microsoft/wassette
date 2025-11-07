// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! CLI command definitions for wassette

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};

use crate::format::OutputFormat;

#[derive(Parser, Debug)]
#[command(name = "wassette-mcp-server", about, long_about = None)]
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
    /// Begin handling requests over the specified protocol.
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
        /// Path to the WebAssembly component file
        path: PathBuf,
    },
    /// Manage tools (list, read, invoke).
    Tool {
        #[command(subcommand)]
        command: ToolCommands,
    },
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct Serve {
    /// Directory where components are stored. Defaults to $XDG_DATA_HOME/wassette/components
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_dir: Option<PathBuf>,

    #[command(flatten)]
    pub transport: TransportFlags,

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
}

#[derive(Args, Debug, Clone, Serialize, Deserialize, Default)]
#[group(required = false, multiple = false)]
pub struct TransportFlags {
    /// Serving with SSE transport
    #[arg(long)]
    #[serde(skip)]
    pub sse: bool,

    /// Serving with stdio transport
    #[arg(long)]
    #[serde(skip)]
    pub stdio: bool,

    /// Serving with streamable HTTP transport  
    #[arg(long)]
    #[serde(skip)]
    pub streamable_http: bool,
}

#[derive(Debug)]
pub enum Transport {
    Sse,
    Stdio,
    StreamableHttp,
}

impl From<&TransportFlags> for Transport {
    fn from(f: &TransportFlags) -> Self {
        match (f.sse, f.stdio, f.streamable_http) {
            (true, false, false) => Transport::Sse,
            (false, true, false) => Transport::Stdio,
            (false, false, true) => Transport::StreamableHttp,
            _ => Transport::Stdio, // Default case: use stdio transport
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
    #[command(name = "environment-variable")]
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
