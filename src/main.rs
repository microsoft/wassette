// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! The main `wassette(1)` command with improved command structure.
//!
//! Commands:
//! - `wassette run` - Local MCP execution via stdio transport
//! - `wassette serve` - Remote HTTP execution for development

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
    name = "wassette",
    about = "A security-oriented runtime that runs WebAssembly Components via MCP",
    long_about = None,
    version = get_version_info()
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Run(RunConfig),
    Serve(ServeConfig),
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
struct RunConfig {
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_dir: Option<PathBuf>,
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
struct ServeConfig {
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_dir: Option<PathBuf>,

    #[arg(long, hide = true)]
    #[serde(skip)]
    stdio: bool,

    #[arg(long, hide = true)]
    #[serde(skip)]
    http: bool,
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
            if config.stdio {
                eprintln!("⚠️  WARNING: 'wassette serve --stdio' is deprecated!");
                eprintln!("   Please use 'wassette run' for MCP stdio transport.");
                eprintln!();

                let server_config: ServerConfig = config.into();
                run_server(server_config, TransportType::Stdio).await?;
            } else {
                let server_config: ServerConfig = config.into();
                run_server(server_config, TransportType::Http).await?;
            }
        }
    }

    Ok(())
}