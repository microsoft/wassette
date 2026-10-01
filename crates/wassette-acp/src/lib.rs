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
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use wasmtime::{Config, Engine};

mod bridge;
mod client_impl;
mod data;
mod generation;
mod group;
mod http_policy;
mod install;
mod sandbox;
mod secrets;
mod secrets_impl;
mod state;
mod tool_broker;
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
// defined exactly once under `crate::wassette::acp::*`, and a single
// set of `Host` trait impls on `HostState` satisfies both linkers.
//
// Bindgen flips imports/exports from the host's perspective: imported
// interfaces (`client` for both worlds, plus `agent` for `layer`) become
// `Host` traits we implement; exported interfaces (`agent`) become
// callable methods on the wrapper struct.
wasmtime::component::bindgen!({
    path: ["../../wit/component-generation", "wit/acp"],
    world: "wassette:acp/provider@7.0.0",
    imports: { default: async },
    exports: { default: async },
    with: {
        "wassette:component-generation/builder@0.1.0": crate::generation::builder,
    },
});

mod layer_bindings {
    // The layer bindgen lives in its own module so its generated
    // `exports` module and `Layer` world wrapper don't collide with
    // the provider's. Interface types are shared via `with:` so every
    // WIT record/variant is still defined exactly once at the crate
    // root, and a single set of `Host` impls on `HostState` satisfies
    // both linkers.
    wasmtime::component::bindgen!({
        path: ["../../wit/component-generation", "wit/acp"],
        world: "wassette:acp/layer@7.0.0",
        imports: { default: async },
        exports: { default: async },
        with: {
            "wassette:acp/errors": crate::wassette::acp::errors,
            "wassette:acp/content": crate::wassette::acp::content,
            "wassette:acp/init": crate::wassette::acp::init,
            "wassette:acp/sessions": crate::wassette::acp::sessions,
            "wassette:acp/prompts": crate::wassette::acp::prompts,
            "wassette:acp/tools": crate::wassette::acp::tools,
            "wassette:acp/terminals": crate::wassette::acp::terminals,
            "wassette:acp/filesystem": crate::wassette::acp::filesystem,
            "wassette:acp/agent": crate::wassette::acp::agent,
            "wassette:acp/client": crate::wassette::acp::client,
            "wassette:component-tools/tools@0.1.0": crate::wassette::component_tools::tools,
            "wassette:component-generation/builder@0.1.0": crate::wassette::component_generation::builder,
            "wasmcloud:secrets/store@2.1.0": crate::wasmcloud::secrets::store,
            "wasmcloud:secrets/reveal@2.1.0": crate::wasmcloud::secrets::reveal,
        },
    });
}

pub use layer_bindings::Layer;

use crate::install::{AcpLocalValidator, Resolver};
use crate::sandbox::Sandbox;
use crate::state::StageKind;
use crate::wasm::{SessionFactory, SessionRegistry, Stage};
/// `Host` trait for the layer's *imported* `agent` interface. Since the
/// `with:` clause on the layer bindgen shares this interface with the
/// provider's top-level bindgen (both worlds import `agent` for the
/// `session` resource's destructor), `crate::layer_agent` and
/// `crate::wassette::acp::agent` point to the same module. A single
/// `HostWithStore` impl on `HasSelf<HostState>` therefore satisfies
/// both worlds' linkers.
pub use crate::wassette::acp::agent as layer_agent;

/// Arguments for `wassette acp`: run Wassette as an ACP agent whose brain
/// is a WebAssembly component.
#[derive(clap::Args, Debug)]
pub struct AcpArgs {
    /// Path, URI, or component id of a terminal ACP **provider** wasm
    /// component (the bottom of a chain). Repeat to make multiple providers
    /// available in the session's model selector. At least one is required.
    ///
    /// Accepts anything `wassette component load` does — a filesystem
    /// path (`./my-agent.wasm`), an `oci://` reference, or an `https://`
    /// URL — plus the id of a component already in the component
    /// directory. Explicit local paths and downloads are transactionally
    /// installed; later selections use the embedded semantic component id.
    #[arg(long = "provider", value_name = "PATH|URI|COMPONENT_ID")]
    pub providers: Vec<String>,

