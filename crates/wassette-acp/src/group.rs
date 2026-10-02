// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Multi-provider session grouping.
//!
//! One ACP session maps to a [`SessionGroup`]: a bundle of one
//! [`Session`] per loaded provider (each its own wasm chain / `Store`),
//! sharing a single editor-facing session id. The group merges every
//! provider's **model** selector into one cross-provider dropdown — each
//! entry labelled by the provider that owns it — so the user picks which
//! model *from which provider* backs the session. The provider that owns
//! the active model is the **active provider**: it backs prompts, mode
//! switches, and the non-model selectors (mode / thinking / …).
//!
//! Selecting a model from a different provider switches the active
//! provider; the option set is then rebuilt from the new active provider
//! (plus the merged model list).
//!
//! With a **single** provider the group is a transparent passthrough:
//! options, values, and ids are forwarded verbatim, so existing
//! single-provider behaviour (and its value formats) is unchanged.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::tool_broker::ToolInventoryEntry;
use crate::translate;
use crate::wasm::{
    LiveConfigOption, PromptOutcome, Session, SetConfigOptionOutcome, SetModeOutcome,
};
use crate::wassette::acp::content::ContentBlock;
use crate::wassette::acp::sessions::{
    ComponentSource, SessionConfigId, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectGroup, SessionConfigSelectOption, SessionConfigSelectOptions,
    SessionConfigValueId, SessionModeId,
};

/// Well-known config-option id for the host's merged, cross-provider
/// model selector. Deliberately matches the id every in-tree provider
/// already uses for its own model selector so single-provider passthrough
/// is a no-op and clients keep their model-category keyboard shortcuts.
const HOST_MODEL_CONFIG_ID: &str = "model";

/// Well-known config-option id for the host-owned terminal (CLI) toggle.
/// A boolean session config option (default `false`) that gates host-side
/// terminal execution for every provider chain in the group. Owned and
/// enforced by the host — no guest provider advertises it, and the setter
/// is intercepted by the host. The Copilot provider also receives an
/// internal notification to keep its model-facing tool list in sync.
pub const TERMINAL_CONFIG_ID: &str = "terminal";
// Stopgap until ACP advertises internal config capabilities: all providers
// export the same setter, and Copilot deliberately hides these options.
fn is_copilot_provider(component_id: &str) -> bool {
    matches!(
        component_id,
        "local:acp_copilot_provider" | "acp-copilot-provider"
    )
}

/// Internal, never-advertised config option the host sends to the Copilot
/// provider when a builder image enables component generation.
const GENERATION_CONFIG_ID: &str = "component-generation";

/// Identity used for host-synthesized config entries (the merged model
/// selector). `translate` drops config-option provenance on the wire, so
/// this is informational only.
const HOST_COMPONENT_ID: &str = "local:host";

/// Separates the provider id from the provider-native model value inside a
/// merged model selector value. ASCII unit separator: never appears in a
/// component id or model id, and the host keeps a decode map anyway so the
/// value is never parsed back apart.
const MODEL_VALUE_DELIM: char = '\u{1f}';

/// One provider inside a group: its chain [`Session`] plus the latest
/// config options it advertised (refreshed on every set-config-option
/// round-trip so the merged view stays current).
struct ProviderEntry {
    component_id: String,
    session: Session,
    options: Mutex<Vec<SessionConfigOption>>,
}

pub struct ProviderSession {
    pub component_id: String,
    pub session: Session,
    pub options: Option<Vec<SessionConfigOption>>,
}

/// A bundle of provider sessions presented to the editor as one ACP
/// session. Cheap to clone (an `Arc`).
#[derive(Clone)]
pub struct SessionGroup {
    inner: Arc<GroupInner>,
}

