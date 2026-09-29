// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Per-session conversation history persistence.
//!
//! The host gives every wasm instance a private read/write directory at
//! `/data` (a per-app, per-project scratch dir). This component writes one
//! JSON file per session under `/data/sessions/<id>.json`.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::copilot::Message;

const ROOT: &str = "/data";
const SESSIONS_SUBDIR: &str = "sessions";
const PREFERENCES_FILE: &str = "preferences.json";

fn sessions_dir() -> PathBuf {
    PathBuf::from(format!("{ROOT}/{SESSIONS_SUBDIR}"))
}

fn preferences_path() -> PathBuf {
    PathBuf::from(format!("{ROOT}/{PREFERENCES_FILE}"))
}

/// Sanitize a session id into a filename. Reject anything that isn't a plain
/// filename component.
fn path_for(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id == "."
        || session_id == ".."
    {
        return None;
    }
    Some(sessions_dir().join(format!("{session_id}.json")))
}

/// On-disk session payload: conversation history plus the active model, the
/// active thinking level, and the working directory the editor sent on
/// `session/new` / `session/load`.
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub history: Vec<Message>,
    pub model: String,
    /// Active thinking level id (e.g. `"low"`, `"medium"`, `"high"`), sourced
    /// from the current model's upstream `reasoning_effort` set. Empty when the
    /// model advertises no reasoning levels. Sent as the native
    /// `reasoning_effort` API parameter on each turn.
    #[serde(default)]
    pub reasoning: String,
    /// Active chat mode id (`"agent"` / `"plan"` / `"autopilot"`), mirroring
    /// the Copilot CLI's mode selector. `agent` is the default conversational
    /// mode; `plan` steers the model toward producing a plan without making
    /// changes; `autopilot` runs autonomously and implies auto tool approval.
    /// Empty on legacy session files → treated as `"agent"`.
    #[serde(default)]
    pub mode: String,
    /// Auto tool approval ("allow all"). When `true`, tool calls are approved
    /// without prompting the client. Defaults to `false` (prompt for each);
    /// autopilot mode implies this even when the flag is off. Deliberately
    /// *not* seeded from global preferences, so every new session starts safe.
    #[serde(default)]
    pub allow_all: bool,
    #[serde(default)]
    pub cwd: String,
    /// Cumulative usage-based cost billed to this session so far, in AI Units
    /// (AIU), summed from each turn's streamed `copilot_usage.total_nano_aiu`
    /// (nano-AIU / 1e9). Reported to the editor via the `usage-update`'s cost
    /// field so users can track spend. `0` for sessions that only used included
    /// models or run on an unlimited/usage-based plan.
    #[serde(default)]
    pub cost_aiu: f64,
    /// Tokens occupying the model's context window as of the last completed
    /// turn (the `used` half of the context-usage meter). Persisted so a
    /// resumed session — and the *start* of every new turn — can render the
    /// meter at its last-known value instead of flashing `0`. `0` before the
    /// first turn reports usage.
    #[serde(default)]
    pub used_tokens: u64,
    /// Whether this session has ever received usage-based (AIU) billing data
    /// from the endpoint. Once `true`, the cost meter is emitted from the
    /// start of every turn (even at `0`) so the UI stays stable; kept separate
    /// from `cost_aiu` so unlimited/usage-based plans (which report `0` cost)
    /// still surface the meter.
    #[serde(default)]
    pub report_cost: bool,
}

/// Read a session from disk. Returns `Ok(None)` if the file doesn't exist.
pub fn load(session_id: &str, default_model: &str) -> Result<Option<SessionState>, String> {
    let Some(path) = path_for(session_id) else {
        return Err(format!("invalid session id: {session_id:?}"));
    };
    match fs::read(&path) {
        Ok(bytes) => {
            let mut session: SessionState = serde_json::from_slice(&bytes)
                .map_err(|e| format!("decode {}: {e}", path.display()))?;
            if session.model.is_empty() {
                session.model = default_model.to_string();
            }
            Ok(Some(session))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// Persist a session to disk. Creates the `sessions/` subdir if missing and
/// overwrites any existing file.
pub fn save(session_id: &str, session: &SessionState) -> Result<(), String> {
    let Some(path) = path_for(session_id) else {
        return Err(format!("invalid session id: {session_id:?}"));
    };
    fs::create_dir_all(sessions_dir())
        .map_err(|e| format!("mkdir {}: {e}", sessions_dir().display()))?;
    let bytes = serde_json::to_vec_pretty(session).map_err(|e| format!("encode session: {e}"))?;
    fs::write(&path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Global, cross-session defaults applied to brand-new sessions: the model and
/// thinking level the user most recently selected (in any session). Persisting
/// this lets a new session start on the same — often reasoning-capable — model
/// as the last one, so its Thinking selector is populated from the start rather
/// than only appearing after the user switches models.
#[derive(Clone, Serialize, Deserialize)]
pub struct Preferences {
    pub model: String,
    /// Last selected thinking level id (e.g. `"low"`/`"medium"`/`"high"`).
    /// Empty when the last model advertised no reasoning levels.
    #[serde(default)]
    pub reasoning: String,
}

/// Read the global preferences. Falls back to the model + thinking level of
/// the most recently modified saved session when no preferences file exists
/// yet (so an install with prior sessions still starts new sessions on the
/// last-used model). Returns `None` only when there's nothing to go on — a
/// truly fresh install — so callers use the configured default model.
pub fn load_preferences() -> Option<Preferences> {
    if let Ok(bytes) = fs::read(preferences_path())
        && let Ok(prefs) = serde_json::from_slice::<Preferences>(&bytes) {
            return Some(prefs);
        }
    latest_session_preference()
}

/// The model + reasoning of the most recently modified saved session, used as
/// a fallback before any explicit preferences file has been written.
fn latest_session_preference() -> Option<Preferences> {
    #[derive(Deserialize)]
    struct SessionPref {
        model: String,
        #[serde(default)]
        reasoning: String,
    }

    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(sessions_dir()).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if newest.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
            newest = Some((mtime, path));
        }
    }
    let (_, path) = newest?;
    let bytes = fs::read(path).ok()?;
    let pref = serde_json::from_slice::<SessionPref>(&bytes).ok()?;
    if pref.model.is_empty() {
        return None;
    }
    Some(Preferences {
        model: pref.model,
        reasoning: pref.reasoning,
    })
}

/// Persist the global preferences, creating `/data` if missing. Best-effort:
/// callers ignore the error so a failed save never fails the config change.
pub fn save_preferences(prefs: &Preferences) -> Result<(), String> {
    fs::create_dir_all(ROOT).map_err(|e| format!("mkdir {ROOT}: {e}"))?;
    let bytes =
        serde_json::to_vec_pretty(prefs).map_err(|e| format!("encode preferences: {e}"))?;
    let path = preferences_path();
    fs::write(&path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}
