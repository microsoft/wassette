// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Policy-derived sandboxing for ACP chain stages.
//!
//! Upstream's host handed every guest a blanket-allow `WasiCtx`:
//! inherited network, inherited environment, whatever the process could
//! reach. Wassette already knows how to turn a policy document into a
//! capability set — [`wassette::create_wasi_state_template_from_policy`],
//! the same function the MCP server uses — so ACP stages go through it
//! too. Host-scoped network grants allow filtered `wasi:http` requests,
//! not raw sockets (which cannot enforce host-name restrictions).
//!
//! # Where a stage's policy comes from
//!
//! The resolver selects effective policy during installation, retaining an
//! existing stored policy ahead of source sidecars. Sandboxing consumes only
//! the admitted snapshot's bytes, never a live filename. Local source sidecars
//! are captured during acquisition; explicit local inputs are installed too.
//!
//! A stage with **no** policy gets [`WasiStateTemplate::default`]: no
//! network, no preopens, no environment. Its only filesystem access is
//! the per-session `/data` directory the host preopens for it, which is
//! host-owned rather than policy-granted.
//!
//! # Chain-wide grants
//!
//! One ACP session is one `Store<HostState>` holding *every* stage of a
//! chain, and a store has exactly one `WasiCtx`. Per-stage contexts would
//! mean per-stage stores, which is precisely the design the chain gives
//! up in order to pass resources between stages. So the grants of the
//! stages in a chain are unioned into one [`ChainSandbox`]: the sandbox a
//! layer runs under is its own policy *plus* the policies of the stages
//! it wraps. Practically this means a layer inherits the provider's reach
//! — worth knowing before putting an untrusted layer in front of a
//! network-granted provider.
//!
//! `--allow-all` restores the upstream blanket-allow behaviour for demos.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use policy::PolicyParser;
use tracing::{info, warn};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder};
use wassette::{WasiStateTemplate, create_wasi_state_template_from_policy};

use crate::install::ResolvedComponent;
use crate::secrets::SecretsRegistry;

/// The capabilities one stage is allowed, before they are merged into a
/// chain-wide [`ChainSandbox`].
#[derive(Clone)]
pub enum Sandbox {
    /// `--allow-all`: inherit the host's network and environment. Demo
    /// escape hatch, not a policy.
    AllowAll,
    /// Grants derived from the stage's Wassette policy. A stage with no
    /// policy file gets the default template, which grants nothing.
    Policy(Box<PolicyGrants>),
}

/// A stage's policy-derived grants plus where they came from.
#[derive(Clone)]
pub struct PolicyGrants {
    /// The policy file the grants came from; `None` when the stage has
    /// no policy and is therefore fully denied.
    pub policy_path: Option<PathBuf>,
    /// Include declared grants even when an environment variable is unset.
    #[cfg_attr(not(test), allow(dead_code))]
    pub has_policy_grants: bool,
    pub template: WasiStateTemplate,
}

