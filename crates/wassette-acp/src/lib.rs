// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! ACP host for Wassette.
//!
//! Loads an ACP agent component and bridges it to the editor over the ACP
//! JSON-RPC wire protocol on stdio. Logs go to stderr — stdout is the
//! protocol channel. Configure verbosity with the `RUST_LOG` environment
//! variable (e.g. `RUST_LOG=wassette_acp=debug`), or with `--log-level` /
//! `--log-filter`. Pass `--log-file <path>` to also write logs to a file
//! (useful for debugging when stderr is hidden behind the editor).
//!
//! The crate is driven through [`AcpArgs`] and [`run`], which
//! `wassette acp` wires up as a subcommand.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use etcetera::BaseStrategy;
use tokio::sync::mpsc;
use tokio::task::LocalSet;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use wasmtime::component::Component;
use wasmtime::{Config, Engine};

mod bridge;
mod client_impl;
mod group;
mod http_policy;
mod install;
mod sandbox;
mod secrets;
mod secrets_impl;
mod state;
mod translate;
mod wasi_log;
mod wasm;

// Generate wasmtime component bindings for both ACP worlds.
//
// The `layer` world is a superset of `provider`: same exports plus an
// additional `import agent;` so a layer can forward downstream. We
// generate them as separate top-level types (`Provider`, `Layer`) so
// the rest of the host can statically distinguish a terminal stage from
// an intermediate one. The `with:` clause on the layer makes it reuse
// the provider's interface types verbatim — every WIT record/variant is
// defined exactly once under `crate::yosh::acp::*`, and a single
// set of `Host` trait impls on `HostState` satisfies both linkers.
//
// Bindgen flips imports/exports from the host's perspective: imported
// interfaces (`client` for both worlds, plus `agent` for `layer`) become
// `Host` traits we implement; exported interfaces (`agent`) become
// callable methods on the wrapper struct.
wasmtime::component::bindgen!({
    path: "wit/acp",
    world: "provider",
    imports: { default: async },
    exports: { default: async },
});

mod layer_bindings {
    // The layer bindgen lives in its own module so its generated
    // `exports` module and `Layer` world wrapper don't collide with
    // the provider's. Interface types are shared via `with:` so every
    // WIT record/variant is still defined exactly once at the crate
    // root, and a single set of `Host` impls on `HostState` satisfies
    // both linkers.
    wasmtime::component::bindgen!({
        path: "wit/acp",
        world: "layer",
        imports: { default: async },
        exports: { default: async },
        with: {
            "yosh:acp/errors": crate::yosh::acp::errors,
            "yosh:acp/content": crate::yosh::acp::content,
            "yosh:acp/init": crate::yosh::acp::init,
            "yosh:acp/sessions": crate::yosh::acp::sessions,
            "yosh:acp/prompts": crate::yosh::acp::prompts,
            "yosh:acp/tools": crate::yosh::acp::tools,
            "yosh:acp/terminals": crate::yosh::acp::terminals,
            "yosh:acp/filesystem": crate::yosh::acp::filesystem,
            "yosh:acp/agent": crate::yosh::acp::agent,
            "yosh:acp/client": crate::yosh::acp::client,
            "wasmcloud:secrets/store@0.1.0-draft": crate::wasmcloud::secrets::store,
            "wasmcloud:secrets/reveal@0.1.0-draft": crate::wasmcloud::secrets::reveal,
        },
    });
}

pub use layer_bindings::Layer;

use crate::install::Resolver;
use crate::sandbox::Sandbox;
use crate::state::StageKind;
use crate::wasm::{SessionFactory, SessionRegistry, Stage};
/// `Host` trait for the layer's *imported* `agent` interface. Since the
/// `with:` clause on the layer bindgen shares this interface with the
/// provider's top-level bindgen (both worlds import `agent` for the
/// `session` resource's destructor), `crate::layer_agent` and
/// `crate::yosh::acp::agent` point to the same module. A single
/// `HostWithStore` impl on `HasSelf<HostState>` therefore satisfies
/// both worlds' linkers.
pub use crate::yosh::acp::agent as layer_agent;

