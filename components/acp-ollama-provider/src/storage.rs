// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Per-session conversation history persistence.
//!
//! The host gives every wasm instance a private read/write directory at
//! `/data` (a per-app scratch dir). This component organises its own
//! `sessions/` subdirectory inside it and writes one JSON file per
//! session: `/data/sessions/<id>.json`.
//!
//! Other components are free to use `/data` differently — caches,
//! embeddings, model state — or to ignore it entirely (e.g. components
//! backed by a remote API that owns its own persistence).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ollama::Message;

const ROOT: &str = "/data";
const SESSIONS_SUBDIR: &str = "sessions";

fn sessions_dir() -> PathBuf {
    PathBuf::from(format!("{ROOT}/{SESSIONS_SUBDIR}"))
}

/// Sanitize a session id into a filename. Session ids are agent-minted but
/// might contain path separators in the future; reject anything that isn't
/// a plain filename component.
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

/// On-disk session payload: conversation history plus the active model.
///
/// Persists in tagged-object form (`{ "history": [...], "model": "..." }`).
/// Older session files written before the model picker existed contained a
/// bare `Vec<Message>`; [`load`] handles both shapes via [`OnDisk`] below.
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub history: Vec<Message>,
    pub model: String,
    /// Absolute working directory the editor sent on `session/new` /
    /// `session/load`. Used by tool runners that need to resolve
    /// relative paths into the absolute paths ACP requires (e.g.
    /// `read_file`).
    #[serde(default)]
    pub cwd: String,
}

/// Untagged accept-both view of the on-disk format. New writes always use
/// the `Tagged` variant; legacy files (just `Vec<Message>`) deserialize
/// through `Legacy`.
#[derive(Deserialize)]
#[serde(untagged)]
enum OnDisk {
    Tagged(SessionState),
    Legacy(Vec<Message>),
}

/// Read a session from disk. Returns `Ok(None)` if the file doesn't exist.
/// Legacy files (bare arrays) load with `model` set to `default_model`.
pub fn load(session_id: &str, default_model: &str) -> Result<Option<SessionState>, String> {
    let Some(path) = path_for(session_id) else {
        return Err(format!("invalid session id: {session_id:?}"));
    };
    match fs::read(&path) {
        Ok(bytes) => {
            let parsed: OnDisk = serde_json::from_slice(&bytes)
                .map_err(|e| format!("decode {}: {e}", path.display()))?;
            let session = match parsed {
                OnDisk::Tagged(s) => s,
                OnDisk::Legacy(history) => SessionState {
                    history,
                    model: default_model.to_string(),
                    cwd: String::new(),
                },
            };
            Ok(Some(session))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// Persist a session to disk. Creates the `sessions/` subdir if missing
/// and overwrites any existing file.
pub fn save(session_id: &str, session: &SessionState) -> Result<(), String> {
    let Some(path) = path_for(session_id) else {
        return Err(format!("invalid session id: {session_id:?}"));
    };
    fs::create_dir_all(sessions_dir())
        .map_err(|e| format!("mkdir {}: {e}", sessions_dir().display()))?;
    let bytes = serde_json::to_vec_pretty(session).map_err(|e| format!("encode session: {e}"))?;
    fs::write(&path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}