struct GroupInner {
    /// Editor-facing ACP session id for the whole group.
    session_id: String,
    /// Providers in load order. Always non-empty.
    providers: Vec<ProviderEntry>,
    multi_provider: bool,
    /// Index into `providers` of the active provider (backs prompts and
    /// the non-model selectors).
    active: Mutex<usize>,
    /// Decode map for merged model values: merged value id -> (provider
    /// index, provider-native model value). Rebuilt on every
    /// [`GroupInner::build_options`].
    model_map: Mutex<HashMap<String, (usize, String)>>,
    /// Current value of the host-owned `terminal` boolean config option.
    /// Defaults to `false`; toggled via `session/set_config_option` and
    /// fanned out to every provider chain's [`Session`].
    terminal_enabled: Mutex<bool>,
    /// Whether the client advertised support for boolean config options
    /// (`session.configOptions.boolean`). When `false` the group does not
    /// advertise the `terminal` toggle at all (per the RFD, agents must
    /// not send boolean options to clients that didn't opt in).
    boolean_config_supported: bool,
    operation: Arc<tokio::sync::Mutex<()>>,
    configuration: Arc<tokio::sync::RwLock<()>>,
    live_setter: tokio::sync::Mutex<()>,
}

pub struct ConfigOperation {
    pub live: bool,
    _shared: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
    _exclusive: Option<tokio::sync::OwnedRwLockWriteGuard<()>>,
    _operation: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl SessionGroup {
    /// Build a group in provider load order and the editor-facing session id.
    /// The first provider with model choices starts active in multi-provider
    /// mode; a single provider is passed through. `boolean_config_supported`
    /// records whether the client opted into boolean config options and
    /// gates whether the host-owned `terminal` toggle is advertised.
    pub fn new(
        session_id: String,
        providers: Vec<ProviderSession>,
        boolean_config_supported: bool,
    ) -> anyhow::Result<Self> {
        assert!(!providers.is_empty(), "SessionGroup needs >= 1 provider");
        let multi_provider = providers.len() > 1;
        let providers = if multi_provider {
            let mut eligible = Vec::new();
            for provider in providers {
                let options = provider.options.as_deref().unwrap_or_default();
                if validated_model(options, &provider.component_id)?.is_none() {
                    tracing::info!(provider = %provider.component_id, "omitting provider without model choices");
                    continue;
                }
                eligible.push(provider);
            }
            anyhow::ensure!(
                !eligible.is_empty(),
                "no selectable ACP providers: multi-provider sessions require at least one provider with model choices"
            );
            eligible
        } else {
            providers
        };
        let providers = providers
            .into_iter()
            .map(
                |ProviderSession {
                     component_id,
                     session,
                     options,
                 }| {
                    ProviderEntry {
                        component_id,
                        session,
                        options: Mutex::new(options.unwrap_or_default()),
                    }
                },
            )
            .collect();
        Ok(Self {
            inner: Arc::new(GroupInner {
                session_id,
                providers,
                multi_provider,
                active: Mutex::new(0),
                model_map: Mutex::new(HashMap::new()),
                terminal_enabled: Mutex::new(false),
                boolean_config_supported,
                operation: Arc::new(tokio::sync::Mutex::new(())),
                configuration: Arc::new(tokio::sync::RwLock::new(())),
                live_setter: tokio::sync::Mutex::new(()),
            }),
        })
    }

    /// Whether more than one provider is loaded (i.e. merging is active).
    pub fn is_multi_provider(&self) -> bool {
        self.inner.multi_provider
    }

    /// Reserve before spawning the request so selection cannot overtake an
    /// accepted prompt, and cancellation never waits on the store lock.
    pub fn begin_operation(
        &self,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, agent_client_protocol::Error> {
        self.inner
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| busy_error())
    }

    pub fn begin_configuration(
        &self,
        config_id: &str,
    ) -> Result<ConfigOperation, agent_client_protocol::Error> {
        let shared = self
            .inner
            .configuration
            .clone()
            .try_read_owned()
            .map_err(|_| busy_error())?;
        let live = self.live_config_available(config_id);
        if live {
            Ok(ConfigOperation {
                live,
                _shared: Some(shared),
                _exclusive: None,
                _operation: None,
            })
        } else {
            drop(shared);
            let exclusive = self
                .inner
                .configuration
                .clone()
                .try_write_owned()
                .map_err(|_| busy_error())?;
            let operation = self.begin_operation()?;
            Ok(ConfigOperation {
                live,
                _shared: None,
                _exclusive: Some(exclusive),
                _operation: Some(operation),
            })
        }
    }

    pub async fn lock_live_setter(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.live_setter.lock().await
    }

    fn live_config_available(&self, config_id: &str) -> bool {
        if config_id == TERMINAL_CONFIG_ID {
            return self.inner.providers.iter().all(|provider| {
                !is_copilot_provider(&provider.component_id)
                    || provider.session.live_config_supported()
            });
        }
        let active = self.inner.active_idx();
        let provider = &self.inner.providers[active];
        config_id == "allow-all"
            && is_copilot_provider(&provider.component_id)
            && provider.session.live_config_supported()
            && provider.options.lock().unwrap().iter().any(|option| {
                option.id == config_id
                    && matches!(&option.options, SessionConfigSelectOptions::Ungrouped(values)
                        if values.len() == 2
                            && values.iter().any(|value| value.value == "on")
                            && values.iter().any(|value| value.value == "off"))
            })
    }

    /// Finish binding and release only the selected chain's creation updates.
    /// Multi-provider chains already have this host ID before guest creation;
    /// single-provider chains retain their native session ID.
    pub async fn bind_editor_session_ids(&self) {
        for (index, p) in self.inner.providers.iter().enumerate() {
            p.session
                .set_editor_session_id(self.inner.session_id.clone())
                .await;
            if self.is_multi_provider() {
                p.session.set_active(index == 0).await;
            }
        }
    }

    /// The merged config-option set to advertise to the editor. Also
    /// refreshes the internal model-value decode map.
    pub fn config_options(&self) -> Vec<SessionConfigOption> {
        self.inner.build_options()
    }

    /// Current state of the host-owned `terminal` boolean config option,
    /// or `None` when it must not be advertised (client didn't opt into
    /// boolean config options). `Some(false)` means advertised and off.
    pub fn terminal_option(&self) -> Option<bool> {
        if self.inner.boolean_config_supported {
            Some(*self.inner.terminal_enabled.lock().unwrap())
        } else {
            None
        }
    }

    /// The active chain's catalog is representative of this editor session;
    /// each chain still has its own broker view and permission decisions.
    pub async fn tool_inventory(&self) -> anyhow::Result<Vec<ToolInventoryEntry>> {
        let index = *self.inner.active.lock().unwrap();
        let broker = self.inner.providers[index]
            .session
            .tool_broker()
            .await
            .ok_or_else(|| anyhow::anyhow!("Tool broker unavailable"))?;
        broker.tool_inventory().await.map_err(Into::into)
    }

    pub async fn generation_available(&self) -> bool {
        let index = *self.inner.active.lock().unwrap();
        if !is_copilot_provider(&self.inner.providers[index].component_id) {
            return false;
        }
        self.inner.providers[index]
            .session
            .tool_broker()
            .await
            .is_some_and(|broker| broker.generation_available())
    }

    pub fn copilot_active(&self) -> bool {
        let index = *self.inner.active.lock().unwrap();
        is_copilot_provider(&self.inner.providers[index].component_id)
    }

    pub async fn set_tool_enabled(
        &self,
        reference: wassette::ToolRef,
        enabled: bool,
    ) -> anyhow::Result<()> {
        for provider in &self.inner.providers {
            let broker = provider
                .session
                .tool_broker()
                .await
                .ok_or_else(|| anyhow::anyhow!("Tool broker unavailable"))?;
            broker.set_tool_enabled(reference.clone(), enabled).await?;
        }
        Ok(())
    }

    pub async fn enable_component_tools(&self, component_id: &str) -> anyhow::Result<Vec<String>> {
        let mut names = None;
        for provider in &self.inner.providers {
            let broker = provider
                .session
                .tool_broker()
                .await
                .ok_or_else(|| anyhow::anyhow!("Tool broker unavailable"))?;
            let enabled = broker.enable_component_tools(component_id).await?;
            names.get_or_insert(enabled);
        }
        Ok(names.unwrap_or_default())
    }

    /// Toggle the host-owned `terminal` config option. Records the new
    /// value and fans it out to every provider chain's [`Session`] so the
    /// `client.terminal` host impl honours it regardless of which provider
    /// is active (including after a later provider switch).
    pub async fn set_terminal_enabled(&self, enabled: bool) -> anyhow::Result<()> {
        let previous = *self.inner.terminal_enabled.lock().unwrap();
        let mut notified = Vec::new();
        for p in &self.inner.providers {
            if is_copilot_provider(&p.component_id) {
                notified.push(p);
                match notify_copilot_terminal(&p.session, enabled).await {
                    SetConfigOptionOutcome::Done(_) => {}
                    SetConfigOptionOutcome::Wit(e) => {
                        return Err(self
                            .rollback_terminal(
                                &notified,
                                &[],
                                previous,
                                anyhow::anyhow!("Copilot provider rejected terminal toggle: {e:?}"),
                            )
                            .await);
                    }
                    SetConfigOptionOutcome::Trap(e) => {
                        return Err(self
                            .rollback_terminal(
                                &notified,
                                &[],
                                previous,
                                e.context("Copilot provider terminal toggle trapped").into(),
                            )
                            .await);
                    }
                }
            }
        }
        let mut updated = Vec::new();
        for p in &self.inner.providers {
            updated.push(p);
            if let Err(error) = p.session.set_terminal_enabled(enabled).await {
                return Err(self
                    .rollback_terminal(
                        &notified,
                        &updated,
                        previous,
                        error.context("host terminal toggle failed").into(),
                    )
                    .await);
            }
        }
        *self.inner.terminal_enabled.lock().unwrap() = enabled;
        Ok(())
    }

    /// Tell every Copilot provider chain that the operator permits guest
    /// component generation, so it advertises its model-facing build tool.
    /// Only sent when generation is available: a disabled host sends nothing
    /// and the provider keeps the tool hidden. The host stays authoritative —
    /// `builder.generate` still checks the profile on every call. A provider
    /// that rejects the notification (e.g. an older build) keeps working
    /// without the tool.
    pub async fn enable_copilot_generation(&self) {
        for p in &self.inner.providers {
            if !is_copilot_provider(&p.component_id) {
                continue;
            }
            match p
                .session
                .set_config_option(GENERATION_CONFIG_ID.to_string(), "on".to_string())
                .await
            {
                SetConfigOptionOutcome::Done(_) => {}
                SetConfigOptionOutcome::Wit(e) => tracing::warn!(
                    provider = %p.component_id, error = ?e,
                    "Copilot provider rejected the component-generation notification"
                ),
                SetConfigOptionOutcome::Trap(e) => tracing::warn!(
                    provider = %p.component_id, error = %e,
                    "Copilot provider trapped on the component-generation notification"
                ),
            }
        }
    }

    async fn rollback_terminal(
        &self,
        providers: &[&ProviderEntry],
        hosts: &[&ProviderEntry],
        enabled: bool,
        error: anyhow::Error,
    ) -> anyhow::Error {
        let mut failures = Vec::new();
        for host in hosts {
            if let Err(error) = host.session.set_terminal_enabled(enabled).await {
                tracing::error!(provider = %host.component_id, %error, "failed to restore host terminal gate");
                failures.push(format!("{} host gate: {error}", host.component_id));
            }
        }
        for p in providers {
            match notify_copilot_terminal(&p.session, enabled).await {
                SetConfigOptionOutcome::Done(_) => {}
                SetConfigOptionOutcome::Wit(e) => {
                    tracing::error!(provider = %p.component_id, error = ?e, "failed to restore Copilot terminal tool list");
                    failures.push(format!("{} provider: {e:?}", p.component_id));
                }
                SetConfigOptionOutcome::Trap(e) => {
                    tracing::error!(provider = %p.component_id, error = %e, "failed to restore Copilot terminal tool list");
                    failures.push(format!("{} provider: {e}", p.component_id));
                }
            }
        }
        if failures.is_empty() {
            error
        } else {
            error.context(format!(
                "terminal restoration failed: {}",
                failures.join("; ")
            ))
        }
    }

    /// Handle `session/set_config_option`. Routes model selections to the
    /// owning provider (switching the active provider when it differs) and
    /// every other selector to the active provider, then returns the full
    /// rebuilt option set.
    pub async fn set_config_option(
        &self,
        config_id: SessionConfigId,
        value: SessionConfigValueId,
    ) -> SetConfigOptionOutcome {
        let inner = &self.inner;

        // Single provider: forward verbatim.
        if !inner.multi_provider {
            let outcome = inner.providers[0]
                .session
                .set_config_option(config_id, value)
                .await;
            return inner.absorb(0, outcome);
        }

        // Merged model selector: decode -> (provider, native value).
        if config_id == HOST_MODEL_CONFIG_ID {
            let decoded = inner.model_map.lock().unwrap().get(&value).cloned();
            let Some((idx, native)) = decoded else {
                return SetConfigOptionOutcome::Wit(translate::internal_error(&format!(
                    "unknown model selection `{value}`"
                )));
            };
            // The target provider's own model-option id (usually "model").
            let target_model_id = inner.providers[idx]
                .options
                .lock()
                .unwrap()
                .iter()
                .find(|o| is_model(o))
                .map(|o| o.id.clone());
            let Some(target_model_id) = target_model_id else {
                return SetConfigOptionOutcome::Wit(translate::internal_error(
                    "target provider advertises no model selector",
                ));
            };
            let outcome = inner.providers[idx]
                .session
                .set_config_option(target_model_id, native)
                .await;
            let outcome = inner.validate_outcome(idx, outcome);
            if matches!(outcome, SetConfigOptionOutcome::Done(_)) && idx != inner.active_idx() {
                inner.providers[inner.active_idx()]
                    .session
                    .set_active(false)
                    .await;
                inner.providers[idx].session.set_active(true).await;
            }
            commit_active_on_success(&inner.active, idx, &outcome);
            return inner.absorb(idx, outcome);
        }

        // Any other selector: forward to the active provider.
        let active = inner.active_idx();
        let outcome = inner.providers[active]
            .session
            .set_config_option(config_id, value)
            .await;
        let outcome = inner.validate_outcome(active, outcome);
        inner.absorb(active, outcome)
    }

    pub async fn set_auto_approve(&self, enabled: bool) -> SetConfigOptionOutcome {
        let active = self.inner.active_idx();
        let outcome = self.inner.providers[active]
            .session
            .set_live_config_option(LiveConfigOption::AutoApprove, enabled)
            .await;
        let outcome = self.inner.validate_outcome(active, outcome);
        self.inner.absorb(active, outcome)
    }

    /// Switch the active provider's session mode (legacy `set-mode`).
    pub async fn set_mode(&self, mode_id: SessionModeId) -> SetModeOutcome {
        let active = self.inner.active_idx();
        self.inner.providers[active].session.set_mode(mode_id).await
    }

    /// Run a prompt turn on the active provider. Updates are forwarded
    /// under the group's editor-facing session id.
    pub async fn prompt(&self, prompt: Vec<ContentBlock>) -> PromptOutcome {
        let active = self.inner.active_idx();
        let session_id = self.inner.session_id.clone();
        self.inner.providers[active]
            .session
            .prompt(session_id, prompt)
            .await
    }

    /// Reset before the prompt task is spawned, so a later cancel is not lost.
    pub fn prepare_prompt(&self) {
        let active = self.inner.active_idx();
        self.inner.providers[active].session.prepare_prompt();
    }

    /// Cancel any in-flight prompt. Signals every provider's cancel watch
    /// (idle providers ignore it) so a mid-turn provider switch can't
    /// leave a prompt running.
    pub fn cancel(&self) {
        for p in &self.inner.providers {
            p.session.cancel();
        }
    }
}

fn busy_error() -> agent_client_protocol::Error {
    let mut error = agent_client_protocol::Error::invalid_request();
    error.message = "session is busy; cancel or finish the current operation before changing providers or starting another operation".to_string();
    error
}

async fn notify_copilot_terminal(session: &Session, enabled: bool) -> SetConfigOptionOutcome {
    if session.live_config_supported() {
        session
            .set_live_config_option(LiveConfigOption::Terminal, enabled)
            .await
    } else {
        session
            .set_config_option(
                TERMINAL_CONFIG_ID.to_string(),
                if enabled { "on" } else { "off" }.to_string(),
            )
            .await
    }
}

fn commit_active_on_success(active: &Mutex<usize>, idx: usize, outcome: &SetConfigOptionOutcome) {
    if matches!(outcome, SetConfigOptionOutcome::Done(_)) {
        *active.lock().unwrap() = idx;
    }
}

fn validated_model<'a>(
    options: &'a [SessionConfigOption],
    component_id: &str,
) -> anyhow::Result<Option<&'a SessionConfigOption>> {
    let mut models = options.iter().filter(|option| is_model(option));
    let Some(model) = models.next() else {
        return Ok(None);
    };
    anyhow::ensure!(
        models.next().is_none(),
        "provider `{component_id}` advertises multiple model selectors"
    );
    let choices = flatten_select_options(&model.options);
    if choices.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(
        choices
            .iter()
            .any(|choice| choice.value == model.current_value),
        "provider `{component_id}` advertises a current model absent from its choices"
    );
    let mut values = std::collections::HashSet::new();
    anyhow::ensure!(
        choices.iter().all(|choice| values.insert(&choice.value)),
        "provider `{component_id}` advertises duplicate model values"
    );
    anyhow::ensure!(
        !options
            .iter()
            .any(|option| option.id == HOST_MODEL_CONFIG_ID && !is_model(option)),
        "provider `{component_id}` uses reserved `model` id for a non-model option"
    );
    Ok(Some(model))
}