/// Arguments for `wassette acp`: run Wassette as an ACP agent whose brain
/// is a WebAssembly component.
#[derive(clap::Args, Debug)]
pub struct AcpArgs {
    /// Path, URI, or component id of a terminal ACP **provider** wasm
    /// component (the bottom of a chain). At least one is required.
    ///
    /// May be passed multiple times to load several providers at once.
    /// Every provider is instantiated for each session and its models
    /// are merged into a single **model** selector, labelled by
    /// provider, so the user can pick which model from which provider
    /// backs the session (the rest of the selectors — mode, thinking,
    /// … — follow the provider owning the active model). The same set
    /// of `--layer`s wraps every provider.
    ///
    /// Accepts anything `wassette component load` does — a filesystem
    /// path (`./my-agent.wasm`), an `oci://` reference, or an `https://`
    /// URL — plus the id of a component already in the component
    /// directory. Downloads are stored in the component directory, so a
    /// component only has to be fetched once.
    #[arg(long = "provider", value_name = "PATH|URI|COMPONENT_ID")]
    pub providers: Vec<String>,

    /// Path, URI, or component id of a **layer** wasm component to wrap
    /// the providers. May be passed multiple times; layers are applied
    /// editor-side → provider-side in the order given (the first
    /// `--layer` is the outermost stage closest to the host).
    /// Same syntax as `--provider`.
    #[arg(long = "layer", value_name = "PATH|URI|COMPONENT_ID")]
    pub layers: Vec<String>,

    /// Directory where components are stored. Defaults to
    /// `$XDG_DATA_HOME/wassette/components` — the same store
    /// `wassette component load` writes to.
    #[arg(long)]
    pub component_dir: Option<PathBuf>,

    /// Directory where component secrets are stored. Defaults to
    /// `$XDG_CONFIG_HOME/wassette/secrets` — the same store
    /// `wassette secret set` writes to.
    #[arg(long)]
    pub secrets_dir: Option<PathBuf>,

    /// Run every stage with the host's network and environment instead of
    /// its Wassette policy.
    ///
    /// By default each provider and layer is sandboxed from its
    /// `<component-id>.policy.yaml` (looked up in the component
    /// directory, then beside the `.wasm`), exactly as
    /// `wassette component load` + `wassette policy attach` set it up for
    /// MCP. **A component with no policy therefore gets no network and no
    /// filesystem access beyond its own per-session `/data` directory.**
    /// Grant reach with a policy — `permissions.network.allow` for hosts,
    /// `permissions.storage.allow` for paths, `permissions.environment.allow`
    /// for environment variables — or pass this flag to skip policy
    /// enforcement entirely. Intended for demos and local debugging.
    #[arg(long)]
    pub allow_all: bool,

    /// Optional path to a file to mirror logs into. The same events that
    /// go to stderr are appended to this file (no ANSI colors). Useful
    /// when running under an editor that swallows or hides the host's
    /// stderr.
    #[arg(long)]
    pub log_file: Option<PathBuf>,

    /// Coarse log level. Equivalent to `RUST_LOG=wassette_acp=<level>`.
    /// Use `--log-filter` for full `tracing` directive syntax (per-target
    /// levels). `RUST_LOG`, if set, takes precedence over both flags.
    #[arg(long, value_enum, default_value_t = LogLevel::Info)]
    pub log_level: LogLevel,

    /// Full `tracing-subscriber` env-filter directive. Overrides
    /// `--log-level` when set. Example:
    /// `--log-filter "wassette_acp=debug,agent_client_protocol=trace"`.
    #[arg(long)]
    pub log_filter: Option<String>,
}