    /// Path, URI, or component id of a **layer** wasm component to wrap
    /// the provider. May be passed multiple times; layers are applied
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
    /// By default each provider and layer is sandboxed from the effective
    /// policy captured with its installation receipt, exactly as
    /// `wassette component load` + `wassette policy attach` set it up for
    /// MCP. **A component with no policy therefore gets no network and no
    /// filesystem access beyond its own per-session `/data` directory
    /// (not mounted for a layered chain without --allow-shared-grants).**
    /// Grant reach with a policy — `permissions.network.allow` for hosts,
    /// `permissions.storage.allow` for paths, `permissions.environment.allow`
    /// for environment variables — or pass this flag to skip policy
    /// enforcement entirely. Intended for demos and local debugging.
    #[arg(long)]
    pub allow_all: bool,

    /// Permit layered chains with policy grants, stored secrets or --allow-all,
    /// and mount the provider's persistent /data directory for the chain.
    /// Stages share one WASI context, and concurrent callbacks may be
    /// attributed to the wrong stage (including secret lookups). Does not
    /// isolate stages; use only with mutually trusted components.
    /// Nonempty legacy /data directories without a matching receipt-bound
    /// ownership record remain protected, regardless of this flag.
    #[arg(long)]
    pub allow_shared_grants: bool,

    /// Optional path to a file to mirror logs into. The same events that
    /// go to stderr are written to a timestamped file (created or
    /// truncated for each run, no ANSI colors). Useful
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

    /// Expose tools belonging to this installed semantic component id to the
    /// ACP provider. Repeat to expose more than one component.
    #[arg(long = "tool", value_name = "COMPONENT_ID")]
    pub tools: Vec<String>,

    /// Directory scanned for locally built components.
    #[arg(long)]
    pub local_component_dir: Option<PathBuf>,

    /// Local component discovery: off, startup, or watch.
    #[arg(long, value_enum)]
    pub local_components: Option<AcpLocalComponentsMode>,

    /// Trusted operator generation profile. Never inferred from a guest request.
    #[cfg(feature = "component-generation")]
    #[arg(long, value_name = "PATH")]
    pub generation_config: Option<PathBuf>,
}

/// Coarse verbosity for the host's own logs.
#[derive(Copy, Clone, Debug, clap::ValueEnum)]
pub enum LogLevel {
    /// Verbose host diagnostics (does not enable JSON-RPC wire payload logs).
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

/// ACP CLI choice for local component discovery.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum AcpLocalComponentsMode {
    Off,
    Startup,
    Watch,
}

fn acp_engine() -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_more_async_builtins(true);
    config.wasm_component_model_async_stackful(true);
    config.wasm_component_model_implements(true);
    config.wasm_features(wasmtime::WasmFeatures::CM_ASYNC, true);
    config.wasm_features(wasmtime::WasmFeatures::CM_MORE_ASYNC_BUILTINS, true);
    config.wasm_features(wasmtime::WasmFeatures::CM_ASYNC_STACKFUL, true);
    Ok(Engine::new(&config)?)
}

