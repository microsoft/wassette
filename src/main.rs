// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! The main `wassette(1)` command.
//! 
//! DEPRECATED: This binary is deprecated in favor of the `weld` command.
//! - Use `weld run` instead of `wassette serve --stdio`
//! - Use `weld serve` instead of `wassette serve --http`

#![warn(missing_docs)]

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

mod config;
mod server;

use server::{get_version_info, run_server, ServerConfig, TransportType};


#[derive(Parser, Debug)]
#[command(
    name = "wassette-mcp-server", 
    about = "DEPRECATED: Use 'weld run' for MCP stdio or 'weld serve' for HTTP", 
    long_about = "This binary is deprecated in favor of the 'weld' command.\n\nMigration guide:\n- 'wassette serve --stdio' → 'weld run'\n- 'wassette serve --http' → 'weld serve'", 
    version = get_version_info()
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Begin handling requests over the specified protocol. DEPRECATED: Use 'weld run' or 'weld serve'
    Serve(Serve),
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
struct Serve {
    /// Directory where plugins are stored. Defaults to $XDG_DATA_HOME/wasette/components
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_dir: Option<PathBuf>,

    /// Enable stdio transport. DEPRECATED: Use 'weld run' instead
    #[arg(long)]
    #[serde(skip)]
    stdio: bool,

    /// Enable HTTP transport. DEPRECATED: Use 'weld serve' instead
    #[arg(long)]
    #[serde(skip)]
    http: bool,
}

impl From<Serve> for ServerConfig {
    fn from(serve: Serve) -> Self {
        ServerConfig {
            plugin_dir: serve.plugin_dir,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Serve(cfg) => {
            // Print deprecation warning
            eprintln!("⚠️  WARNING: 'wassette serve' is deprecated!");
            eprintln!("   Migration guide:");
            if cfg.stdio || (!cfg.stdio && !cfg.http) {
                eprintln!("   - Use 'weld run' instead of 'wassette serve --stdio'");
            }
            if cfg.http {
                eprintln!("   - Use 'weld serve' instead of 'wassette serve --http'");
            }
            eprintln!();

            // Determine transport type
            let transport_type = match (cfg.stdio, cfg.http) {
                (false, false) => TransportType::Stdio, // Default case: use stdio transport
                (true, false) => TransportType::Stdio,  // Stdio transport only
                (false, true) => TransportType::Http,   // HTTP transport only
                (true, true) => {
                    return Err(anyhow::anyhow!(
                        "Running both stdio and HTTP transports simultaneously is not supported. Please choose one."
                    ));
                }
            };

            let server_config: ServerConfig = cfg.clone().into();
            run_server(server_config, transport_type).await?;
        }
    }

    Ok(())
}