impl Sandbox {
    /// A shared WASI context would expose these grants to every chain stage.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn has_shared_grants(&self) -> bool {
        match self {
            Sandbox::AllowAll => true,
            Sandbox::Policy(grants) => {
                let t = &grants.template;
                grants.has_policy_grants
                    || t.network_perms.allow_tcp
                    || t.network_perms.allow_udp
                    || t.network_perms.allow_ip_name_lookup
                    || !t.allowed_hosts.is_empty()
                    || !t.preopened_dirs.is_empty()
                    || !t.config_vars.is_empty()
            }
        }
    }

    /// Resolve grants from one admitted snapshot. Relative `fs://` storage
    /// grants still resolve against `component_dir`, not the source directory.
    pub async fn load(
        allow_all: bool,
        resolved: &ResolvedComponent,
        component_dir: &Path,
        secrets: &SecretsRegistry,
    ) -> Result<Self> {
        let component_id = &resolved.component_id;
        let component_secrets = secrets.snapshot(component_id).await?;
        if allow_all {
            warn!(
                component = component_id,
                "--allow-all: stage runs with inherited network and environment, policy ignored"
            );
            return Ok(Sandbox::AllowAll);
        }

        let Some(content) = &resolved.snapshot.policy else {
            info!(
                component = component_id,
                "no policy found: stage gets no network and no filesystem beyond its own /data"
            );
            return Ok(Sandbox::Policy(Box::new(PolicyGrants {
                policy_path: None,
                has_policy_grants: false,
                template: WasiStateTemplate::default(),
            })));
        };

        let policy_path = component_dir.join(format!(
            "{}.policy.yaml",
            resolved.snapshot.receipt.storage_key.as_str()
        ));
        let policy = PolicyParser::parse_bytes(content)
            .with_context(|| format!("parsing captured policy for `{component_id}`"))?;
        let has_policy_grants = policy
            .permissions
            .network
            .as_ref()
            .and_then(|p| p.allow.as_ref())
            .is_some_and(|allow| !allow.is_empty())
            || policy
                .permissions
                .storage
                .as_ref()
                .and_then(|p| p.allow.as_ref())
                .is_some_and(|allow| !allow.is_empty())
            || policy
                .permissions
                .environment
                .as_ref()
                .and_then(|p| p.allow.as_ref())
                .is_some_and(|allow| !allow.is_empty());

        // Secrets are injected as environment variables the same way the
        // MCP path does it, so `wassette secret set <id> KEY=…` reaches
        // an ACP stage through its policy too.
        let host_env: std::collections::HashMap<String, String> = std::env::vars().collect();
        let template = create_wasi_state_template_from_policy(
            &policy,
            component_dir,
            &host_env,
            Some(&component_secrets),
        )
        .with_context(|| format!("building a sandbox from {}", policy_path.display()))?;

        info!(
            component = component_id,
            policy = %policy_path.display(),
            hosts = template.allowed_hosts.len(),
            preopens = template.preopened_dirs.len(),
            "stage sandboxed by policy"
        );
        Ok(Sandbox::Policy(Box::new(PolicyGrants {
            policy_path: Some(policy_path),
            has_policy_grants,
            template,
        })))
    }

    /// Human-readable summary for logs and `--help`-adjacent diagnostics.
    pub fn describe(&self) -> String {
        match self {
            Sandbox::AllowAll => "allow-all (network and environment inherited)".to_string(),
            Sandbox::Policy(grants) => match &grants.policy_path {
                Some(p) => format!("policy {}", p.display()),
                None => "no policy (deny-all)".to_string(),
            },
        }
    }
}

/// Validate captured policy syntax and template construction before admission.
pub(crate) fn validate_policy(bytes: Option<&[u8]>, component_dir: &Path) -> Result<()> {
    if let Some(bytes) = bytes {
        let policy = PolicyParser::parse_bytes(bytes).context("parsing captured ACP policy")?;
        let host_env = std::env::vars().collect();
        create_wasi_state_template_from_policy(&policy, component_dir, &host_env, None)
            .context("validating captured ACP policy template")?;
    }
    Ok(())
}

/// The union of every stage's grants in one chain — what the chain's
/// single `WasiCtx` is built from.
#[derive(Default, Clone)]
pub struct ChainSandbox {
    allow_all: bool,
    env: BTreeMap<String, String>,
    preopens: Vec<Preopen>,
    allowed_hosts: BTreeSet<String>,
}

#[derive(Clone)]
struct Preopen {
    host_path: PathBuf,
    guest_path: String,
    dir_perms: DirPerms,
    file_perms: FilePerms,
}

impl ChainSandbox {
    fn raw_sockets_allowed(&self) -> bool {
        self.allow_all
    }