/// Coarse verbosity for the host's own logs.
#[derive(Copy, Clone, Debug, clap::ValueEnum)]
pub enum LogLevel {
    /// Everything, including per-message wire traces.
    Trace,
    /// Debugging detail.
    Debug,
    /// Lifecycle events (the default).
    Info,
    /// Recoverable problems only.
    Warn,
    /// Failures only.
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

/// Run the ACP host: resolve the provider/layer chain, then speak ACP
/// JSON-RPC on stdio until the client disconnects.
///
/// Session actors are `!Send` (they own a `Store<HostState>`), so the
/// work happens inside a [`LocalSet`] pinned to the calling thread of the
/// *current* runtime — no nested runtime is created.
pub async fn run(args: AcpArgs) -> Result<()> {
    // rustls 0.23 links both crypto backends in this dependency graph
    // (wasmtime-wasi-http + oci-client pull `aws-lc-rs`; reqwest/hyper-rustls
    // pull `ring`), so it cannot auto-select a process-level CryptoProvider
    // and panics on the first outbound TLS handshake made by a guest. Install
    // `aws-lc-rs` explicitly to match wasmtime's TLS backend. Idempotent — the
    // `Err` (provider already installed) is safe to ignore.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    init_logging(&args)?;

    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_more_async_builtins(true);
    config.wasm_component_model_async_stackful(true);
    config.wasm_features(wasmtime::WasmFeatures::CM_ASYNC, true);
    config.wasm_features(wasmtime::WasmFeatures::CM_MORE_ASYNC_BUILTINS, true);
    config.wasm_features(wasmtime::WasmFeatures::CM_ASYNC_STACKFUL, true);
    let engine = Engine::new(&config)?;

    if args.providers.is_empty() {
        anyhow::bail!(
            "missing provider wasm component: pass `--provider <path|uri|component-id>` \
             (repeatable)"
        );
    }

    let component_dir = match args.component_dir.clone() {
        Some(dir) => dir,
        None => default_component_dir()?,
    };
    let secrets_dir = match args.secrets_dir.clone() {
        Some(dir) => dir,
        None => default_secrets_dir()?,
    };
    info!(
        component_dir = %component_dir.display(),
        secrets_dir = %secrets_dir.display(),
        "wassette stores",
    );

    let data_root = init_data_root()?;
    let resolver = Arc::new(Resolver::new(component_dir));

    // Each component gets a private secret store keyed by its Wassette
    // component id: `store.get(key)` reads that component's secrets file
    // and nothing else.
    let secrets = Arc::new(crate::secrets::SecretsRegistry::new(secrets_dir));

    // `LocalSet` pins the `!Send` session actors to this thread while
    // `Send` work keeps running on the caller's runtime worker pool.
    let local = LocalSet::new();
    local
        .run_until(async move {
            // Resolve provider/layer args (filesystem paths pass through;
            // URIs download into the Wassette component dir; bare ids come
            // from it). The Wassette component id that keys a stage's
            // secret store and `/data` comes from the same arg.
            let mut providers: Vec<Stage> = Vec::with_capacity(args.providers.len());
            for arg in &args.providers {
                let resolved = resolver
                    .resolve(arg)
                    .await
                    .with_context(|| format!("resolving provider `{arg}`"))?;
                let sandbox = Sandbox::load(
                    args.allow_all,
                    &resolved.component_id,
                    &resolved.path,
                    resolver.component_dir(),
                    &secrets,
                )
                .await
                .with_context(|| format!("sandboxing provider `{arg}`"))?;
                let stage = load_stage(
                    &engine,
                    &resolved.path,
                    StageKind::Provider,
                    resolved.component_id,
                    sandbox,
                )?;
                info!(
                    path = %resolved.path.display(),
                    provider = %stage.component_id,
                    sandbox = %stage.sandbox.describe(),
                    "loaded provider component",
                );
                providers.push(stage);
            }
            info!(
                provider_count = providers.len(),
                layer_count = args.layers.len(),
                "chain configuration",
            );

            let mut layers: Vec<Stage> = Vec::with_capacity(args.layers.len());
            for arg in &args.layers {
                let resolved = resolver
                    .resolve(arg)
                    .await
                    .with_context(|| format!("resolving layer `{arg}`"))?;
                let sandbox = Sandbox::load(
                    args.allow_all,
                    &resolved.component_id,
                    &resolved.path,
                    resolver.component_dir(),
                    &secrets,
                )
                .await
                .with_context(|| format!("sandboxing layer `{arg}`"))?;
                layers.push(load_stage(
                    &engine,
                    &resolved.path,
                    StageKind::Layer,
                    resolved.component_id,
                    sandbox,
                )?);
            }
            for (idx, stage) in layers.iter().enumerate() {
                info!(
                    idx,
                    layer = %stage.component_id,
                    sandbox = %stage.sandbox.describe(),
                    "loaded layer",
                );
            }

            let (outbound_tx, outbound_rx) = mpsc::channel(64);
            let factory = Arc::new(SessionFactory::new(
                engine,
                providers,
                layers,
                outbound_tx,
                data_root,
                secrets,
                resolver,
            ));
            let registry = Arc::new(SessionRegistry::new());

            info!("listening for ACP JSON-RPC on stdio");

            bridge::run(factory, registry, outbound_rx).await
        })
        .await
}

/// `$XDG_DATA_HOME/wassette/components` — the same component store
/// `wassette component load` and `wassette run` use.
fn default_component_dir() -> Result<PathBuf> {
    let strategy = etcetera::choose_base_strategy().context("unable to get home directory")?;
    Ok(strategy.data_dir().join("wassette").join("components"))
}

/// `$XDG_CONFIG_HOME/wassette/secrets` — the same secret store
/// `wassette secret set` writes to.
fn default_secrets_dir() -> Result<PathBuf> {
    let strategy = etcetera::choose_base_strategy().context("unable to get home directory")?;
    Ok(strategy.config_dir().join("wassette").join("secrets"))
}

/// Load a wasm component from disk and pair it with its component
/// identity (`namespace:component-name`; see
/// [`install::component_id_for_arg`]). Used for both the provider and
/// each layer stage. Validates the component's import set against the
/// world it was passed as so a layer-shaped wasm passed via `--provider`
/// (or vice versa) is rejected at boot rather than failing later at
/// instantiation with a less obvious error.
fn load_stage(
    engine: &Engine,
    path: &std::path::Path,
    kind: StageKind,
    component_id: String,
    sandbox: Sandbox,
) -> Result<Stage> {
    let component = Component::from_file(engine, path)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("loading {}", path.display()))?;
    validate_imports(engine, &component, kind)
        .with_context(|| format!("validating {}", path.display()))?;
    Ok(Stage {
        component,
        component_id,
        sandbox,
    })
}

