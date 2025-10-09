// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Benchmark helper to measure Wassette startup responsiveness when preloading components.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::{fs, thread};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

/// CLI arguments for the startup benchmark helper.
#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Measure Wassette startup responsiveness with preloaded components."
)]
struct Args {
    /// Path to the Wassette binary that will be benchmarked.
    #[arg(long, default_value = "target/release/wassette")]
    wassette_bin: PathBuf,

    /// Directory containing pre-built WebAssembly components (defaults to ./bin).
    #[arg(long, default_value = "bin")]
    components_dir: PathBuf,

    /// Explicit list of component files to include (relative to --components-dir unless absolute).
    #[arg(long = "component")]
    components: Vec<PathBuf>,

    /// Number of benchmark repetitions to run.
    #[arg(long, default_value_t = 3)]
    runs: u32,

    /// Maximum time (in seconds) to wait for startup logs per run.
    #[arg(long, default_value_t = 60)]
    timeout_seconds: u64,

    /// Log level for the Wassette process.
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Optional output path for writing the benchmark result as JSON.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Optional history file (JSON) to append the summary to.
    #[arg(long)]
    history_file: Option<PathBuf>,

    /// Maximum number of entries to retain in the history file.
    #[arg(long, default_value_t = 120)]
    history_limit: usize,

    /// Git commit identifier to include in the output.
    #[arg(long)]
    git_sha: Option<String>,

    /// Git reference (branch or tag) to include in the output.
    #[arg(long)]
    git_ref: Option<String>,
}

#[derive(Debug)]
struct ComponentFile {
    name: String,
    path: PathBuf,
}

#[derive(Debug, Serialize, Clone, Deserialize)]
struct RunSummary {
    run: u32,
    ready_seconds: f64,
    load_complete_seconds: f64,
    component_load_seconds: f64,
}

#[derive(Debug, Serialize, Clone, Deserialize)]
struct SummaryStats {
    ready_seconds_avg: f64,
    ready_seconds_min: f64,
    ready_seconds_max: f64,
    load_complete_seconds_avg: f64,
    load_complete_seconds_min: f64,
    load_complete_seconds_max: f64,
    component_load_seconds_avg: f64,
    component_load_seconds_min: f64,
    component_load_seconds_max: f64,
    runs: usize,
}

#[derive(Debug, Serialize, Clone, Deserialize)]
struct BenchmarkOutput {
    timestamp: DateTime<Utc>,
    git_sha: Option<String>,
    git_ref: Option<String>,
    component_count: usize,
    components: Vec<String>,
    runs: Vec<RunSummary>,
    summary: SummaryStats,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct BenchmarkHistory {
    version: u32,
    entries: Vec<BenchmarkOutput>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let components = gather_components(&args)?;
    if components.is_empty() {
        bail!("No WebAssembly components were found for the benchmark.");
    }

    let mut runs = Vec::with_capacity(args.runs as usize);
    for run_idx in 1..=args.runs {
        let result = measure_once(&args, &components, run_idx)
            .with_context(|| format!("Failed to run benchmark iteration {run_idx}"))?;
        runs.push(result);
    }

    let summary = build_summary(&runs);
    let output = BenchmarkOutput {
        timestamp: Utc::now(),
        git_sha: args.git_sha.clone(),
        git_ref: args.git_ref.clone(),
        component_count: components.len(),
        components: components.iter().map(|c| c.name.clone()).collect(),
        runs,
        summary,
    };

    if let Some(path) = &args.output {
        write_json(path, &output)?;
    } else {
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    if let Some(history_path) = &args.history_file {
        update_history(history_path, &output, args.history_limit)?;
    }

    Ok(())
}

fn gather_components(args: &Args) -> Result<Vec<ComponentFile>> {
    let dir = args
        .components_dir
        .canonicalize()
        .context("Failed to resolve components directory")?;

    let mut files = Vec::new();
    if !args.components.is_empty() {
        for component in &args.components {
            let candidate = if component.is_absolute() {
                component.clone()
            } else {
                dir.join(component)
            };

            if !candidate.exists() {
                bail!(
                    "Specified component does not exist: {}",
                    candidate.display()
                );
            }

            ensure_wasm(&candidate)?;
            files.push(ComponentFile {
                name: component_name(&candidate),
                path: candidate,
            });
        }
    } else {
        let entries = fs::read_dir(&dir)
            .with_context(|| format!("Failed to list components in {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "wasm") {
                files.push(ComponentFile {
                    name: component_name(&path),
                    path,
                });
            }
        }
    }

    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

fn ensure_wasm(path: &Path) -> Result<()> {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("wasm"))
        != Some(true)
    {
        bail!(
            "Component must be a .wasm file, but {} was provided",
            path.display()
        );
    }
    Ok(())
}

fn component_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

fn measure_once(args: &Args, components: &[ComponentFile], run_idx: u32) -> Result<RunSummary> {
    let plugin_dir = prepare_plugin_dir(components)
        .with_context(|| format!("Unable to stage components for run {run_idx}"))?;

    let (ready, load_complete) = run_wassette(args, plugin_dir.path(), run_idx)?;
    let component_load = load_complete
        .checked_sub(ready)
        .unwrap_or_else(|| Duration::from_secs(0));

    Ok(RunSummary {
        run: run_idx,
        ready_seconds: ready.as_secs_f64(),
        load_complete_seconds: load_complete.as_secs_f64(),
        component_load_seconds: component_load.as_secs_f64(),
    })
}

fn prepare_plugin_dir(components: &[ComponentFile]) -> Result<TempDir> {
    let temp_dir = TempDir::new().context("Failed to create temporary plugin directory")?;

    for component in components {
        let file_name = component
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Component has an invalid file name: {}",
                    component.path.display()
                )
            })?;
        let destination = temp_dir.path().join(file_name);
        fs::copy(&component.path, &destination).with_context(|| {
            format!(
                "Failed to copy component {} to temporary directory",
                component.path.display()
            )
        })?;
    }

    Ok(temp_dir)
}

