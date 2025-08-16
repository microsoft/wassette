// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! CLI command definitions for wassette

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

use crate::format::OutputFormat;

#[derive(Parser, Debug)]
#[command(name = "wassette-mcp-server", about, long_about = None)]
pub struct Cli {
    /// Print version information
    #[arg(long, short = 'V')]
    pub version: bool,

    /// Directory where plugins are stored (ignored when using --version)
    #[arg(long)]
    pub plugin_dir: Option<std::path::PathBuf>,

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
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct Serve {
    /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wasette/components
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_dir: Option<PathBuf>,

    /// Enable stdio transport
    #[arg(long)]
    #[serde(skip)]
    pub stdio: bool,

    /// Enable SSE transport
    #[arg(long)]
    #[serde(skip)]
    pub sse: bool,

    /// Enable streamable HTTP transport  
    #[arg(long)]
    #[serde(skip)]
    pub streamable_http: bool,

    /// Set environment variables (KEY=VALUE format). Can be specified multiple times.
    #[arg(long = "env", value_parser = crate::parse_env_var)]
    #[serde(skip)]
    pub env_vars: Vec<(String, String)>,

    /// Load environment variables from a file (supports .env format)
    #[arg(long = "env-file")]
    #[serde(skip)]
    pub env_file: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum ComponentCommands {
    /// Load a WebAssembly component from a file path or OCI registry.
    Load {
        /// Path to the component (file:// or oci://)
        path: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// Unload a WebAssembly component.
    Unload {
        /// Component ID to unload
        id: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// List all loaded components.
    List {
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
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
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
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
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
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
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// Grant network permission to a component.
    Network {
        /// Component ID to grant permission to
        component_id: String,
        /// Host to grant access to
        host: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// Grant environment variable permission to a component.
    #[command(name = "environment-variable")]
    EnvironmentVariable {
        /// Component ID to grant permission to
        component_id: String,
        /// Environment variable key
        key: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
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
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// Revoke network permission from a component.
    Network {
        /// Component ID to revoke permission from
        component_id: String,
        /// Host to revoke access from
        host: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
    /// Revoke environment variable permission from a component.
    #[command(name = "environment-variable")]
    EnvironmentVariable {
        /// Component ID to revoke permission from
        component_id: String,
        /// Environment variable key
        key: String,
        /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wassette/components
        #[arg(long)]
        plugin_dir: Option<PathBuf>,
    },
}