/// Semver range of `yosh:acp` this host can speak. Components whose
/// `yosh:acp/*` exports carry a version outside this range are rejected
/// up front. The version itself comes from the in-tree WIT
/// (`package yosh:acp@<v>;`); bump both together.
pub(crate) const EXPECTED_ACP_REQ: &str = "^7.0.0";

/// Concrete version the host's bindgen was generated against. Used for
/// user-facing error messages so a mismatched component sees the exact
/// version the host ships, not just the range.
pub(crate) const HOST_ACP_VERSION: &str = "7.0.0";

/// Inspect a component's exports and decide which `yosh:acp` world it
/// implements:
///
/// - `yosh:acp/provider`: exports `yosh:acp/agent` only.
/// - `yosh:acp/layer`:    exports `yosh:acp/agent` *and* `yosh:acp/client`.
///
/// Any other export shape — wrong package namespace, missing `agent`,
/// or a version incompatible with [`EXPECTED_ACP_REQ`] — is rejected up
/// front so the failure isn't deferred to instantiation.
pub(crate) fn classify_acp_component(engine: &Engine, component: &Component) -> Result<StageKind> {
    let req = semver::VersionReq::parse(EXPECTED_ACP_REQ)
        .expect("EXPECTED_ACP_REQ is a hardcoded valid semver req");
    let ty = component.component_type();
    let mut exports_agent = false;
    let mut exports_client = false;
    for (name, _) in ty.exports(engine) {
        let Some(rest) = name.strip_prefix("yosh:acp/") else {
            continue;
        };
        // Split `<iface>` from optional `@<version>`.
        let (iface, version_str) = match rest.split_once('@') {
            Some((i, v)) => (i, Some(v)),
            None => (rest, None),
        };
        let version_label = version_str.map_or(" (unversioned)".to_string(), |v| format!("@{v}"));
        let parsed = version_str
            .map(semver::Version::parse)
            .transpose()
            .map_err(|e| {
                anyhow::anyhow!(
                    "component exports `yosh:acp/{iface}{version_label}` but the version is \
                     not valid semver: {e}",
                )
            })?;
        let compatible = match parsed {
            Some(v) => req.matches(&v),
            // Unversioned exports are accepted only when the host's
            // requirement also has no version pin.
            None => req == semver::VersionReq::STAR,
        };
        if !compatible {
            anyhow::bail!(
                "component exports `yosh:acp/{iface}{version_label}` but this host requires \
                 `yosh:acp@{EXPECTED_ACP_REQ}` (built against `yosh:acp@{HOST_ACP_VERSION}`); \
                 rebuild the component against the matching WIT definition"
            );
        }
        match iface {
            "agent" => exports_agent = true,
            "client" => exports_client = true,
            _ => {}
        }
    }
    if !exports_agent {
        anyhow::bail!(
            "component does not implement the `yosh:acp/provider` or \
             `yosh:acp/layer` world (host expects `yosh:acp@{EXPECTED_ACP_REQ}`)"
        );
    }
    Ok(if exports_client {
        StageKind::Layer
    } else {
        StageKind::Provider
    })
}

