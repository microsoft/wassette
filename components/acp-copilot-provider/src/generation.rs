// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Copilot model adapter for Wassette's host-owned component-generation import.
//!
//! The host advertises nothing here: it notifies this provider through an
//! internal config option when generation is available, and remains the
//! authority on every request. It
//! prompts the editor for build and install, so this
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
                pinned by the host are available; there is no network or Cargo.toml. \
                Newly built tools are available in this session immediately; ACP layers still \
                require a new session.",
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
    let summary = match report.disposition {
        Disposition::Installed => format!(
            "Installed component `{id}`{revision} in the Wassette component store."
        ),
        Disposition::ToolsEligible => format!(
            "Installed component `{id}`{revision}; it is eligible as an ordinary tool but \
             exports no callable functions."
        ),
        Disposition::SessionTools => format!(
            "Installed component `{id}`{revision} and registered its tools with this session \
             (handles: {}). You can call them now.",
            report.tool_handles.join(", ")
        ),
        Disposition::LaterSelectionRequired => format!(
            "Installed ACP layer `{id}`{revision}. Layers are not hot-swapped into a running \
             session; select it when starting a new ACP session."
        ),
        Disposition::CommittedNotExposed => format!(
            "Installed component `{id}`{revision}, but session publication did not finish. \
             Do not retry as a new build."
        ),
    };
    format!("{summary}\n\nReport: {}", report.report_json)
}

fn describe_error(error: &GenerationError) -> String {
    match error {
        GenerationError::Disabled => "Component generation is unavailable on this Wassette host. \
            Install with `just install` and place the builder image at \
            ~/.local/share/wassette/builder/rust-initrd.cpio."
            .to_string(),
        GenerationError::SessionNotBound => {
            "The host has no editor session bound to this request; nothing was built.".to_string()
        }
        GenerationError::PermissionDenied => "Permission denied: the user rejected an approval, \
            or this operation is unavailable (rebuilds are disabled by default). \
            Nothing was installed. Do not retry unless the user \
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
    fn request_defaults_to_a_new_tool() {
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
            })
        );
    }

    #[test]
    fn request_maps_kind_and_rebuild() {
        let mut args = args();
        args["kind"] = json!("acp-layer");
        args["expected_revision"] = json!("rev-1");
        let request: Value = serde_json::from_str(&request_json(&args).unwrap()).unwrap();
        assert_eq!(request["build"]["kind"], "AcpLayer");
        assert_eq!(
            request["target"],
            json!({"mode": "rebuild", "expected_revision": "rev-1"})
        );
    }

    #[test]
    fn layer_generation_has_no_tool_exposure_intent() {
        let mut args = args();
        args["kind"] = json!("acp-layer");
        let request: Value = serde_json::from_str(&request_json(&args).unwrap()).unwrap();
        assert_eq!(request["build"]["kind"], "AcpLayer");
        assert!(request.get("intent").is_none());
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
        assert!(describe(&Err(GenerationError::Disabled)).contains("rust-initrd.cpio"));
        let text = describe(&Err(GenerationError::BuildFailed("E0425".into())));
        assert!(text.contains("E0425") && text.contains("again"), "{text}");
    }
}
