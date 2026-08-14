//! ACP wasmtime host.
//!
//! Loads an ACP agent component and bridges it to the editor over the ACP
//! JSON-RPC wire protocol on stdio. Logs go to stderr; configure verbosity
//! with the `RUST_LOG` environment variable (e.g. `RUST_LOG=host=debug`).
//! Pass `--log-file <path>` to also write logs to a file (useful for
//! debugging when stderr is hidden behind the editor).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
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
mod install;
mod secrets;
mod secrets_impl;
mod state;
mod translate;
mod utils;
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

/// `Host` trait for the layer's *imported* `agent` interface. Since the
/// `with:` clause on the layer bindgen shares this interface with the
/// provider's top-level bindgen (both worlds import `agent` for the
/// `session` resource's destructor), `crate::layer_agent` and
/// `crate::yosh::acp::agent` point to the same module. A single
/// `HostWithStore` impl on `HasSelf<HostState>` therefore satisfies
/// both worlds' linkers.
pub use crate::yosh::acp::agent as layer_agent;
pub use layer_bindings::Layer;

use crate::state::StageKind;
use crate::wasm::{SessionFactory, SessionRegistry, Stage};

#[derive(Parser)]
struct Args {
    /// Optional admin subcommand. When omitted, the host runs an ACP
    /// agent chain (the default). When present, it manages per-component
    /// secrets and exits.
    #[command(subcommand)]
    command: Option<Command>,

    /// Path or WIT name of a terminal ACP **provider** wasm component
    /// (the bottom of a chain). At least one is required.
    ///
    /// May be passed multiple times to load several providers at once.
    /// Every provider is instantiated for each session and its models
    /// are merged into a single **model** selector, labelled by
    /// provider, so the user can pick which model from which provider
    /// backs the session (the rest of the selectors — mode, thinking,
    /// … — follow the provider owning the active model). The same set
    /// of `--layer`s wraps every provider.
    ///
    /// Accepts either a filesystem path (`./my-agent.wasm`) or a
    /// WIT-style package name (`namespace:package[@version]`) — the
    /// latter is resolved against the local component cache, installing
    /// from the registry on first use. `--provider` takes precedence
    /// over the legacy positional argument.
    #[arg(long = "provider", value_name = "PATH|WIT_NAME")]
    providers: Vec<String>,

    /// Legacy positional alias for `--provider`. Retained so existing
    /// single-provider invocations keep working unchanged; used only
    /// when no `--provider` flag is given.
    wasm_path: Option<String>,

    /// Path or WIT name of a **layer** wasm component to wrap the
    /// provider. May be passed multiple times; layers are applied
    /// editor-side → provider-side in the order given (the first
    /// `--layer` is the outermost stage closest to the host).
    /// Same syntax as `--provider`.
    #[arg(long = "layer", value_name = "PATH|WIT_NAME")]
    layers: Vec<String>,

    /// Optional path to a file to mirror logs into. The same events that
    /// go to stderr are appended to this file (no ANSI colors). Useful
    /// when running under an editor that swallows or hides the host's
    /// stderr.
    #[arg(long)]
    log_file: Option<PathBuf>,

    /// Coarse log level. Equivalent to `RUST_LOG=host=<level>`. Use
    /// `--log-filter` for full `tracing` directive syntax (per-target
    /// levels). `RUST_LOG`, if set, takes precedence over both flags.
    #[arg(long, value_enum, default_value_t = LogLevel::Info)]
    log_level: LogLevel,

    /// Full `tracing-subscriber` env-filter directive. Overrides
    /// `--log-level` when set. Example:
    /// `--log-filter "host=debug,agent_client_protocol=trace"`.
    #[arg(long)]
    log_filter: Option<String>,

    /// Which `keyring-core` credential store backs per-component
    /// secrets. Defaults to the platform-native OS store (`native`); pass
    /// `mock` for an in-memory store (tests/CI, empty each run). Applies
    /// to both the host run path and the `secret` subcommands.
    #[arg(long, value_name = "native|mock")]
    keyring_store: Option<crate::secrets::keyring_store::Backend>,

    /// Prefix for keyring `service` names. Each component's secrets live
    /// under `service = "<prefix>:<component-id>"`, keeping this host's
    /// entries from colliding with other apps in a shared OS keychain.
    #[arg(long, value_name = "PREFIX", default_value = crate::secrets::DEFAULT_SERVICE_PREFIX)]
    keyring_service_prefix: String,
}

/// Admin subcommands. Absent = run the ACP host (the default).
#[derive(clap::Subcommand)]
enum Command {
    /// Manage per-component secrets in the keyring store.
    #[command(subcommand)]
    Secret(SecretCommand),
}