/// Reject components whose detected world (provider vs layer) doesn't
/// match the CLI flag they were passed under. The classification itself
/// also catches non-ACP components and ACP version mismatches; see
/// [`classify_acp_component`].
fn validate_imports(engine: &Engine, component: &Component, kind: StageKind) -> Result<()> {
    let detected = classify_acp_component(engine, component)?;
    match (kind, detected) {
        (StageKind::Provider, StageKind::Layer) => anyhow::bail!(
            "component implements the `yosh:acp/layer` world; \
             pass it via `--layer` rather than `--provider`",
        ),
        (StageKind::Layer, StageKind::Provider) => anyhow::bail!(
            "component implements the `yosh:acp/provider` world; \
             pass it via `--provider` rather than `--layer`",
        ),
        _ => Ok(()),
    }
}

/// Configure the global `tracing` subscriber. **Stderr only** — stdout is
/// the ACP protocol channel. `--log-file` adds an opt-in file layer (ANSI
/// off, so the file stays grep-friendly). Each boot writes to its own
/// timestamped file — e.g. `host.log` becomes `host-<unix-ts>.log` — so
/// runs never stomp each other and old logs stick around for postmortems.
/// `RUST_LOG` takes precedence over the `--log-filter` / `--log-level`
/// flags.
///
/// A subscriber installed by the caller wins; the flags are then inert.
fn init_logging(args: &AcpArgs) -> Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let directive = args.log_filter.clone().unwrap_or_else(|| {
            format!(
                "wassette_acp={level},wasm_stderr=info",
                level = args.log_level.as_str()
            )
        });
        tracing_subscriber::EnvFilter::new(directive)
    });

    let stderr_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let log_path = args.log_file.as_deref().map(timestamped_log_path);
    let file_layer = log_path.as_deref().map(open_log_file).transpose()?;

    if tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init()
        .is_err()
    {
        // Someone (the `wassette` binary, a test harness) already
        // installed a subscriber. Keep going rather than aborting the
        // session.
        return Ok(());
    }

    if let Some(path) = log_path.as_deref() {
        info!(path = %path.display(), "mirroring logs to file");
    }

    Ok(())
}

/// Insert a unix-seconds timestamp before the extension so each boot
/// gets its own file. `logs/host.log` -> `logs/host-1714838400.log`.
fn timestamped_log_path(path: &std::path::Path) -> std::path::PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("host");
    let ext = path.extension().and_then(|s| s.to_str());
    let name = match ext {
        Some(ext) => format!("{stem}-{ts}.{ext}"),
        None => format!("{stem}-{ts}"),
    };
    match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(parent) => parent.join(name),
        None => std::path::PathBuf::from(name),
    }
}

/// Open `path` (creating parent dirs as needed) and wrap it in a non-ANSI
/// `tracing_subscriber` layer suitable for appending logs to.
fn open_log_file<S>(
    path: &std::path::Path,
) -> Result<Box<dyn tracing_subscriber::Layer<S> + Send + Sync>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating log directory {}", parent.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("opening log file {}", path.display()))?;
    // truncate is a no-op on the fresh timestamped path, but keeps
    // behavior sane if the user happens to point at an existing file.

    let subscriber = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(file);

    Ok(Box::new(subscriber))
}

/// Resolve and create the per-app data root, returning its path.
///
/// Each session gets a project- and component-scoped subdirectory
/// underneath this:
///
///   `<data_root>/<project_id>/<component_slug>/`    <-- mounted at /data
///
/// `<project_id>` is a hash of the session's cwd (no path leakage in
/// the dir name); `<component_slug>` is the component identity
/// (`namespace:component-name`) with `:` slugified to `__`. The
/// result: data is naturally siloed per project so an agent can't
/// accidentally leak history between unrelated codebases.
fn init_data_root() -> Result<PathBuf> {
    let data_root = resolve_data_root().context("resolving data root")?;
    std::fs::create_dir_all(&data_root)
        .with_context(|| format!("creating data root {}", data_root.display()))?;
    info!(path = %data_root.display(), "data root");
    Ok(data_root)
}

/// `$XDG_STATE_HOME/wassette/acp`, falling back to
/// `$HOME/.local/state/wassette/acp`. This is the *root*; per-session
/// data dirs are subpaths underneath.
fn resolve_data_root() -> Result<PathBuf> {
    const APP: &str = "wassette";
    const SUBDIR: &str = "acp";
    if let Some(base) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(base).join(APP).join(SUBDIR));
    }
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow::anyhow!("neither XDG_STATE_HOME nor HOME is set"))?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("state")
        .join(APP)
        .join(SUBDIR))
}