    /// Union `sandbox` into this chain's grants.
    pub fn merge(&mut self, sandbox: &Sandbox) {
        match sandbox {
            Sandbox::AllowAll => self.allow_all = true,
            Sandbox::Policy(grants) => {
                let t = &grants.template;
                for (k, v) in &t.config_vars {
                    self.env.insert(k.clone(), v.clone());
                }
                for dir in &t.preopened_dirs {
                    self.preopens.push(Preopen {
                        host_path: dir.host_path.clone(),
                        guest_path: dir.guest_path.clone(),
                        dir_perms: dir.dir_perms,
                        file_perms: dir.file_perms,
                    });
                }
                self.allowed_hosts.extend(t.allowed_hosts.iter().cloned());
            }
        }
    }

    /// Hosts outbound HTTP may reach, or `None` under `--allow-all`
    /// (meaning: no filtering).
    pub fn http_allowlist(&self) -> Option<&BTreeSet<String>> {
        if self.allow_all {
            None
        } else {
            Some(&self.allowed_hosts)
        }
    }

    /// Build the chain's `WasiCtx`.
    ///
    /// `data_dir`, when set, is preopened at `/data`. That preopen is
    /// host-owned — it is the session's own scratch space, created per
    /// project and per component — so it exists regardless of policy.
    /// stdout/stderr are always routed into `tracing` because stdout is
    /// the JSON-RPC channel and must never carry guest bytes.
    pub fn build_ctx(&self, data_dir: Option<&Path>) -> Result<WasiCtx> {
        let mut wasi = WasiCtxBuilder::new();
        wasi.stderr(crate::wasi_log::TracingStream::new("stderr"))
            .stdout(crate::wasi_log::TracingStream::new("stdout"));

        if self.raw_sockets_allowed() {
            wasi.inherit_network().inherit_env();
        } else {
            // The HTTP hook checks host grants, but raw TCP/DNS/UDP
            // would bypass it. Keep WASI sockets denied for every
            // policy-scoped chain; `--allow-all` is the explicit escape.
            wasi.allow_tcp(false);
            wasi.allow_udp(false);
            wasi.allow_ip_name_lookup(false);
            for (key, value) in &self.env {
                wasi.env(key, value);
            }
            for dir in &self.preopens {
                wasi.preopened_dir(
                    &dir.host_path,
                    &dir.guest_path,
                    dir.dir_perms,
                    dir.file_perms,
                )
                .map_err(anyhow::Error::from)
                .with_context(|| {
                    format!(
                        "preopening {} at {}",
                        dir.host_path.display(),
                        dir.guest_path
                    )
                })?;
            }
        }

        if let Some(dir) = data_dir {
            wasi.preopened_dir(dir, "/data", DirPerms::all(), FilePerms::all())
                .map_err(anyhow::Error::from)
                .with_context(|| format!("preopening {} at /data", dir.display()))?;
        }

        Ok(wasi.build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants_from(yaml: &str, component_dir: &Path) -> Sandbox {
        let policy = PolicyParser::parse_str(yaml).unwrap();
        let template = create_wasi_state_template_from_policy(
            &policy,
            component_dir,
            &Default::default(),
            None,
        )
        .unwrap();
        Sandbox::Policy(Box::new(PolicyGrants {
            policy_path: None,
            has_policy_grants: false,
            template,
        }))
    }

    #[test]
    fn no_policy_denies_network() {
        assert!(
            !Sandbox::Policy(Box::new(PolicyGrants {
                policy_path: None,
                has_policy_grants: false,
                template: WasiStateTemplate::default(),
            }))
            .has_shared_grants()
        );
        let mut chain = ChainSandbox::default();
        chain.merge(&Sandbox::Policy(Box::new(PolicyGrants {
            policy_path: None,
            has_policy_grants: false,
            template: WasiStateTemplate::default(),
        })));
        assert!(chain.preopens.is_empty());
        assert!(chain.env.is_empty());
        assert_eq!(chain.http_allowlist().map(|h| h.len()), Some(0));
    }

    #[test]
    fn network_policy_grants_http_hosts_without_raw_sockets() {
        let dir = tempfile::tempdir().unwrap();
        let mut chain = ChainSandbox::default();
        let granted = grants_from(
            r#"
version: "1.0"
description: "test"
permissions:
  network:
    allow:
      - host: "api.example.com"
"#,
            dir.path(),
        );
        assert!(granted.has_shared_grants());
        chain.merge(&granted);
        assert!(chain.http_allowlist().unwrap().contains("api.example.com"));
        assert!(!chain.raw_sockets_allowed());
        chain.build_ctx(None).unwrap();
    }

    #[test]
    fn allow_all_disables_filtering() {
        assert!(Sandbox::AllowAll.has_shared_grants());
        let mut chain = ChainSandbox::default();
        chain.merge(&Sandbox::AllowAll);
        assert!(chain.http_allowlist().is_none());
        assert!(chain.raw_sockets_allowed());
    }

    #[test]
    fn injected_environment_secrets_require_shared_grants_opt_in() {
        let mut template = WasiStateTemplate::default();
        template
            .config_vars
            .insert("API_KEY".to_string(), "secret".to_string());
        assert!(
            Sandbox::Policy(Box::new(PolicyGrants {
                policy_path: None,
                has_policy_grants: false,
                template,
            }))
            .has_shared_grants()
        );
    }

    #[test]
    fn chain_grants_are_the_union_of_stage_grants() {
        let dir = tempfile::tempdir().unwrap();
        let mut chain = ChainSandbox::default();
        // Layer: nothing.
        chain.merge(&Sandbox::Policy(Box::new(PolicyGrants {
            policy_path: None,
            has_policy_grants: false,
            template: WasiStateTemplate::default(),
        })));
        // Provider: one host.
        chain.merge(&grants_from(
            r#"
version: "1.0"
description: "test"
permissions:
  network:
    allow:
      - host: "provider.example.com"
"#,
            dir.path(),
        ));
        assert!(
            chain
                .http_allowlist()
                .unwrap()
                .contains("provider.example.com")
        );
    }

    #[test]
    fn storage_policy_becomes_a_preopen() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("workspace")).unwrap();
        let mut chain = ChainSandbox::default();
        let granted = grants_from(
            r#"
version: "1.0"
description: "test"
permissions:
  storage:
    allow:
      - uri: "fs://workspace"
        access: ["read"]
"#,
            dir.path(),
        );
        assert!(granted.has_shared_grants());
        chain.merge(&granted);
        assert_eq!(chain.preopens.len(), 1);
        assert_eq!(chain.preopens[0].guest_path, "workspace");
        // Read-only: no write bit.
        assert!(!chain.preopens[0].file_perms.contains(FilePerms::WRITE));
        // The context builds against the real directory.
        chain.build_ctx(None).unwrap();
    }