impl GroupInner {
    fn active_idx(&self) -> usize {
        *self.active.lock().unwrap()
    }

    fn validate_outcome(
        &self,
        idx: usize,
        outcome: SetConfigOptionOutcome,
    ) -> SetConfigOptionOutcome {
        if self.multi_provider
            && let SetConfigOptionOutcome::Done(options) = &outcome
        {
            match validated_model(options, &self.providers[idx].component_id) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return SetConfigOptionOutcome::Wit(translate::internal_error(
                        "provider removed its model choices",
                    ));
                }
                Err(error) => {
                    return SetConfigOptionOutcome::Wit(translate::internal_error(
                        &error.to_string(),
                    ));
                }
            }
        }
        outcome
    }

    /// Store a provider's freshly returned options (on a successful
    /// set-config-option) and rebuild the merged view; pass errors/traps
    /// through unchanged.
    fn absorb(&self, idx: usize, outcome: SetConfigOptionOutcome) -> SetConfigOptionOutcome {
        match outcome {
            SetConfigOptionOutcome::Done(opts) => {
                *self.providers[idx].options.lock().unwrap() = opts;
                SetConfigOptionOutcome::Done(self.build_options())
            }
            other => other,
        }
    }

    /// Compute the option set advertised to the editor, refreshing
    /// `model_map`. Single provider: verbatim passthrough. Multiple: one
    /// merged model selector plus the active provider's other selectors.
    fn build_options(&self) -> Vec<SessionConfigOption> {
        if !self.multi_provider {
            let opts = self.providers[0].options.lock().unwrap().clone();
            // Identity decode map so model selections route uniformly.
            let mut map = HashMap::new();
            if let Some(model) = opts.iter().find(|o| is_model(o)) {
                for so in flatten_select_options(&model.options) {
                    map.insert(so.value.clone(), (0usize, so.value.clone()));
                }
            }
            *self.model_map.lock().unwrap() = map;
            return opts;
        }

        let active = self.active_idx();
        let active_opts = self.providers[active].options.lock().unwrap().clone();

        // Merge every provider's model options into one selector, one
        // native ACP group per provider. Option *values* stay
        // group-unique (encoded with the provider id) because selection
        // round-trips by value alone; the group is display-only, so
        // option *names* are the provider's own, unsuffixed.
        let mut groups: Vec<SessionConfigSelectGroup> = Vec::new();
        let mut map: HashMap<String, (usize, String)> = HashMap::new();
        let mut current_value = String::new();
        for (idx, p) in self.providers.iter().enumerate() {
            let opts = p.options.lock().unwrap();
            let Some(model) = opts.iter().find(|o| is_model(o)) else {
                continue;
            };
            let mut group_values: Vec<SessionConfigSelectOption> = Vec::new();
            for so in flatten_select_options(&model.options) {
                let merged = encode_model_value(&p.component_id, &so.value);
                map.insert(merged.clone(), (idx, so.value.clone()));
                group_values.push(SessionConfigSelectOption {
                    value: merged.clone(),
                    name: so.name.clone(),
                    description: so.description.clone(),
                });
                if idx == active && so.value == model.current_value {
                    current_value = merged;
                }
            }
            if !group_values.is_empty() {
                groups.push(SessionConfigSelectGroup {
                    group: p.component_id.clone(),
                    name: p.component_id.clone(),
                    options: group_values,
                });
            }
        }
        *self.model_map.lock().unwrap() = map;

        let merged_model = SessionConfigOption {
            id: HOST_MODEL_CONFIG_ID.to_string(),
            name: "Model".to_string(),
            description: Some("Model to use, grouped by provider.".to_string()),
            category: Some(SessionConfigOptionCategory::Model),
            current_value,
            options: SessionConfigSelectOptions::Grouped(groups),
            provided_by: ComponentSource {
                component_id: HOST_COMPONENT_ID.to_string(),
            },
        };

        // Active provider's non-model selectors, with the merged model
        // selector taking the place of the active provider's own (or
        // prepended if it has none).
        let mut out = Vec::with_capacity(active_opts.len() + 1);
        let mut inserted = false;
        for o in active_opts {
            if is_model(&o) {
                if !inserted {
                    out.push(merged_model.clone());
                    inserted = true;
                }
            } else {
                out.push(o);
            }
        }
        if !inserted {
            out.insert(0, merged_model);
        }
        out
    }
}

