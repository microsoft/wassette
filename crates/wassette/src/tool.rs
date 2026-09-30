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
    /// The embedded root component name, never a filename or private storage key.
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
    Name {
        component_id: Option<&'a str>,
        name: &'a str,
    },
}

impl ToolSelector<'_> {
    fn label(self) -> String {
        match self {
            Self::Exact(key) => format!("{}::{:?}", key.component_id.as_str(), key.export),
            Self::Name { name, .. } => name.to_owned(),
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
            ToolSelector::Name { component_id, name } => self
                .tool_map
                .get(name)
                .into_iter()
                .flatten()
                .filter(|info| component_id.is_none_or(|id| info.component_id == id))
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

    fn descriptors_for_component(&self, id: &ComponentId) -> Result<Vec<ScopedToolDescriptor>> {
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
    /// Receipt-bound metadata avoids compilation when available. This is an
    /// unversioned inventory read, not an atomic cross-component catalog snapshot.
    pub async fn list_tool_descriptors(&self) -> Result<Vec<ScopedToolDescriptor>> {
        let mut tools = Vec::new();
        for id in self.installed_tool_ids().await? {
            tools.extend(
                self.list_tools_for_component(&ComponentId::from_declared_name(&id)?)
                    .await?,
            );
        }
        Ok(tools)
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
        let mut tools = self.component_tool_descriptors(component_id).await?;
        tools.sort_by(|left, right| {
            let left = &left.key.export;
            let right = &right.key.export;
            (
                &left.package_name,
                &left.interface_name,
                &left.function_name,
            )
                .cmp(&(
                    &right.package_name,
                    &right.interface_name,
                    &right.function_name,
                ))
        });
        Ok(tools)
    }

    async fn component_tool_descriptors(
        &self,
        component_id: &ComponentId,
    ) -> Result<Vec<ScopedToolDescriptor>> {
        let id = component_id.as_str();
        let snapshot = self.store_snapshot(id).await?;
        ensure!(
            snapshot.receipt.requests_tool_exposure(),
            "Component '{id}' is not installed for ordinary tool exposure"
        );
        {
            let state = self.registry.state.read().await;
            if state.components.get(id).is_some_and(|instance| {
                instance.revision.as_ref() == Some(&snapshot.receipt.revision)
            }) {
                return state.descriptors_for_component(component_id);
            }
        }
        if let Some(metadata) = self.read_cached_metadata(id).await? {
            return Ok(metadata
                .function_identifiers
                .into_iter()
                .zip(metadata.tool_schemas)
                .map(|(export, schema)| ScopedToolDescriptor {
                    key: ToolKey {
                        component_id: snapshot.receipt.component_id.clone(),
                        export,
                    },
                    schema: schema::canonicalize_tool_schema(&schema),
                })
                .collect());
        }
        self.ensure_component_loaded(id).await?;
        let state = self.registry.state.read().await;
        ensure!(
            state.components.contains_key(id),
            "Component not found: {id}"
        );
        state.descriptors_for_component(component_id)
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
    /// No registry or filesystem lock is held over guest execution. This is not
    /// revision-safe admission for a previously approved descriptor.
    pub async fn invoke_scoped_tool(
        &self,
        key: &ToolKey,
        arguments: &Value,
    ) -> Result<ScopedToolOutput> {
        self.ensure_component_loaded(key.component_id.as_str())
            .await?;
        let (component, descriptor) = self.select_loaded_tool(ToolSelector::Exact(key)).await?;
        self.execute_tool_call(component, descriptor, arguments)
            .await
    }

    /// Invoke a unique normalized name among the currently registered tools.
    ///
    /// Global collisions remain errors, including collisions added during lazy
    /// loading. Callers populate the registry using the existing startup/hydration
    /// APIs; this method does not introduce a background catalog refresh policy.
    pub async fn invoke_unique_tool(
        &self,
        name: &str,
        arguments: &Value,
    ) -> Result<ScopedToolOutput> {
        let component_id = self
            .get_component_id_for_tool(name)
            .await
            .with_context(|| format!("Failed to find component for tool '{name}'"))?;
        self.ensure_component_loaded(&component_id)
            .await
            .with_context(|| {
                format!("Failed to load component '{component_id}' for tool '{name}'")
            })?;
        let (component, descriptor) = self
            .select_loaded_tool(ToolSelector::Name {
                component_id: None,
                name,
            })
            .await?;
        ensure!(
            descriptor.key.component_id.as_str() == component_id,
            "Tool '{name}' changed components while loading; retry the call"
        );
        self.execute_tool_call(component, descriptor, arguments)
            .await
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
