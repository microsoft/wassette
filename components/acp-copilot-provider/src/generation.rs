// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Copilot model adapter for Wassette's host-owned component-generation import.
//!
//! The host advertises nothing here: it notifies this provider through an
//! internal config option when the operator profile permits building and
//! installing components, and remains the authority on every request. It
//! prompts the editor for each phase (build, install, exposure), so this
//! provider does not add a permission prompt of its own.

use serde_json::{Value, json};

use crate::bindings::wassette::component_generation::builder::{
    Disposition, GenerationError, GenerationReport,
};

pub const TOOL_BUILD_COMPONENT: &str = "build_component";

/// OpenAI-compatible function definition for `build_component`.
pub fn tool_def() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": TOOL_BUILD_COMPONENT,
            "description": "Build a new WebAssembly component from Rust source and WIT \
                in Wassette's isolated builder VM, then install it in the user's Wassette \
                component store. The host asks the user to approve each phase. The Rust \
                source is the crate's entire `src/lib.rs`: implement the generated \
                `bindings` traits for the selected world and end with \
                `bindings::export!(Component with_types_in bindings);`. For example, for \
                `world tool { export answer: func() -> u32; }` write \
                `struct Component; impl bindings::Guest for Component { fn answer() -> u32 { 42 } } \
                bindings::export!(Component with_types_in bindings);`. Only `std` and crates \
                pinned by the operator are available; there is no network or Cargo.toml. \
                This conversation's tools never change: a component built here becomes \
                usable only in a NEW session that selects it.",
            "parameters": {
                "type": "object",
                "properties": {
                    "component_name": {
                        "type": "string",
                        "description": "Embedded component name, which becomes its component id, e.g. `local:weather`."
                    },
                    "world": {
                        "type": "string",
                        "description": "Name of the WIT world to build; it must be declared in `wit`."
                    },
                    "wit": {
                        "type": "string",
                        "description": "Complete WIT package text, including its `package` declaration and the world."
                    },
                    "source": {
                        "type": "string",
                        "description": "Complete Rust source for the component's `src/lib.rs`."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["tool", "acp-layer"],
                        "description": "`tool` (default) for an ordinary Wassette tool, `acp-layer` for an ACP layer."
                    },
                    "intent": {
                        "type": "string",
                        "enum": ["install-only", "expose-tools"],
                        "description": "`install-only` (default) persists the component without exposing tools. `expose-tools` also asks to expose its tools; it needs separate operator permission and does not apply to ACP layers."
                    },
                    "expected_revision": {
                        "type": "string",
                        "description": "Only to rebuild a component previously generated here: the `revision` from its build report. Omit to create a new component."
                    }
                },
                "required": ["component_name", "world", "wit", "source"]
            }
        }
    })
}

/// Map the model's arguments onto the host's `GenerationRequest` JSON. Paths,
/// compiler flags and permissions are not request fields.
pub fn request_json(args: &Value) -> Result<String, String> {
    let field = |name: &str| -> Result<&str, String> {
        args.get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("the '{name}' argument is required and must be a string"))
    };
    let kind = match args.get("kind").and_then(Value::as_str).unwrap_or("tool") {
        "tool" => "Tool",
        "acp-layer" => "AcpLayer",
        other => return Err(format!("unknown kind '{other}' (expected tool or acp-layer)")),
    };
    let intent = match args
        .get("intent")
        .and_then(Value::as_str)
        .unwrap_or("install-only")
    {
        "install-only" => "InstallOnly",
        "expose-tools" => "ExposeTools",
        other => {
            return Err(format!(
                "unknown intent '{other}' (expected install-only or expose-tools)"
            ));
        }
    };
    let target = match args.get("expected_revision").and_then(Value::as_str) {
        Some(revision) if !revision.is_empty() => {
            json!({"mode": "rebuild", "expected_revision": revision})
        }
        _ => json!({"mode": "new"}),
    };
    Ok(json!({
        "build": {
            "component_name": field("component_name")?,
            "world": field("world")?,
            "wit": field("wit")?,
            "source": field("source")?,
            "kind": kind,
        },
        "target": target,
        "intent": intent,
    })
    .to_string())
}

/// Turn the host's answer into text the model can act on.
pub fn describe(result: &Result<GenerationReport, GenerationError>) -> String {
    match result {
        Ok(report) => describe_report(report),
        Err(error) => describe_error(error),
    }
}

fn describe_report(report: &GenerationReport) -> String {
    let parsed: Value = serde_json::from_str(&report.report_json).unwrap_or(Value::Null);
    let id = parsed
        .get("component_id")
        .and_then(Value::as_str)
        .unwrap_or("<component id>");
    let revision = parsed
        .get("revision")
        .and_then(Value::as_str)
        .map(|revision| format!(" (revision `{revision}`)"))
        .unwrap_or_default();
    let new_session = format!(
        "Running ACP sessions are never hot-swapped, so this conversation's tools do not \
         change. To use its tools, the user must start a NEW ACP session that selects it with \
         `--tool {id}` (or load it in the Wassette MCP server)."
    );
    let summary = match report.disposition {
        Disposition::Installed => format!(
            "Installed component `{id}`{revision} in the Wassette component store. It is not \
             exposed as a tool. {new_session}"
        ),
        Disposition::ToolsEligible => format!(
            "Installed component `{id}`{revision}; it is eligible as an ordinary tool but \
             exports no callable functions for this session. {new_session}"
        ),
        Disposition::SessionTools => format!(
            "Installed component `{id}`{revision} and registered its tools with this host \
             session (handles: {}). {new_session}",
            report.tool_handles.join(", ")
        ),
        Disposition::LaterSelectionRequired => format!(
            "Installed ACP layer `{id}`{revision}. It is not active; the user must start a NEW \
             ACP session with `--layer {id}` to use it."
        ),
        Disposition::CommittedNotExposed => format!(
            "Installed component `{id}`{revision}, but the requested exposure did not finish. \
             Do not retry as a new build. {new_session}"
        ),
    };
    format!("{summary}\n\nReport: {}", report.report_json)
}