#[derive(clap::Subcommand)]
enum SecretCommand {
    /// Store a secret for a component, reading the value from stdin.
    ///
    /// The value is stored under `service = "<prefix>:<component-id>"`,
    /// `user = <key>`, in the store selected by `--keyring-store`. A
    /// single trailing newline is stripped from string values (pass
    /// `--bytes` to store stdin verbatim).
    Set {
        /// Component identity `namespace:component-name`. Registry
        /// components use their WIT `namespace:package` (e.g.
        /// `yosh:ollama-provider`); components loaded from a file use
        /// `local:<filename-stem>` (e.g. `local:ollama_provider`). The
        /// host logs this identity at startup.
        component_id: String,
        /// Secret key name the component looks up via `store.get`.
        key: String,
        /// Store stdin as raw bytes instead of a UTF-8 string (no
        /// newline stripping).
        #[arg(long)]
        bytes: bool,
    },
    /// Delete a component's secret. Succeeds even if it does not exist.
    Delete {
        /// Component identity `namespace:component-name` (e.g.
        /// `local:ollama_provider` or `yosh:ollama-provider`).
        component_id: String,
        /// Secret key name.
        key: String,
    },
    /// Check whether a component's secret is set, without revealing its
    /// value. Exits `0` if the secret exists, `1` if it does not — so it
    /// can be used as a predicate (e.g. `host secret check … && …`).
    Check {
        /// Component identity `namespace:component-name` (e.g.
        /// `local:ollama_provider` or `yosh:ollama-provider`).
        component_id: String,
        /// Secret key name.
        key: String,
    },
}

#[derive(Copy, Clone, Debug, clap::ValueEnum)]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
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

fn main() -> Result<()> {
    // rustls 0.23 links both crypto backends in this dependency graph
    // (wasmtime-wasi-http + oci-client pull `aws-lc-rs`; reqwest/hyper-rustls
    // pull `ring`), so it cannot auto-select a process-level CryptoProvider
    // and panics on the first outbound TLS handshake made by a guest. Install
    // `aws-lc-rs` explicitly to match wasmtime's TLS backend. Idempotent — the
    // `Err` (provider already installed) is safe to ignore.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let args = Args::parse();
    init_logging(&args)?;

    // The keyring store backs both secret resolution (the host run path)
    // and the `secret` admin subcommands, so initialize it once up front
    // for every invocation. Setting the store is cheap and doesn't touch
    // the keychain until a secret is actually read or written.
    let backend = args
        .keyring_store
        .unwrap_or(crate::secrets::keyring_store::Backend::Native);
    crate::secrets::keyring_store::init_default_store(backend)
        .context("initializing keyring store")?;
    info!(?backend, "initialized keyring store");

    // Admin subcommands need neither the wasm engine nor the async
    // runtime: run and exit.
    if let Some(command) = args.command {
        return run_secret_command(command, &args.keyring_service_prefix);
    }

    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_more_async_builtins(true);
    config.wasm_component_model_async_stackful(true);
    config.wasm_features(wasmtime::WasmFeatures::CM_ASYNC, true);
    config.wasm_features(wasmtime::WasmFeatures::CM_MORE_ASYNC_BUILTINS, true);
    config.wasm_features(wasmtime::WasmFeatures::CM_ASYNC_STACKFUL, true);
    let engine = Engine::new(&config)?;

    // Resolve provider args: every `--provider` flag, falling back to
    // the legacy positional argument when none were given. At least one
    // provider is required.
    let provider_args: Vec<String> = if !args.providers.is_empty() {
        args.providers.clone()
    } else {
        args.wasm_path.clone().into_iter().collect()
    };
    if provider_args.is_empty() {
        anyhow::bail!(
            "missing provider wasm component: pass `--provider <path|wit-name>` \
             (repeatable) or as a positional arg"
        );
    }

    let data_root = init_data_root()?;

    // Each component gets a private secret store namespaced by its
    // identity: `store.get(key)` resolves against
    // `service = "<prefix>:<component-id>"` in the keyring store.
    let secrets = Arc::new(crate::secrets::SecretsRegistry::new(
        args.keyring_service_prefix.clone(),
    ));

    // Multi-threaded runtime + `LocalSet`: pins `!Send` session actors to
    // the `block_on` thread while `Send` work runs on the worker pool.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let local = LocalSet::new();
    local.block_on(&runtime, async move {
        // Resolve provider/layer args (filesystem paths pass through;
        // WIT names install-on-miss against the component cache). The
        // component identity (`namespace:component-name`) that keys its
        // secret store and `/data` comes from the same arg.
        let mut providers: Vec<Stage> = Vec::with_capacity(provider_args.len());
        for arg in &provider_args {
            let provider_path = install::resolve(arg)
                .await
                .with_context(|| format!("resolving provider `{arg}`"))?;
            let provider_id = install::component_id_for_arg(arg)
                .with_context(|| format!("deriving component id for `{arg}`"))?;
            let stage = load_stage(&engine, &provider_path, StageKind::Provider, provider_id)?;
            info!(path = %provider_path.display(), provider = %stage.component_id, "loaded provider component");
            providers.push(stage);
        }
        info!(
            provider_count = providers.len(),
            layer_count = args.layers.len(),
            "chain configuration",
        );

        let mut layers: Vec<Stage> = Vec::with_capacity(args.layers.len());
        for arg in &args.layers {
            let p = install::resolve(arg)
                .await
                .with_context(|| format!("resolving layer `{arg}`"))?;
            let layer_id = install::component_id_for_arg(arg)
                .with_context(|| format!("deriving component id for `{arg}`"))?;
            layers.push(load_stage(&engine, &p, StageKind::Layer, layer_id)?);
        }
        for (idx, stage) in layers.iter().enumerate() {
            info!(idx, layer = %stage.component_id, "loaded layer");
        }

        let (outbound_tx, outbound_rx) = mpsc::channel(64);
        let factory = Arc::new(SessionFactory::new(
            engine,
            providers,
            layers,
            outbound_tx,
            data_root,
            secrets,
        ));
        let registry = Arc::new(SessionRegistry::new());

        info!("listening for ACP JSON-RPC on stdio");

        bridge::run(factory, registry, outbound_rx).await
    })
}