    #[test]
    fn data_dir_is_preopened_without_any_policy() {
        let dir = tempfile::tempdir().unwrap();
        let chain = ChainSandbox::default();
        chain.build_ctx(Some(dir.path())).unwrap();
    }

    #[tokio::test]
    async fn captured_policy_is_used_after_live_policy_replacement() {
        let store = tempfile::tempdir().unwrap();
        let beside = tempfile::tempdir().unwrap();
        let wasm = beside.path().join("agent.wasm");
        std::fs::write(
            &wasm,
            crate::install::named_fixture("semantic:agent", false),
        )
        .unwrap();
        std::fs::write(
            beside.path().join("agent.policy.yaml"),
            r#"
version: "1.0"
description: "test"
permissions:
  network:
    allow:
      - host: "beside.example.com"
"#,
        )
        .unwrap();
        let secrets = SecretsRegistry::new(store.path());
        let resolved = resolve(store.path(), &wasm, &secrets).await;
        std::fs::write(store.path().join("agent.policy.yaml"), "not: [valid").unwrap();
        std::fs::write(beside.path().join("agent.policy.yaml"), "not: [valid").unwrap();
        let sandbox = Sandbox::load(false, &resolved, store.path(), &secrets)
            .await
            .unwrap();
        assert!(sandbox.describe().contains("agent.policy.yaml"));
        let mut chain = ChainSandbox::default();
        chain.merge(&sandbox);
        assert!(
            chain
                .http_allowlist()
                .unwrap()
                .contains("beside.example.com")
        );
    }