/// Validate generated ACP layers with the same engine and policy checks as ACP.
///
/// This compiles and checks exports; it does not instantiate a guest, establish
/// full host-link compatibility, select a layer, or change a running chain.
#[cfg(feature = "component-generation")]
pub fn generation_validator(
    component_dir: PathBuf,
) -> Result<Arc<dyn ::wassette::local_source::LocalValidator>> {
    Ok(Arc::new(AcpLocalValidator::new(
        acp_engine()?,
        component_dir,
    )))
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
pub async fn run(
    args: AcpArgs,
    local_source_config: ::wassette::local_source::LocalSourceConfig,
) -> Result<()> {
    eprintln!("Notice: wassette acp is experimental and may change or be removed.");
    if args.providers.is_empty() {
        anyhow::bail!("wassette acp requires at least one --provider");
    }
    // rustls 0.23 links both crypto backends in this dependency graph
    // (wasmtime-wasi-http + oci-client pull `aws-lc-rs`; reqwest/hyper-rustls
    // pull `ring`), so it cannot auto-select a process-level CryptoProvider
    // and panics on the first outbound TLS handshake made by a guest. Install
    // `aws-lc-rs` explicitly to match wasmtime's TLS backend. Idempotent — the
    // `Err` (provider already installed) is safe to ignore.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    init_logging(&args)?;

    let engine = acp_engine()?;

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
    let lifecycle_config = ::wassette::LifecycleManager::builder(component_dir.clone())
        .with_secrets_dir(secrets_dir)
        .build_config()?;
    let resolver = Arc::new(Resolver::with_config(lifecycle_config.clone()));
    let tool_manager = Arc::new(::wassette::LifecycleManager::from_config(lifecycle_config).await?);
    #[cfg(feature = "component-generation")]
    if let Some(path) = &args.generation_config {
        let service = ::wassette::generation::GenerationConfig::read(path)?
            .into_service()?
            .with_validator(Arc::new(AcpLocalValidator::new(
                engine.clone(),
                component_dir.clone(),
            )));
        tool_manager.enable_generation(service)?;
    }
    let local_source = if local_source_config.mode == ::wassette::local_source::LocalMode::Off {
        None
    } else {
        let service = ::wassette::local_source::LocalSourceService::new(
            tool_manager.clone(),
            local_source_config.clone(),
        )?
        .with_validator(Arc::new(AcpLocalValidator::new(
            engine.clone(),
            component_dir.clone(),
        )));
        let report = service.reconcile_once(false).await?;
        if report.has_unresolved() {
            tracing::warn!(
                ?report,
                "Some local ACP component sources could not be installed"
            );
        }
        Some(Arc::new(service))
    };

    let secrets = Arc::new(crate::secrets::SecretsRegistry::new(resolver.secrets_dir()));

    // `LocalSet` pins the `!Send` session actors to this thread while
    // `Send` work keeps running on the caller's runtime worker pool.
    let local = LocalSet::new();
    local
        .run_until(async move {
            let mut providers: Vec<Stage> = Vec::with_capacity(args.providers.len());
            for arg in &args.providers {
                let resolved = resolver
                    .resolve_validated(arg, None, &engine, Some(StageKind::Provider))
                    .await
                    .with_context(|| format!("resolving provider `{arg}`"))?;
                if providers.iter().any(|provider| provider.component_id == resolved.component_id) {
                    anyhow::bail!("provider `{}` was selected more than once", resolved.component_id);
                }
                secrets.register(resolved.snapshot.receipt.secret_binding()?)?;
                let sandbox = Sandbox::load(
                    args.allow_all,
                    &resolved,
                    resolver.component_dir(),
                    &secrets,
                )
                .await
                .with_context(|| format!("sandboxing provider `{arg}`"))?;
                let stage = load_stage(&resolved, sandbox)?;
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
                    .resolve_validated(arg, None, &engine, Some(StageKind::Layer))
                    .await
                    .with_context(|| format!("resolving layer `{arg}`"))?;
                secrets.register(resolved.snapshot.receipt.secret_binding()?)?;
                let sandbox = Sandbox::load(
                    args.allow_all,
                    &resolved,
                    resolver.component_dir(),
                    &secrets,
                )
                .await
                .with_context(|| format!("sandboxing layer `{arg}`"))?;
                layers.push(load_stage(&resolved, sandbox)?);
            }
            for (idx, stage) in layers.iter().enumerate() {
                info!(
                    idx,
                    layer = %stage.component_id,
                    sandbox = %stage.sandbox.describe(),
                    "loaded layer",
                );
            }
            require_shared_grants_opt_in(
                !layers.is_empty(),
                args.allow_shared_grants,
                providers
                    .iter()
                    .chain(&layers)
                    .map(|stage| (stage.component_id.as_str(), &stage.sandbox)),
                &secrets,
            )
            .await?;
            #[cfg(feature = "component-generation")]
            let generation_enabled = args.generation_config.is_some();
            #[cfg(not(feature = "component-generation"))]
            let generation_enabled = false;
            if !layers.is_empty()
                && (!args.tools.is_empty() || generation_enabled)
                && !args.allow_shared_grants
            {
                anyhow::bail!(
                    "Layered chains with ordinary tools or generation require --allow-shared-grants; \
                     layers can intercept permissions and share the provider's store"
                );
            }
            let tool_broker = Arc::new(tool_broker::ToolBroker::new(
                tool_manager,
                args.tools.iter().cloned(),
                providers
                    .iter()
                    .chain(&layers)
                    .map(|stage| stage.component_id.clone()),
            ));
            let local_cancel = CancellationToken::new();
            let mut local_tasks = tokio::task::JoinSet::new();
            if let Some(service) = &local_source
                && local_source_config.mode == ::wassette::local_source::LocalMode::Watch
            {
                let service = service.clone();
                let cancel = local_cancel.clone();
                local_tasks.spawn(async move {
                    if let Err(error) = service.watch(cancel).await {
                        tracing::error!(error = %error, "ACP local component watch stopped");
                    }
                });
            }

            let (outbound_tx, outbound_rx) = mpsc::channel(64);
            let factory = Arc::new(
                SessionFactory::new(
                    engine,
                    providers,
                    layers,
                    outbound_tx,
                    data_root,
                    secrets,
                    resolver,
                    tool_broker,
                )
                .with_shared_provider_data(args.allow_shared_grants),
            );
            let registry = Arc::new(SessionRegistry::new());

            info!("listening for ACP JSON-RPC on stdio");

            let result = bridge::run(factory, registry, outbound_rx).await;
            local_cancel.cancel();
            local_tasks.abort_all();
            while local_tasks.join_next().await.is_some() {}
            result
        })
        .await
}

async fn require_shared_grants_opt_in<'a>(
    has_layers: bool,
    allow_shared_grants: bool,
    stages: impl IntoIterator<Item = (&'a str, &'a Sandbox)>,
    secrets: &secrets::SecretsRegistry,
) -> Result<()> {
    if !has_layers || allow_shared_grants {
        return Ok(());
    }
    for (component_id, sandbox) in stages {
        if sandbox.has_shared_grants() || secrets.has_secrets(component_id).await? {
            anyhow::bail!(
                "layered chains with policy grants or stored secrets share one WASI context, \
                 and concurrent callbacks may be attributed to the wrong stage (including secret lookups); \
                 pass --allow-shared-grants only if all stages are mutually trusted"
            );
        }
    }
    Ok(())
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

#[cfg(test)]
mod tool_args_tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        acp: AcpArgs,
    }

    #[test]
    fn tool_exposure_is_explicit_and_repeatable() {
        let parsed = TestCli::try_parse_from([
            "test",
            "--provider",
            "provider",
            "--tool",
            "filesystem-rs",
            "--tool",
            "time-server",
        ])
        .unwrap();
        assert_eq!(parsed.acp.tools, ["filesystem-rs", "time-server"]);
    }

    #[test]
    fn tools_are_not_exposed_by_default() {
        let parsed = TestCli::try_parse_from(["test", "--provider", "provider"]).unwrap();
        assert!(parsed.acp.tools.is_empty());
    }

    #[test]
    fn local_discovery_arguments_are_parsed() {
        let parsed = TestCli::try_parse_from([
            "test",
            "--provider",
            "provider",
            "--local-component-dir",
            "target/local-components",
            "--local-components",
            "watch",
        ])
        .unwrap();
        assert_eq!(
            parsed.acp.local_component_dir,
            Some(PathBuf::from("target/local-components"))
        );
        assert_eq!(
            parsed.acp.local_components,
            Some(AcpLocalComponentsMode::Watch)
        );
    }

    #[test]
    fn local_discovery_is_not_enabled_by_default() {
        let parsed = TestCli::try_parse_from(["test", "--provider", "provider"]).unwrap();
        assert!(parsed.acp.local_component_dir.is_none());
        assert!(parsed.acp.local_components.is_none());
    }
}