fn run_wassette(args: &Args, plugin_dir: &Path, run_idx: u32) -> Result<(Duration, Duration)> {
    let mut command = Command::new(&args.wassette_bin);
    command
        .arg("serve")
        .arg("--sse")
        .arg("--plugin-dir")
        .arg(plugin_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("RUST_LOG", &args.log_level);

    let mut child = command.spawn().with_context(|| {
        format!(
            "Failed to spawn Wassette binary at {}",
            args.wassette_bin.display()
        )
    })?;

    let mut reader_count = 0usize;
    let (tx, rx) = mpsc::channel::<Option<String>>();

    if let Some(stdout) = child.stdout.take() {
        reader_count += 1;
        let tx = tx.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = tx.send(None);
                        break;
                    }
                    Ok(_) => {
                        if tx.send(Some(line)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Some(format!("!ERR! {err}")));
                        let _ = tx.send(None);
                        break;
                    }
                }
            }
        });
    }

    if reader_count == 0 {
        let _ = child.kill();
        let _ = child.wait();
        bail!("Failed to capture Wassette output streams for benchmark logging");
    }

    if let Some(stderr) = child.stderr.take() {
        reader_count += 1;
        let tx = tx.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = tx.send(None);
                        break;
                    }
                    Ok(_) => {
                        if tx.send(Some(line)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Some(format!("!ERR! {err}")));
                        let _ = tx.send(None);
                        break;
                    }
                }
            }
        });
    }

    let start = Instant::now();
    let timeout = Duration::from_secs(args.timeout_seconds);
    let mut ready = None;
    let mut load_complete = None;
    let mut recent_logs: Vec<String> = Vec::new();
    let mut closed_streams = 0usize;

    while (ready.is_none() || load_complete.is_none()) && closed_streams < reader_count {
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "Benchmark run {run_idx} timed out after {} seconds.\nRecent logs:\n{}",
                args.timeout_seconds,
                format_recent_logs(&recent_logs)
            );
        }

        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(line)) => {
                let clean = line.trim().to_string();
                if clean.is_empty() {
                    continue;
                }

                push_log(&mut recent_logs, clean.clone());

                if ready.is_none() && clean.contains("Components will load in the background") {
                    ready = Some(start.elapsed());
                }

                if clean.contains("Background component loading completed") {
                    load_complete = Some(start.elapsed());
                }
            }
            Ok(None) => {
                closed_streams += 1;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    let ready = ready.ok_or_else(|| {
        anyhow::anyhow!(
            "Benchmark run {run_idx} did not observe the startup log.\nRecent logs:\n{}",
            format_recent_logs(&recent_logs)
        )
    })?;

    let load_complete = load_complete.ok_or_else(|| {
        anyhow::anyhow!("Benchmark run {run_idx} did not observe the background loading completion log.\nRecent logs:\n{}",
            format_recent_logs(&recent_logs)
        )
    })?;

    Ok((ready, load_complete))
}

fn push_log(buffer: &mut Vec<String>, entry: String) {
    const MAX_LOGS: usize = 20;
    if buffer.len() == MAX_LOGS {
        buffer.remove(0);
    }
    buffer.push(entry);
}

fn format_recent_logs(logs: &[String]) -> String {
    if logs.is_empty() {
        return String::from("  <no logs captured>");
    }

    logs.iter()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_summary(runs: &[RunSummary]) -> SummaryStats {
    let ready: Vec<f64> = runs.iter().map(|run| run.ready_seconds).collect();
    let load_complete: Vec<f64> = runs.iter().map(|run| run.load_complete_seconds).collect();
    let component_load: Vec<f64> = runs.iter().map(|run| run.component_load_seconds).collect();

    SummaryStats {
        ready_seconds_avg: average(&ready),
        ready_seconds_min: min(&ready),
        ready_seconds_max: max(&ready),
        load_complete_seconds_avg: average(&load_complete),
        load_complete_seconds_min: min(&load_complete),
        load_complete_seconds_max: max(&load_complete),
        component_load_seconds_avg: average(&component_load),
        component_load_seconds_min: min(&component_load),
        component_load_seconds_max: max(&component_load),
        runs: runs.len(),
    }
}

fn average(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn min(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().cloned().fold(f64::INFINITY, f64::min)
}

fn max(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
}

fn write_json(path: &Path, output: &BenchmarkOutput) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(output)?;
    fs::write(path, json).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn update_history(path: &Path, entry: &BenchmarkOutput, limit: usize) -> Result<()> {
    let mut history = if path.exists() {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read history file {}", path.display()))?;
        serde_json::from_str::<BenchmarkHistory>(&content)
            .with_context(|| format!("Failed to deserialize history file {}", path.display()))?
    } else {
        BenchmarkHistory {
            version: 1,
            entries: Vec::new(),
        }
    };

    history.version = history.version.max(1);
    history.entries.push(entry.clone());
    history
        .entries
        .sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    if history.entries.len() > limit {
        let remove = history.entries.len() - limit;
        history.entries.drain(0..remove);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(&history)?;
    fs::write(path, json)
        .with_context(|| format!("Failed to write benchmark history file {}", path.display()))?;

    Ok(())
}