    #[tokio::test]
    async fn the_component_store_wins_over_a_colocated_policy() {
        let store = tempfile::tempdir().unwrap();
        let beside = tempfile::tempdir().unwrap();
        let wasm = beside.path().join("agent.wasm");
        std::fs::write(&wasm, crate::install::named_fixture("agent", false)).unwrap();
        let secrets = SecretsRegistry::new(store.path());
        for host in ["store.example.com", "beside.example.com"] {
            std::fs::write(
                beside.path().join("agent.policy.yaml"),
                format!(
                    r#"
version: "1.0"
description: "test"
permissions:
  network:
    allow:
      - host: "{host}"
"#
                ),
            )
            .unwrap();
            resolve(store.path(), &wasm, &secrets).await;
        }
        let resolved = resolve(store.path(), &wasm, &secrets).await;
        let sandbox = Sandbox::load(false, &resolved, store.path(), &secrets)
            .await
            .unwrap();
        let mut chain = ChainSandbox::default();
        chain.merge(&sandbox);
        assert!(
            chain
                .http_allowlist()
                .unwrap()
                .contains("store.example.com")
        );
    }

    #[tokio::test]
    async fn a_stage_without_a_policy_is_denied_everything() {
        let store = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let wasm = source.path().join("agent.wasm");
        std::fs::write(&wasm, crate::install::named_fixture("agent", false)).unwrap();
        let secrets = SecretsRegistry::new(store.path());
        let resolved = resolve(store.path(), &wasm, &secrets).await;
        let sandbox = Sandbox::load(false, &resolved, store.path(), &secrets)
            .await
            .unwrap();
        assert_eq!(sandbox.describe(), "no policy (deny-all)");
        let mut chain = ChainSandbox::default();
        chain.merge(&sandbox);
        assert!(chain.http_allowlist().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unset_environment_grant_still_requires_opt_in() {
        let store = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let wasm = source.path().join("agent.wasm");
        std::fs::write(&wasm, crate::install::named_fixture("agent", false)).unwrap();
        std::fs::write(
            source.path().join("agent.policy.yaml"),
            "version: '1.0'\npermissions:\n  environment:\n    allow:\n      - key: WASSETTE_TEST_MISSING_ENV_770\n",
        )
        .unwrap();
        let secrets = SecretsRegistry::new(store.path());
        let resolved = resolve(store.path(), &wasm, &secrets).await;
        let sandbox = Sandbox::load(false, &resolved, store.path(), &secrets)
            .await
            .unwrap();
        assert!(sandbox.has_shared_grants());
    }

    #[tokio::test]
    async fn malformed_secrets_fail_even_without_policy_or_with_allow_all() {
        let store = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let wasm = source.path().join("agent.wasm");
        std::fs::write(&wasm, crate::install::named_fixture("agent", false)).unwrap();
        let secrets = SecretsRegistry::new(store.path());
        let resolved = resolve(store.path(), &wasm, &secrets).await;
        std::fs::write(store.path().join("agent.yaml"), "TOKEN: orphan\n").unwrap();
        for allow_all in [false, true] {
            assert!(
                Sandbox::load(allow_all, &resolved, store.path(), &secrets)
                    .await
                    .is_err()
            );
        }
    }

    async fn resolve(store: &Path, wasm: &Path, secrets: &SecretsRegistry) -> ResolvedComponent {
        let resolved = crate::install::Resolver::new(store)
            .unwrap()
            .install_validated(wasm.to_str().unwrap(), None, &wasmtime::Engine::default())
            .await
            .unwrap();
        secrets
            .register(resolved.snapshot.receipt.secret_binding().unwrap())
            .unwrap();
        resolved
    }
}
