// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Component-scoped tool discovery and invocation.
//!
//! Exact export keys distinguish tools whose normalized names collide. They do
//! not pin an installed revision and must not be used as remembered permissions.

use anyhow::{ensure, Context, Result};
pub use component2json::FunctionIdentifier;
use serde_json::Value;
use thiserror::Error;

use crate::{
    schema, ComponentId, ComponentInstance, ComponentRegistryState, LifecycleManager, ToolInfo,
};

/// A semantic component name and its exact exported function identity.
///
/// This is neither a storage path nor a versioned authorization token. Replacing
/// a component can change what this key describes or executes.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolKey {
    /// The source-derived logical name, never a private storage key.
    pub component_id: ComponentId,
    /// Exact package, interface and function names, before normalization.
    pub export: FunctionIdentifier,
}

/// An exact tool key and the schema describing that export.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedToolDescriptor {
    /// Unversioned identity of the export.
    pub key: ToolKey,
    /// Tool schema, including its normalized `name`, description and input/output schemas.
    pub schema: Value,
}

/// A raw return value paired with the descriptor selected for its invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedToolOutput {
    /// Descriptor selected together with the component instance and export.
    pub descriptor: ScopedToolDescriptor,
    /// The existing JSON/text representation of the guest's returned value.
    pub raw_result: String,
}

/// Expected failures when resolving an exact export or a normalized tool name.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ToolLookupError {
    /// No registered export matches the selector.
    #[error("Tool not found: {tool}")]
    NotFound {
        /// Exact export or normalized name that was requested.
        tool: String,
    },
    /// More than one export matches, including collisions within one component.
    #[error("Multiple components found for tool '{tool}': {}", .components.join(", "))]
    Ambiguous {
        /// Exact export or normalized name that was requested.
        tool: String,
        /// Semantic component names for every match; a component can occur more than once.
        components: Vec<String>,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum ToolSelector<'a> {
    Exact(&'a ToolKey),
}

impl ToolSelector<'_> {
    fn label(self) -> String {
        match self {
            Self::Exact(key) => format!("{}::{:?}", key.component_id.as_str(), key.export),
        }
    }
}

impl ToolInfo {
    fn descriptor(&self) -> Result<ScopedToolDescriptor> {
        Ok(ScopedToolDescriptor {
            key: ToolKey {
                component_id: ComponentId::from_declared_name(&self.component_id)?,
                export: self.identifier.clone(),
            },
            schema: schema::canonicalize_tool_schema(&self.schema),
        })
    }
}

impl ComponentRegistryState {
    pub(crate) fn resolve_tool(
        &self,
        selector: ToolSelector<'_>,
    ) -> std::result::Result<&ToolInfo, ToolLookupError> {
        let matches: Vec<_> = match selector {
            ToolSelector::Exact(key) => self
                .tool_map
                .values()
                .flatten()
                .filter(|info| {
                    info.component_id == key.component_id.as_str() && info.identifier == key.export
                })
                .collect(),
        };
        match matches.as_slice() {
            [] => Err(ToolLookupError::NotFound {
                tool: selector.label(),
            }),
            [info] => Ok(info),
            infos => Err(ToolLookupError::Ambiguous {
                tool: selector.label(),
                components: infos.iter().map(|info| info.component_id.clone()).collect(),
            }),
        }
    }

    pub(crate) fn descriptors_for_component(
        &self,
        id: &ComponentId,
    ) -> Result<Vec<ScopedToolDescriptor>> {
        self.tool_map
            .values()
            .flatten()
            .filter(|info| info.component_id == id.as_str())
            .map(ToolInfo::descriptor)
            .collect()
    }
}

impl LifecycleManager {
    /// List descriptors for installed ordinary components requesting tool exposure.
    ///
    /// Receipt-bound metadata avoids compilation when available. This compatibility
    /// view drops the revision references; use `catalog()` for permission-safe identity.
    pub async fn list_tool_descriptors(&self) -> Result<Vec<ScopedToolDescriptor>> {
        Ok(self
            .catalog()
            .await?
            .tools
            .into_iter()
            .map(|tool| tool.tool)
            .collect())
    }

    /// List a component's exports without collapsing normalized-name collisions.
    ///
    /// The semantic name is resolved through the store; install-only and non-tool
    /// receipts are rejected. Missing metadata is restored using the existing
    /// lazy loader. No protocol exposure or permissions are granted by listing.
    pub async fn list_tools_for_component(
        &self,
        component_id: &ComponentId,
    ) -> Result<Vec<ScopedToolDescriptor>> {
        let id = component_id.as_str();
        let snapshot = self.store_snapshot(id).await?;
        ensure!(
            snapshot.receipt.requests_tool_exposure(),
            "Component '{id}' is not installed for ordinary tool exposure"
        );
        Ok(self
            .catalog()
            .await?
            .tools
            .into_iter()
            .filter(|tool| tool.tool.key.component_id == *component_id)
            .map(|tool| tool.tool)
            .collect())
    }

    /// Describe an exact export, independently of normalized-name collisions.
    ///
    /// The returned descriptor is unversioned; it does not reserve the export for
    /// a later call or bind a permission decision to an installed revision.
    pub async fn describe_scoped_tool(&self, key: &ToolKey) -> Result<ScopedToolDescriptor> {
        let mut matches = self
            .list_tools_for_component(&key.component_id)
            .await?
            .into_iter()
            .filter(|descriptor| descriptor.key == *key);
        let descriptor = matches.next().ok_or_else(|| ToolLookupError::NotFound {
            tool: ToolSelector::Exact(key).label(),
        })?;
        ensure!(
            matches.next().is_none(),
            "Duplicate exact export identity in component metadata"
        );
        Ok(descriptor)
    }

    /// Invoke an exact component export in a fresh store with its own policy/secrets.
    ///
    /// Instance, export and schema are selected together after lazy restoration.
    /// No registry or filesystem lock is held over guest execution. This resolves
    /// the latest revision; retain a `ToolRef` instead across a permission prompt.
    pub async fn invoke_scoped_tool(
        &self,
        key: &ToolKey,
        arguments: &Value,
    ) -> Result<ScopedToolOutput> {
        let catalog = self.catalog().await?;
        let tool = catalog
            .tools
            .iter()
            .find(|tool| tool.tool.key == *key)
            .ok_or_else(|| ToolLookupError::NotFound {
                tool: ToolSelector::Exact(key).label(),
            })?;
        let output = self
            .prepare_invocation(&tool.reference, arguments)
            .await?
            .run()
            .await?;
        Ok(ScopedToolOutput {
            descriptor: output.descriptor.tool,
            raw_result: output.raw_result,
        })
    }

    /// Invoke a unique normalized name among the currently registered tools.
    ///
    /// Global collisions remain errors, including collisions added during lazy
    /// loading or before final admission. Cold callers do not require a background loader.
    pub async fn invoke_unique_tool(
        &self,
        name: &str,
        arguments: &Value,
    ) -> Result<ScopedToolOutput> {
        self.invoke_catalog_name(None, name, arguments).await
    }

    pub(crate) async fn select_loaded_tool(
        &self,
        selector: ToolSelector<'_>,
    ) -> Result<(ComponentInstance, ScopedToolDescriptor)> {
        let state = self.registry.state.read().await;
        let info = state.resolve_tool(selector)?;
        let component = state
            .components
            .get(&info.component_id)
            .with_context(|| format!("Component not found: {}", info.component_id))?;
        Ok((component.clone(), info.descriptor()?))
    }
}