/// Pin a validated component and its admitted policy/identity for the stage's
/// entire lifetime. No live Wasm or policy path is reopened here.
fn load_stage(resolved: &install::ResolvedComponent, sandbox: Sandbox) -> Result<Stage> {
    Ok(Stage {
        component: resolved.component.clone(),
        component_id: resolved.component_id.clone(),
        storage_key: resolved.snapshot.receipt.storage_key.clone(),
        snapshot: resolved.snapshot.clone(),
        sandbox,
    })
}

/// Semver range of `wassette:acp` this host can speak. Components whose
/// `wassette:acp/*` exports carry a version outside this range are rejected
/// up front. The version itself comes from the in-tree WIT
/// (`package wassette:acp@<v>;`); bump both together.
pub(crate) const EXPECTED_ACP_REQ: &str = "^7.0.0";

/// Concrete version the host's bindgen was generated against. Used for
/// user-facing error messages so a mismatched component sees the exact
/// version the host ships, not just the range.
pub(crate) const HOST_ACP_VERSION: &str = "7.0.0";

/// Inspect a component's exports and decide which `wassette:acp` world it
/// implements:
///
/// - `wassette:acp/provider`: exports `wassette:acp/agent` only.
/// - `wassette:acp/layer`:    exports `wassette:acp/agent` *and* `wassette:acp/client`.
///
/// Any other export shape — wrong package namespace, missing `agent`,
/// or a version incompatible with [`EXPECTED_ACP_REQ`] — is rejected up
/// front so the failure isn't deferred to instantiation.
pub(crate) fn classify_acp_component(
    inspection: &::wassette::ArtifactInspection,
) -> Result<StageKind> {
    let kind = match inspection.shape {
        ::wassette::ArtifactShape::AcpProvider => StageKind::Provider,
        ::wassette::ArtifactShape::AcpLayer => StageKind::Layer,
        _ => anyhow::bail!(
            "component does not implement the `wassette:acp/provider` or \
             `wassette:acp/layer` world (host expects `wassette:acp@{EXPECTED_ACP_REQ}`)"
        ),
    };
    let req = semver::VersionReq::parse(EXPECTED_ACP_REQ)
        .expect("EXPECTED_ACP_REQ is a hardcoded valid semver req");
    for name in &inspection.acp_exports {
        let Some(rest) = name.strip_prefix("wassette:acp/") else {
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
                    "component exports `wassette:acp/{iface}{version_label}` but the version is \
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
                "component exports `wassette:acp/{iface}{version_label}` but this host requires \
                 `wassette:acp@{EXPECTED_ACP_REQ}` (built against `wassette:acp@{HOST_ACP_VERSION}`); \
                 rebuild the component against the matching WIT definition"
            );
        }
    }
    Ok(kind)
}

