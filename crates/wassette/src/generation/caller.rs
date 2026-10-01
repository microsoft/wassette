// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::*;
use crate::store::EntryRevision;

/// Explicit operator delegation to one installed ordinary component revision.
///
/// These records are read only from host configuration, never from build inputs.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationCallerGrant {
    /// Exact semantic caller name.
    pub component_id: String,
    /// Exact caller revision; updates require a new operator grant.
    pub revision: String,
    /// Permission to spend bounded compiler resources.
    #[serde(default)]
    pub allow_build: bool,
    /// Permission to commit newly generated artifacts.
    #[serde(default)]
    pub allow_install: bool,
    /// Permission to request ordinary-tool runtime eligibility.
    #[serde(default)]
    pub allow_expose: bool,
    /// Exact generated lineage IDs this caller may rebuild, not semantic aliases.
    #[serde(default)]
    pub rebuild_sources: Vec<String>,
}

/// Identity captured by ordinary-tool admission, never supplied by the guest.
#[derive(Clone)]
pub(crate) struct GenerationCaller {
    pub manager: LifecycleManager,
    pub component_id: String,
    pub revision: EntryRevision,
}

impl GenerationService {
    /// Add explicit ordinary-caller grants; no grant is inferred from tool policy.
    pub fn with_callers(mut self, callers: Vec<GenerationCallerGrant>) -> Result<Self> {
        ensure!(
            callers.len() <= 128,
            "too many component-generation caller grants"
        );
        let mut identities = HashSet::new();
        for caller in &callers {
            crate::ComponentId::from_declared_name(&caller.component_id)?;
            ensure!(
                !caller.revision.is_empty()
                    && caller.revision.len() <= 128
                    && !caller.revision.chars().any(char::is_control),
                "invalid generation caller revision"
            );
            ensure!(
                identities.insert((&caller.component_id, &caller.revision)),
                "duplicate component-generation caller grant"
            );
            ensure!(
                caller.rebuild_sources.len() <= 128,
                "too many generated rebuild grants"
            );
            for id in &caller.rebuild_sources {
                SourceIdentity::Generated { id: id.clone() }.validate()?;
            }
        }
        self.callers = callers;
        Ok(self)
    }

    pub(crate) async fn permissions_for_caller(
        &self,
        caller: &GenerationCaller,
        target_name: &str,
        target: &GenerationTarget,
    ) -> Result<GenerationPermissions> {
        authorize_caller(&self.callers, self.permissions, caller, target_name, target).await
    }
}

async fn authorize_caller(
    grants: &[GenerationCallerGrant],
    ceiling: GenerationPermissions,
    caller: &GenerationCaller,
    target_name: &str,
    target: &GenerationTarget,
) -> Result<GenerationPermissions> {
    let grant = grants
        .iter()
        .find(|grant| {
            grant.component_id == caller.component_id
                && grant.revision == caller.revision.to_string()
        })
        .ok_or(GenerationError::PermissionDenied(
            "ordinary component caller",
        ))?;
    let caller_id = caller.component_id.clone();
    let caller_revision = caller.revision.clone();
    let target_name = target_name.to_owned();
    let target_source = store_operation(caller.manager.component_store(), move |store| {
        let snapshot = store
            .snapshot_if_changed(None)?
            .context("missing generation caller inventory")?;
        let current = snapshot
            .entries
            .iter()
            .find(|entry| entry.component_id().as_str() == caller_id);
        ensure!(
            matches!(current, Some(StoredEntry::Installed(receipt))
                    if receipt.revision == caller_revision && receipt.requests_tool_exposure()),
            "ordinary generation caller is stale or unavailable"
        );
        Ok(snapshot
            .entries
            .iter()
            .find(|entry| entry.component_id().as_str() == target_name)
            .map(|entry| entry.binding().source.clone()))
    })
    .await?;
    let may_rebuild = matches!(target, GenerationTarget::Rebuild { .. })
        && matches!(target_source, Some(SourceIdentity::Generated { ref id })
                if grant.rebuild_sources.contains(id));
    Ok(GenerationPermissions::new(
        ceiling.build && grant.allow_build,
        ceiling.install && grant.allow_install,
        ceiling.expose && grant.allow_expose,
        ceiling.rebuild && may_rebuild,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{candidate, manager, permissions, request, wasm};
    use super::*;

    #[tokio::test]
    async fn caller_grants_are_default_denied_and_revision_bound() {
        let (_root, manager) = manager().await;
        let first = candidate(
            &manager,
            request("caller", InstallIntent::ExposeTools),
            wasm("caller", 1),
        )
        .await
        .install(permissions(), CancellationToken::new())
        .await
        .unwrap();
        let caller = GenerationCaller {
            manager: manager.clone(),
            component_id: "caller".into(),
            revision: first.commit.entry.revision().clone(),
        };
        let ceiling = permissions();
        assert!(
            authorize_caller(&[], ceiling, &caller, "new", &GenerationTarget::New)
                .await
                .is_err()
        );
        let grants = vec![GenerationCallerGrant {
            component_id: caller.component_id.clone(),
            revision: caller.revision.to_string(),
            allow_build: true,
            allow_install: true,
            allow_expose: false,
            rebuild_sources: Vec::new(),
        }];
        let allowed = authorize_caller(&grants, ceiling, &caller, "new", &GenerationTarget::New)
            .await
            .unwrap();
        assert!(allowed.can_build() && allowed.can_install());
        assert!(!allowed.can_expose() && !allowed.can_rebuild());
        let wrong = GenerationCaller {
            component_id: "another-caller".into(),
            ..caller.clone()
        };
        assert!(
            authorize_caller(&grants, ceiling, &wrong, "new", &GenerationTarget::New)
                .await
                .is_err()
        );
        manager
            .component_store()
            .update_policy(
                "caller",
                &caller.revision,
                PreparedPolicy::absent(PolicyProvenance::PermissionEdit),
            )
            .unwrap();
        assert!(
            authorize_caller(&grants, ceiling, &caller, "new", &GenerationTarget::New)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_rebuild_grant_names_a_lineage_not_a_claimed_component_name() {
        let (_root, manager) = manager().await;
        let first = candidate(
            &manager,
            request("caller", InstallIntent::ExposeTools),
            wasm("caller", 1),
        )
        .await
        .install(permissions(), CancellationToken::new())
        .await
        .unwrap();
        let caller = GenerationCaller {
            manager: manager.clone(),
            component_id: "caller".into(),
            revision: first.commit.entry.revision().clone(),
        };
        let target = candidate(
            &manager,
            request("target", InstallIntent::InstallOnly),
            wasm("target", 1),
        )
        .await
        .install(permissions(), CancellationToken::new())
        .await
        .unwrap();
        let rebuild = GenerationTarget::Rebuild {
            expected_revision: target.commit.entry.revision().to_string(),
        };
        let mut grant = GenerationCallerGrant {
            component_id: caller.component_id.clone(),
            revision: caller.revision.to_string(),
            allow_build: true,
            allow_install: true,
            allow_expose: false,
            rebuild_sources: vec!["ab".repeat(16)],
        };
        let denied = authorize_caller(&[grant.clone()], permissions(), &caller, "target", &rebuild)
            .await
            .unwrap();
        assert!(!denied.can_rebuild());
        let SourceIdentity::Generated { id } = &target.commit.entry.binding().source else {
            panic!("generated source")
        };
        grant.rebuild_sources = vec![id.clone()];
        let allowed = authorize_caller(&[grant], permissions(), &caller, "target", &rebuild)
            .await
            .unwrap();
        assert!(allowed.can_rebuild());
    }
}
