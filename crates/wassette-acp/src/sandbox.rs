// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Policy-derived sandboxing for ACP chain stages.
//!
//! Upstream's host handed every guest a blanket-allow `WasiCtx`:
//! inherited network, inherited environment, whatever the process could
//! reach. Wassette already knows how to turn a policy document into a
//! capability set — [`wassette::create_wasi_state_template_from_policy`],
//! the same function the MCP server uses — so ACP stages go through it
//! too. An ACP agent component therefore reaches exactly the hosts,
//! paths and environment variables its policy grants and nothing else.
//!
//! # Where a stage's policy comes from
//!
//! Same convention as the rest of Wassette: `<component-id>.policy.yaml`,
//! looked up first in the component directory (where `wassette policy
//! attach` and `wassette component load` put it) and then next to the
//! `.wasm` itself, so a `--provider ./target/…/agent.wasm` picks up a
//! policy sitting beside it without being installed first.
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

use crate::secrets::SecretsRegistry;

/// Filename holding a component's policy, e.g. `agent.policy.yaml`.
fn policy_file_name(component_id: &str) -> String {
    format!("{component_id}.policy.yaml")
}

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
    pub template: WasiStateTemplate,
}

impl Sandbox {
    /// Resolve the sandbox for one stage.
    ///
    /// `component_dir` is the Wassette component directory: it is both
    /// where policies are looked up and the root that relative `fs://`
    /// storage grants are resolved against (matching
    /// [`wassette::create_wasi_state_template_from_policy`]'s contract in
    /// the MCP server).
    pub async fn load(
        allow_all: bool,
        component_id: &str,
        wasm_path: &Path,
        component_dir: &Path,
        secrets: &SecretsRegistry,
    ) -> Result<Self> {
        if allow_all {
            warn!(
                component = component_id,
                "--allow-all: stage runs with inherited network and environment, policy ignored"
            );
            return Ok(Sandbox::AllowAll);
        }

        let Some(policy_path) = find_policy(component_id, wasm_path, component_dir) else {
            info!(
                component = component_id,
                "no policy found: stage gets no network and no filesystem beyond its own /data"
            );
            return Ok(Sandbox::Policy(Box::new(PolicyGrants {
                policy_path: None,
                template: WasiStateTemplate::default(),
            })));
        };

        let content = tokio::fs::read_to_string(&policy_path)
            .await
            .with_context(|| format!("reading policy {}", policy_path.display()))?;
        let policy = PolicyParser::parse_str(&content)
            .with_context(|| format!("parsing policy {}", policy_path.display()))?;

        // Secrets are injected as environment variables the same way the
        // MCP path does it, so `wassette secret set <id> KEY=…` reaches
        // an ACP stage through its policy too.
        let component_secrets = secrets.snapshot(component_id).await;
        let host_env: std::collections::HashMap<String, String> = std::env::vars().collect();
        let template = create_wasi_state_template_from_policy(
            &policy,
            component_dir,
            &host_env,
            component_secrets.as_ref(),
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

/// Look for `<component-id>.policy.yaml` in the component directory, then
/// beside the `.wasm` file.
fn find_policy(component_id: &str, wasm_path: &Path, component_dir: &Path) -> Option<PathBuf> {
    let name = policy_file_name(component_id);
    let in_store = component_dir.join(&name);
    if in_store.is_file() {
        return Some(in_store);
    }
    let beside = wasm_path.parent()?.join(&name);
    if beside.is_file() {
        return Some(beside);
    }
    None
}

/// The union of every stage's grants in one chain — what the chain's
/// single `WasiCtx` is built from.
#[derive(Default, Clone)]
pub struct ChainSandbox {
    allow_all: bool,
    allow_tcp: bool,
    allow_udp: bool,
    allow_ip_name_lookup: bool,
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
    /// Union `sandbox` into this chain's grants.
    pub fn merge(&mut self, sandbox: &Sandbox) {
        match sandbox {
            Sandbox::AllowAll => self.allow_all = true,
            Sandbox::Policy(grants) => {
                let t = &grants.template;
                self.allow_tcp |= t.network_perms.allow_tcp || !t.allowed_hosts.is_empty();
                self.allow_udp |= t.network_perms.allow_udp;
                self.allow_ip_name_lookup |=
                    t.network_perms.allow_ip_name_lookup || !t.allowed_hosts.is_empty();
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

        if self.allow_all {
            wasi.inherit_network().inherit_env();
        } else {
            // `WasiCtxBuilder` denies all three by default; being
            // explicit documents that this is a decision, not an
            // omission.
            wasi.allow_tcp(self.allow_tcp);
            wasi.allow_udp(self.allow_udp);
            wasi.allow_ip_name_lookup(self.allow_ip_name_lookup);
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
            template,
        }))
    }

    #[test]
    fn no_policy_denies_network() {
        let mut chain = ChainSandbox::default();
        chain.merge(&Sandbox::Policy(Box::new(PolicyGrants {
            policy_path: None,
            template: WasiStateTemplate::default(),
        })));
        assert!(!chain.allow_tcp);
        assert!(!chain.allow_udp);
        assert!(chain.preopens.is_empty());
        assert!(chain.env.is_empty());
        assert_eq!(chain.http_allowlist().map(|h| h.len()), Some(0));
    }

    #[test]
    fn network_policy_grants_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let mut chain = ChainSandbox::default();
        chain.merge(&grants_from(
            r#"
version: "1.0"
description: "test"
permissions:
  network:
    allow:
      - host: "api.example.com"
"#,
            dir.path(),
        ));
        assert!(chain.allow_tcp);
        assert!(chain.allow_ip_name_lookup);
        assert!(chain.http_allowlist().unwrap().contains("api.example.com"));
    }

    #[test]
    fn allow_all_disables_filtering() {
        let mut chain = ChainSandbox::default();
        chain.merge(&Sandbox::AllowAll);
        assert!(chain.http_allowlist().is_none());
    }

    #[test]
    fn chain_grants_are_the_union_of_stage_grants() {
        let dir = tempfile::tempdir().unwrap();
        let mut chain = ChainSandbox::default();
        // Layer: nothing.
        chain.merge(&Sandbox::Policy(Box::new(PolicyGrants {
            policy_path: None,
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
        assert!(chain.allow_tcp);
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
        chain.merge(&grants_from(
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
        ));
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
    async fn policy_is_found_beside_the_wasm() {
        let store = tempfile::tempdir().unwrap();
        let beside = tempfile::tempdir().unwrap();
        let wasm = beside.path().join("agent.wasm");
        std::fs::write(&wasm, b"\0asm").unwrap();
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
        let sandbox = Sandbox::load(false, "agent", &wasm, store.path(), &secrets)
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
        std::fs::write(&wasm, b"\0asm").unwrap();
        for (dir, host) in [
            (store.path(), "store.example.com"),
            (beside.path(), "beside.example.com"),
        ] {
            std::fs::write(
                dir.join("agent.policy.yaml"),
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
        }
        let secrets = SecretsRegistry::new(store.path());
        let sandbox = Sandbox::load(false, "agent", &wasm, store.path(), &secrets)
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
        let wasm = store.path().join("agent.wasm");
        std::fs::write(&wasm, b"\0asm").unwrap();
        let secrets = SecretsRegistry::new(store.path());
        let sandbox = Sandbox::load(false, "agent", &wasm, store.path(), &secrets)
            .await
            .unwrap();
        assert_eq!(sandbox.describe(), "no policy (deny-all)");
        let mut chain = ChainSandbox::default();
        chain.merge(&sandbox);
        assert!(!chain.allow_tcp);
    }
}