/// Reject components whose detected world (provider vs layer) doesn't
/// match the CLI flag they were passed under. The classification itself
/// also catches non-ACP components and ACP version mismatches; see
/// [`classify_acp_component`].
pub(crate) fn validate_stage(
    inspection: &::wassette::ArtifactInspection,
    kind: StageKind,
) -> Result<()> {
    let detected = classify_acp_component(inspection)?;
    match (kind, detected) {
        (StageKind::Provider, StageKind::Layer) => anyhow::bail!(
            "component implements the `wassette:acp/layer` world; \
             pass it via `--layer` rather than `--provider`",
        ),
        (StageKind::Layer, StageKind::Provider) => anyhow::bail!(
            "component implements the `wassette:acp/provider` world; \
             pass it via `--provider` rather than `--layer`",
        ),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod classification_tests {
    use ::wassette::{ArtifactInspection, ArtifactShape, IdentityError, UnsupportedArtifact};

    use super::*;

    fn inspection(shape: ArtifactShape, export: &str) -> ArtifactInspection {
        ArtifactInspection {
            identity: Err(IdentityError::Missing),
            shape,
            acp_exports: vec![export.to_owned()],
        }
    }

    #[test]
    fn shared_shapes_retain_acp_version_and_stage_checks() {
        let provider = inspection(ArtifactShape::AcpProvider, "wassette:acp/agent@7.0.0");
        assert!(matches!(
            classify_acp_component(&provider).unwrap(),
            StageKind::Provider
        ));
        assert!(validate_stage(&provider, StageKind::Provider).is_ok());
        assert!(validate_stage(&provider, StageKind::Layer).is_err());
        let layer = inspection(ArtifactShape::AcpLayer, "wassette:acp/agent@7.0.0");
        assert!(matches!(
            classify_acp_component(&layer).unwrap(),
            StageKind::Layer
        ));
        assert!(validate_stage(&layer, StageKind::Provider).is_err());
        for export in [
            "wassette:acp/agent",
            "wassette:acp/agent@6.0.0",
            "wassette:acp/agent@invalid",
        ] {
            assert!(
                classify_acp_component(&inspection(ArtifactShape::AcpProvider, export)).is_err()
            );
        }
        assert!(classify_acp_component(&inspection(ArtifactShape::ToolCandidate, "")).is_err());
        assert!(
            classify_acp_component(&inspection(
                ArtifactShape::Unsupported(UnsupportedArtifact::CoreModule),
                ""
            ))
            .is_err()
        );
    }

    #[cfg(feature = "component-generation")]
    #[test]
    fn generation_validator_compiles_without_starting_or_installing_a_layer() {
        let root = tempfile::tempdir().unwrap();
        let component_dir = root.path().join("not-created");
        let validator = generation_validator(component_dir.clone()).unwrap();
        let wasm = wat::parse_str(
            r#"(component $generated-layer
                (core module $m (func $start unreachable) (start $start))
                (core instance $i (instantiate $m))
                (instance $empty)
                (export "wassette:acp/agent@7.0.0" (instance $empty))
                (export "wassette:acp/client@7.0.0" (instance $empty)))"#,
        )
        .unwrap();
        let inspection = ::wassette::inspect_artifact(&wasm).unwrap();
        assert!(matches!(inspection.shape, ArtifactShape::AcpLayer));
        assert!(matches!(
            validator.validate(&wasm, &inspection, None).unwrap(),
            ::wassette::store::ValidationEvidence::AcpCompiledAndExportChecked { .. }
        ));
        assert!(!component_dir.exists());
        let ordinary = wat::parse_str("(component $ordinary)").unwrap();
        assert!(
            validator
                .validate(
                    &ordinary,
                    &::wassette::inspect_artifact(&ordinary).unwrap(),
                    None,
                )
                .is_err()
        );
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

/// Use the platform's state directory (or its data directory where no
/// state directory exists). This is the *root*; per-session data dirs
/// are subpaths underneath.
fn resolve_data_root() -> Result<PathBuf> {
    data_root_from_strategy(etcetera::choose_base_strategy())
}

fn data_root_from_strategy(
    strategy: std::result::Result<impl BaseStrategy, etcetera::HomeDirError>,
) -> Result<PathBuf> {
    let strategy =
        strategy.context("unable to determine ACP data root: no home directory found")?;
    let base = strategy.state_dir().unwrap_or_else(|| strategy.data_dir());
    Ok(base.join("wassette").join("acp"))
}

#[cfg(test)]
mod data_root_tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn no_home_uses_etcetera_state_directory() {
        const CHILD: &str = "WASSETTE_ACP_TEST_NO_HOME";
        if std::env::var_os(CHILD).is_some() {
            assert!(std::env::var_os("HOME").is_none());
            assert!(std::env::var_os("XDG_STATE_HOME").is_none());
            let state_dir = etcetera::choose_base_strategy()
                .unwrap()
                .state_dir()
                .unwrap();
            assert_eq!(resolve_data_root().unwrap(), state_dir.join("wassette/acp"));
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("data_root_tests::no_home_uses_etcetera_state_directory")
            .arg("--nocapture")
            .env_remove("HOME")
            .env_remove("XDG_STATE_HOME")
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child test failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn strategy_failure_has_clear_error() {
        let error = data_root_from_strategy(Err::<etcetera::base_strategy::Xdg, _>(
            etcetera::HomeDirError,
        ))
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unable to determine ACP data root: no home directory found")
        );
    }
}

#[cfg(test)]
mod shared_grants_tests {
    use super::*;

    #[tokio::test]
    async fn unprivileged_layered_chains_run_without_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = secrets::SecretsRegistry::new(dir.path());
        secrets.register(secrets::test_binding("provider")).unwrap();
        secrets.register(secrets::test_binding("layer")).unwrap();
        let denied = Sandbox::Policy(Box::new(crate::sandbox::PolicyGrants {
            policy_path: None,
            has_policy_grants: false,
            template: ::wassette::WasiStateTemplate::default(),
        }));
        assert!(
            require_shared_grants_opt_in(
                true,
                false,
                [("provider", &denied), ("layer", &denied)],
                &secrets
            )
            .await
            .is_ok()
        );
        for (has_layers, opt_in, expected_ok) in [
            (true, false, false),
            (true, true, true),
            (false, false, true),
        ] {
            assert_eq!(
                require_shared_grants_opt_in(
                    has_layers,
                    opt_in,
                    [("provider", &Sandbox::AllowAll), ("layer", &denied)],
                    &secrets
                )
                .await
                .is_ok(),
                expected_ok,
            );
        }
    }

    #[tokio::test]
    async fn invalid_secrets_store_cannot_bypass_layer_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("layer.yaml"), "not: [valid").unwrap();
        let secrets = secrets::SecretsRegistry::new(dir.path());
        secrets.register(secrets::test_binding("provider")).unwrap();
        secrets.register(secrets::test_binding("layer")).unwrap();
        let denied = Sandbox::Policy(Box::new(crate::sandbox::PolicyGrants {
            policy_path: None,
            has_policy_grants: false,
            template: ::wassette::WasiStateTemplate::default(),
        }));
        let err = require_shared_grants_opt_in(
            true,
            false,
            [("provider", &denied), ("layer", &denied)],
            &secrets,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("checking secrets for component `layer`")
        );
    }
}