/// Run a `secret` admin subcommand against the initialized keyring store.
/// Synchronous: keyring access blocks but needs no async runtime. Secret
/// values are read from stdin and never logged.
fn run_secret_command(command: Command, prefix: &str) -> Result<()> {
    use std::io::Read;
    let Command::Secret(command) = command;
    match command {
        SecretCommand::Set {
            component_id,
            key,
            bytes,
        } => {
            let value = if bytes {
                let mut buf = Vec::new();
                std::io::stdin()
                    .read_to_end(&mut buf)
                    .context("reading secret bytes from stdin")?;
                crate::secrets::SecretValue::Bytes(buf)
            } else {
                let mut s = String::new();
                std::io::stdin()
                    .read_to_string(&mut s)
                    .context("reading secret from stdin")?;
                // Strip a single trailing newline so `echo secret | …`
                // stores `secret`, not `secret\n`.
                if s.ends_with('\n') {
                    s.pop();
                    if s.ends_with('\r') {
                        s.pop();
                    }
                }
                crate::secrets::SecretValue::String(s)
            };
            crate::secrets::set_secret(prefix, &component_id, &key, &value)
                .with_context(|| format!("setting secret for `{component_id}` key `{key}`"))?;
            info!(component_id = %component_id, key = %key, "stored secret");
            eprintln!("stored secret for component `{component_id}`, key `{key}`");
        }
        SecretCommand::Delete { component_id, key } => {
            crate::secrets::delete_secret(prefix, &component_id, &key)
                .with_context(|| format!("deleting secret for `{component_id}` key `{key}`"))?;
            info!(component_id = %component_id, key = %key, "deleted secret");
            eprintln!("deleted secret for component `{component_id}`, key `{key}`");
        }
        SecretCommand::Check { component_id, key } => {
            let present = crate::secrets::check_secret(prefix, &component_id, &key)
                .with_context(|| format!("checking secret for `{component_id}` key `{key}`"))?;
            if present {
                info!(component_id = %component_id, key = %key, "secret is set");
                eprintln!("secret is set for component `{component_id}`, key `{key}`");
            } else {
                info!(component_id = %component_id, key = %key, "secret is not set");
                eprintln!("secret is NOT set for component `{component_id}`, key `{key}`");
                // Signal absence with a non-zero exit so `check` works as a
                // shell predicate. No async runtime or wasm resources are
                // live on this admin path, so exiting directly is safe.
                std::process::exit(1);
            }
        }
    }
    Ok(())
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
) -> Result<Stage> {
    let component = Component::from_file(engine, path)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("loading {}", path.display()))?;
    validate_imports(engine, &component, kind)
        .with_context(|| format!("validating {}", path.display()))?;
    Ok(Stage {
        component,
        component_id,
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

/// Configure the global `tracing` subscriber. Stderr is always wired up;
/// `--log-file` adds an opt-in file layer (ANSI off, so the file stays
/// grep-friendly). Each boot writes to its own timestamped file —
/// e.g. `host.log` becomes `host-<unix-ts>.log` — so runs never
/// stomp each other and old logs stick around for postmortems.
/// `RUST_LOG` takes precedence over the
/// `--log-filter` / `--log-level` flags.
fn init_logging(args: &Args) -> Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let directive = args
            .log_filter
            .clone()
            .unwrap_or_else(|| format!("host={},wasm_stderr=info", args.log_level.as_str()));
        tracing_subscriber::EnvFilter::new(directive)
    });

    let stderr_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let log_path = args.log_file.as_deref().map(timestamped_log_path);
    let file_layer = log_path.as_deref().map(open_log_file).transpose()?;

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();

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

/// `$XDG_STATE_HOME/playground-wasm-acp`, falling back to
/// `$HOME/.local/state/playground-wasm-acp`. This is the *root*; per-session
/// data dirs are subpaths underneath.
fn resolve_data_root() -> Result<PathBuf> {
    const APP: &str = "playground-wasm-acp";
    if let Some(base) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(base).join(APP));
    }
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow::anyhow!("neither XDG_STATE_HOME nor HOME is set"))?;
    Ok(PathBuf::from(home).join(".local").join("state").join(APP))
}
