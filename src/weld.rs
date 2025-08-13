// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! The `weld` command - provides secure local and remote execution of WebAssembly components.
//!
//! This binary replaces `wassette serve` with clearer separation of local vs remote execution:
//! - `weld run` - Local MCP execution (equivalent to `wassette serve --stdio`)
//! - `weld serve` - Remote HTTP execution (equivalent to `wassette serve --http`)

#![warn(missing_docs)]

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

use wassette_mcp_server::server::{get_version_info, run_server, ServerConfig, TransportType};

#[derive(Parser, Debug)]
#[command(name = "weld", about = "Secure WebAssembly component runtime for AI agents", long_about = None, version = get_version_info())]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run locally with MCP stdio transport for AI agent integration
    Run(RunConfig),
    /// Serve remotely with HTTP transport for development and testing
    Serve(ServeConfig),
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
struct RunConfig {
    /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wasette/components
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_dir: Option<PathBuf>,
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
struct ServeConfig {
    /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wasette/components
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_dir: Option<PathBuf>,
}

impl From<RunConfig> for ServerConfig {
    fn from(config: RunConfig) -> Self {
        ServerConfig {
            plugin_dir: config.plugin_dir,
        }
    }
}

impl From<ServeConfig> for ServerConfig {
    fn from(config: ServeConfig) -> Self {
        ServerConfig {
            plugin_dir: config.plugin_dir,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run(config) => {
            let server_config: ServerConfig = config.into();
            run_server(server_config, TransportType::Stdio).await?;
        }
        Commands::Serve(config) => {
            let server_config: ServerConfig = config.into();
            run_server(server_config, TransportType::Http).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_config_conversion() {
        let run_config = RunConfig {
            plugin_dir: Some(PathBuf::from("/test/plugins")),
        };
        
        let server_config: ServerConfig = run_config.into();
        assert_eq!(server_config.plugin_dir, Some(PathBuf::from("/test/plugins")));
    }

    #[test]
    fn test_serve_config_conversion() {
        let serve_config = ServeConfig {
            plugin_dir: Some(PathBuf::from("/test/plugins")),
        };
        
        let server_config: ServerConfig = serve_config.into();
        assert_eq!(server_config.plugin_dir, Some(PathBuf::from("/test/plugins")));
    }

    #[test]
    fn test_config_with_none_plugin_dir() {
        let run_config = RunConfig {
            plugin_dir: None,
        };
        
        let server_config: ServerConfig = run_config.into();
        assert_eq!(server_config.plugin_dir, None);
    }
}