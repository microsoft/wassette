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

use crate::translate;
use crate::wasm::{PromptOutcome, Session, SetConfigOptionOutcome, SetModeOutcome};
use crate::yosh::acp::content::ContentBlock;
use crate::yosh::acp::sessions::{
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
/// is intercepted before reaching any guest.
pub const TERMINAL_CONFIG_ID: &str = "terminal";

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
}

impl SessionGroup {
    /// Build a group from `(component_id, session, initial_config_options)`
    /// tuples in provider load order and the editor-facing session id.
    /// The first provider starts active. `boolean_config_supported`
    /// records whether the client opted into boolean config options and
    /// gates whether the host-owned `terminal` toggle is advertised.
    pub fn new(
        session_id: String,
        providers: Vec<(String, Session, Vec<SessionConfigOption>)>,
        boolean_config_supported: bool,
    ) -> Self {
        assert!(!providers.is_empty(), "SessionGroup needs >= 1 provider");
        let providers = providers
            .into_iter()
            .map(|(component_id, session, options)| ProviderEntry {
                component_id,
                session,
                options: Mutex::new(options),
            })
            .collect();
        Self {
            inner: Arc::new(GroupInner {
                session_id,
                providers,
                active: Mutex::new(0),
                model_map: Mutex::new(HashMap::new()),
                terminal_enabled: Mutex::new(false),
                boolean_config_supported,
            }),
        }
    }

    /// Whether more than one provider is loaded (i.e. merging is active).
    pub fn is_multi_provider(&self) -> bool {
        self.inner.providers.len() > 1
    }

    /// Point every provider chain's outbound `notify-session` updates at
    /// the group's editor-facing session id. Each provider mints its own
    /// per-session id; without this, updates from a switched (non-first)
    /// provider would reach the editor tagged with an id it never saw and
    /// be dropped. A no-op for the first provider (its id already *is* the
    /// group id). Only called for multi-provider groups, so single-provider
    /// sessions keep the guest id verbatim (transparent passthrough).
    pub async fn bind_editor_session_ids(&self) {
        for p in &self.inner.providers {
            p.session
                .set_editor_session_id(self.inner.session_id.clone())
                .await;
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

    /// Toggle the host-owned `terminal` config option. Records the new
    /// value and fans it out to every provider chain's [`Session`] so the
    /// `client.terminal` host impl honours it regardless of which provider
    /// is active (including after a later provider switch).
    pub async fn set_terminal_enabled(&self, enabled: bool) {
        *self.inner.terminal_enabled.lock().unwrap() = enabled;
        for p in &self.inner.providers {
            p.session.set_terminal_enabled(enabled).await;
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
        if inner.providers.len() == 1 {
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
            *inner.active.lock().unwrap() = idx;
            let outcome = inner.providers[idx]
                .session
                .set_config_option(target_model_id, native)
                .await;
            return inner.absorb(idx, outcome);
        }

        // Any other selector: forward to the active provider.
        let active = inner.active_idx();
        let outcome = inner.providers[active]
            .session
            .set_config_option(config_id, value)
            .await;
        inner.absorb(active, outcome)
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

    /// Cancel any in-flight prompt. Signals every provider's cancel watch
    /// (idle providers ignore it) so a mid-turn provider switch can't
    /// leave a prompt running.
    pub fn cancel(&self) {
        for p in &self.inner.providers {
            p.session.cancel();
        }
    }
}

impl GroupInner {
    fn active_idx(&self) -> usize {
        *self.active.lock().unwrap()
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
        if self.providers.len() == 1 {
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
        if current_value.is_empty() {
            if let Some(first) = groups.iter().flat_map(|g| g.options.iter()).next() {
                current_value = first.value.clone();
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
fn flatten_select_options(
    opts: &SessionConfigSelectOptions,
) -> Vec<&SessionConfigSelectOption> {
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