fn describe_error(error: &GenerationError) -> String {
    match error {
        GenerationError::Disabled => "Component generation is disabled on this Wassette host. \
            The operator must run a `wassette` built with the `component-generation` feature \
            (for example `just install`) and start `wassette acp` with \
            `--generation-config <profile>` permitting build and install."
            .to_string(),
        GenerationError::SessionNotBound => {
            "The host has no editor session bound to this request; nothing was built.".to_string()
        }
        GenerationError::PermissionDenied => "Permission denied: the user rejected an approval, \
            or the operator profile does not allow this operation (exposure and rebuilds need \
            separate operator permission). Nothing was installed. Do not retry unless the user \
            asks."
            .to_string(),
        GenerationError::Cancelled => {
            "Generation was cancelled before anything was committed.".to_string()
        }
        GenerationError::Busy => {
            "All builder slots are busy. Wait for the running build to finish, then retry."
                .to_string()
        }
        GenerationError::InvalidRequest(message) => format!(
            "Invalid request: {message}\nFix the arguments, Rust source or WIT and call \
             {TOOL_BUILD_COMPONENT} again."
        ),
        GenerationError::Stale(message) => format!(
            "The expected revision is no longer current: {message}\nUse the latest revision, \
             or omit expected_revision to create a new component."
        ),
        GenerationError::BuildFailed(message) => format!(
            "Build failed. Fix the Rust source or WIT and call {TOOL_BUILD_COMPONENT} again.\n\
             Diagnostics:\n{message}"
        ),
        GenerationError::Unavailable(message) => format!(
            "The component builder is unavailable: {message}\nThis is a host setup problem; \
             tell the user instead of retrying."
        ),
        GenerationError::Committed(report) => format!(
            "The component was committed, but the requested publication did not finish. Do \
             not retry as a new build; inspect the receipt first.\n\nReport: {}",
            report.report_json
        ),
        GenerationError::RecoveryRequired(report) => format!(
            "The store commit needs recovery. Do not retry; ask the user to inspect the store \
             operation.\n\nReport: {report}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> Value {
        json!({
            "component_name": "local:answer",
            "world": "tool",
            "wit": "package local:answer; world tool { export answer: func() -> u32; }",
            "source": "struct Component;",
        })
    }

    #[test]
    fn request_defaults_to_a_new_install_only_tool() {
        let request: Value = serde_json::from_str(&request_json(&args()).unwrap()).unwrap();
        assert_eq!(
            request,
            json!({
                "build": {
                    "component_name": "local:answer",
                    "world": "tool",
                    "wit": "package local:answer; world tool { export answer: func() -> u32; }",
                    "source": "struct Component;",
                    "kind": "Tool",
                },
                "target": {"mode": "new"},
                "intent": "InstallOnly",
            })
        );
    }

    #[test]
    fn request_maps_kind_intent_and_rebuild() {
        let mut args = args();
        args["kind"] = json!("acp-layer");
        args["intent"] = json!("expose-tools");
        args["expected_revision"] = json!("rev-1");
        let request: Value = serde_json::from_str(&request_json(&args).unwrap()).unwrap();
        assert_eq!(request["build"]["kind"], "AcpLayer");
        assert_eq!(request["intent"], "ExposeTools");
        assert_eq!(
            request["target"],
            json!({"mode": "rebuild", "expected_revision": "rev-1"})
        );
    }

    #[test]
    fn request_rejects_missing_and_unknown_arguments() {
        let mut missing = args();
        missing.as_object_mut().unwrap().remove("source");
        assert!(request_json(&missing).unwrap_err().contains("'source'"));
        let mut kind = args();
        kind["kind"] = json!("provider");
        assert!(request_json(&kind).unwrap_err().contains("unknown kind"));
    }

    #[test]
    fn reports_point_to_a_new_session() {
        let text = describe(&Ok(GenerationReport {
            report_json: json!({"component_id": "local:answer", "revision": "r1"}).to_string(),
            disposition: Disposition::Installed,
            tool_handles: Vec::new(),
        }));
        assert!(text.contains("`local:answer` (revision `r1`)"), "{text}");
        assert!(text.contains("NEW ACP session"), "{text}");
        assert!(text.contains("--tool local:answer"), "{text}");
    }

    #[test]
    fn errors_are_actionable() {
        assert!(describe(&Err(GenerationError::Disabled)).contains("--generation-config"));
        let text = describe(&Err(GenerationError::BuildFailed("E0425".into())));
        assert!(text.contains("E0425") && text.contains("again"), "{text}");
    }
}