/// Whether a config option is the model selector (`category == model`).
fn is_model(o: &SessionConfigOption) -> bool {
    matches!(o.category, Some(SessionConfigOptionCategory::Model))
}

/// Flatten a provider's select options (ungrouped or grouped) into a
/// single sequence. Grouping is a display concern the host re-derives per
/// provider, so scanning a provider's own options ignores any inbound
/// grouping.
fn flatten_select_options(opts: &SessionConfigSelectOptions) -> Vec<&SessionConfigSelectOption> {
    match opts {
        SessionConfigSelectOptions::Ungrouped(list) => list.iter().collect(),
        SessionConfigSelectOptions::Grouped(groups) => {
            groups.iter().flat_map(|g| g.options.iter()).collect()
        }
    }
}

/// Encode a provider-native model value into a group-unique merged value.
fn encode_model_value(provider_id: &str, value: &str) -> String {
    format!("{provider_id}{MODEL_VALUE_DELIM}{value}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_capabilities_use_source_or_retained_receipt_identity() {
        for id in ["local:acp_copilot_provider", "acp-copilot-provider"] {
            assert!(is_copilot_provider(id), "{id}");
        }
        for id in [
            "acp_copilot_provider",
            "local:unrelated-provider",
            "local:acp-copilot-provider",
            "cosmetic:acp-copilot-provider",
            "ghcr.io/example/acp-copilot-provider",
        ] {
            assert!(!is_copilot_provider(id), "{id}");
        }
    }

    #[test]
    fn failed_model_selection_keeps_the_previous_provider() {
        let active = Mutex::new(0);
        let failure = SetConfigOptionOutcome::Wit(translate::internal_error("model rejected"));
        commit_active_on_success(&active, 1, &failure);
        assert_eq!(*active.lock().unwrap(), 0);
        commit_active_on_success(&active, 1, &SetConfigOptionOutcome::Done(vec![]));
        assert_eq!(*active.lock().unwrap(), 1);
    }

    #[test]
    fn model_less_options_keep_other_selectors() {
        let opts = [SessionConfigOption {
            id: "mode".to_string(),
            name: "Mode".to_string(),
            description: None,
            category: None,
            current_value: "default".to_string(),
            options: SessionConfigSelectOptions::Ungrouped(vec![]),
            provided_by: ComponentSource {
                component_id: "local:provider".to_string(),
            },
        }];
        let mut empty_model = opts[0].clone();
        empty_model.id = "model".to_string();
        empty_model.category = Some(SessionConfigOptionCategory::Model);
        assert!(
            validated_model(&[empty_model, opts[0].clone()], "test")
                .unwrap()
                .is_none()
        );
    }
}
