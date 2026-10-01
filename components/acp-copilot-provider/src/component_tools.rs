// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Model-facing Wassette tools backed by the host's
//! `wassette:component-tools/tools` broker.
//!
//! The host decides which components are exposed to a session; this provider
//! only reads that catalog and calls back through the same import. Nothing is
//! advertised unless the broker currently lists it, so a host that exposes no
//! components (the default) leaves the model with the built-in tools alone.
//! The host also owns the permission prompt and the `tool_call` session
//! updates for these calls, so this module emits no UI of its own and never
//! sees — or relays — the grants behind a tool.
//!
//! Every call is addressed by the handle the host issued for the exact tool
//! revision this session admitted. A handle the host rejects as stale or
//! unknown is never re-resolved by name: that would invoke whatever code now
//! answers to that name, defeating the revision pinning the handle exists to
//! enforce. The failure is reported to the model instead, and the next round
//! re-reads the catalog.

use serde_json::{Value, json};

use crate::bindings::wassette::component_tools::tools::{
    self, CatalogResult, ToolDescriptor, ToolError, ToolResult,
};

/// Max length of an OpenAI-compatible function name.
const MAX_NAME: usize = 64;

/// Hex digits of the descriptor-key digest appended when a catalog name is
/// unusable as-is.
const DIGEST_LEN: usize = 12;

/// One catalog entry, ready to advertise to the model and call back by handle.
#[derive(Clone, Debug, PartialEq)]
pub struct BrokerTool {
    /// Function name shown to the model.
    pub name: String,
    /// Opaque broker handle used to invoke this exact tool revision.
    pub handle: String,
    /// OpenAI-compatible function definition.
    pub def: Value,
}

/// The tools the host currently exposes to this session.
#[derive(Default)]
pub struct Broker {
    loaded: bool,
    generation: u64,
    tools: Vec<BrokerTool>,
}

impl Broker {
    /// Refresh the catalog from the host. Returns whether the advertised set
    /// changed (so the caller can rebuild its tool array) plus a message to
    /// surface when the host could not answer, or when its catalog could not
    /// be turned into callable function definitions.
    ///
    /// The first call always reports a change so the caller builds its tool
    /// array once, even when the host exposes nothing.
    pub async fn refresh(&mut self, reserved: &[&str]) -> (bool, Option<String>) {
        let known = self.loaded.then_some(self.generation);
        let first = !self.loaded;
        match tools::list_tools(known).await {
            Ok(CatalogResult::Unchanged(generation)) => {
                self.generation = generation;
                (first, None)
            }
            Ok(CatalogResult::Changed(catalog)) => {
                self.loaded = true;
                self.generation = catalog.generation;
                match build(catalog.tools, reserved) {
                    Ok(next) => {
                        let changed = first || next != self.tools;
                        self.tools = next;
                        (changed, None)
                    }
                    // A catalog we cannot represent faithfully is a failure,
                    // not a tool set to approximate: advertise nothing and
                    // say why.
                    Err(message) => {
                        let changed = first || !self.tools.is_empty();
                        self.tools.clear();
                        (changed, Some(message))
                    }
                }
            }
            Err(error) => {
                // Keep whatever we last advertised: a transient broker failure
                // should not silently drop tools mid-turn.
                self.loaded = true;
                (first, Some(describe_error(&error)))
            }
        }
    }

    /// The OpenAI-compatible function definitions to advertise this round.
    pub fn defs(&self) -> impl Iterator<Item = &Value> {
        self.tools.iter().map(|tool| &tool.def)
    }

    /// A broker holding a fixed catalog, for tests that exercise the tool
    /// array without a host.
    #[cfg(test)]
    pub fn from_tools(tools: Vec<BrokerTool>) -> Self {
        Self {
            loaded: true,
            generation: 1,
            tools,
        }
    }

    pub fn find(&self, name: &str) -> Option<&BrokerTool> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    /// Run a broker tool and return the text to feed back to the model. The
    /// host prompts the user and reports the call to the editor itself.
    ///
    /// Calls go through the admitted handle only. When the host reports the
    /// handle as stale or unknown, the pinned revision is gone: report that
    /// and drop the cached catalog so the next round re-lists. Re-resolving
    /// the call by name would run whatever replaced it, which this session
    /// never admitted.
    pub async fn call(&mut self, tool: &BrokerTool, arguments_json: &str) -> String {
        match tools::call_tool(tool.handle.clone(), arguments_json.to_string()).await {
            Ok(result) => describe_result(&result),
            Err(error) => {
                if matches!(error, ToolError::Stale(_) | ToolError::NotFound(_)) {
                    self.loaded = false;
                }
                describe_error(&error)
            }
        }
    }
}

/// Turn catalog descriptors into advertisable function definitions.
///
/// A catalog name that is unusable as an API function name — empty after
/// sanitizing, shadowing one of this provider's own tools, or colliding with
/// another entry — is given a deterministic suffix derived from the
/// descriptor's component and export, so the same catalog always yields the
/// same names. Nothing is dropped: a catalog that still cannot be named
/// uniquely, or that carries a schema we cannot represent, fails as a whole.
pub fn build(
    mut descriptors: Vec<ToolDescriptor>,
    reserved: &[&str],
) -> Result<Vec<BrokerTool>, String> {
    descriptors.sort_by(|a, b| {
        (&a.component_id, &a.export_name, &a.name).cmp(&(&b.component_id, &b.export_name, &b.name))
    });
    let mut built: Vec<BrokerTool> = Vec::new();
    for descriptor in descriptors {
        let name = unique_name(&descriptor, reserved, &built)?;
        let parameters = parameters(&descriptor.input_schema).map_err(|message| {
            format!(
                "the Wassette tool `{}` (component `{}`, export `{}`) has an unusable input schema: \
                 {message}",
                descriptor.name, descriptor.component_id, descriptor.export_name
            )
        })?;
        let def = json!({
            "type": "function",
            "function": {
                "name": name,
                "description": descriptor.description.clone().unwrap_or_else(|| {
                    format!(
                        "The `{}` export of Wassette component `{}`.",
                        descriptor.export_name, descriptor.component_id
                    )
                }),
                "parameters": parameters,
            }
        });
        built.push(BrokerTool {
            name,
            handle: descriptor.handle,
            def,
        });
    }
    Ok(built)
}

/// The API-safe name to advertise this descriptor under: its sanitized
/// catalog name when that is usable, else that name suffixed with a digest of
/// the descriptor's key.
fn unique_name(
    descriptor: &ToolDescriptor,
    reserved: &[&str],
    built: &[BrokerTool],
) -> Result<String, String> {
    let usable = |name: &str| {
        !name.is_empty() && !reserved.contains(&name) && !built.iter().any(|tool| tool.name == name)
    };
    let base = sanitize_name(&descriptor.name);
    if usable(&base) {
        return Ok(base);
    }
    let digest = key_digest(descriptor);
    let room = MAX_NAME - DIGEST_LEN - 1;
    let stem = truncate(&base, room);
    let qualified = if stem.is_empty() {
        format!("tool_{digest}")
    } else {
        format!("{stem}_{digest}")
    };
    if usable(&qualified) {
        return Ok(qualified);
    }
    Err(format!(
        "the Wassette tool `{}` (component `{}`, export `{}`) cannot be given a unique \
         function name; ask the operator to expose it under a distinct name",
        descriptor.name, descriptor.component_id, descriptor.export_name
    ))
}

/// Stable digest of the descriptor's identity — its component and export, not
/// its handle, which the host reissues. FNV-1a keeps this dependency-free.
fn key_digest(descriptor: &ToolDescriptor) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in descriptor
        .component_id
        .as_bytes()
        .iter()
        .chain(b"\x1f")
        .chain(descriptor.export_name.as_bytes())
    {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")[..DIGEST_LEN].to_string()
}

/// Map the catalog name onto the characters chat completions accept for a
/// function name, keeping it within the length limit.
fn sanitize_name(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => c,
            _ => '_',
        })
        .collect();
    truncate(mapped.trim_matches('_'), MAX_NAME)
}

fn truncate(name: &str, max: usize) -> String {
    name.chars()
        .take(max)
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// The tool's JSON Schema as the chat completions API needs it: an explicit
/// object schema with an explicit property set. Anything else is reported
/// rather than replaced with a permissive default, which would invite the
/// model to call the tool with arguments the component will reject.
fn parameters(input_schema: &str) -> Result<Value, String> {
    let schema: Value = serde_json::from_str(input_schema)
        .map_err(|error| format!("it is not valid JSON ({error})"))?;
    let Value::Object(fields) = &schema else {
        return Err("it is not a JSON Schema object".to_string());
    };
    match fields.get("type") {
        Some(Value::String(kind)) if kind == "object" => {}
        Some(kind) => {
            return Err(format!(
                "its top-level `type` is {kind}, but a tool's arguments must be an object"
            ));
        }
        None => return Err("it declares no top-level `type`".to_string()),
    }
    match fields.get("properties") {
        Some(Value::Object(_)) => {}
        Some(_) => return Err("its `properties` is not an object".to_string()),
        None => return Err("it declares no `properties`".to_string()),
    }
    Ok(schema)
}

/// Render a successful call for the model: the host's text, plus its
/// structured payload when it returned one.
pub fn describe_result(result: &ToolResult) -> String {
    match &result.structured {
        Some(structured) if !structured.is_empty() => {
            if result.text.is_empty() {
                structured.clone()
            } else {
                format!("{}\n\nStructured result: {structured}", result.text)
            }
        }
        _ => result.text.clone(),
    }
}

/// Render a broker failure as something the model can act on. Host policy and
/// permission decisions are reported as outcomes, never as the grants behind
/// them.
pub fn describe_error(error: &ToolError) -> String {
    match error {
        ToolError::NotFound(message) => format!(
            "Error: that tool is no longer available in this session: {message}. It was not \
             run. The tool list is refreshed for the next step; do not assume a tool with the \
             same name is the same tool."
        ),
        ToolError::Ambiguous(candidates) => format!(
            "Error: the tool name is ambiguous ({}). Call the specific tool you want.",
            candidates.join(", ")
        ),
        ToolError::Stale(message) => format!(
            "Error: this tool's version changed since it was offered: {message}. Nothing ran, \
             and the replacement was NOT called in its place. The tool list is refreshed for \
             the next step; re-read it before trying again."
        ),
        ToolError::InvalidArguments(message) => format!(
            "Error: invalid arguments: {message}. Fix the arguments against the tool's schema \
             and call it again."
        ),
        ToolError::PermissionDenied => {
            "The user denied permission to run this tool. Do not retry unless the user asks."
                .to_string()
        }
        ToolError::PolicyDenied(message) => format!(
            "The host's policy refused this call: {message}. Do not retry; tell the user what \
             you were trying to do."
        ),
        ToolError::ExecutionFailed(message) => {
            format!("The tool failed while running: {message}")
        }
        ToolError::Unavailable(message) => format!(
            "Wassette tools are unavailable: {message}. This is a host problem; tell the user \
             instead of retrying."
        ),
        ToolError::SessionNotBound => {
            "The host has no editor session bound to this request; the tool did not run."
                .to_string()
        }
        ToolError::Busy => {
            "All tool execution slots are busy. Wait for the running tools to finish, then retry."
                .to_string()
        }
        ToolError::Cancelled => "The tool call was cancelled before it finished.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(name: &str, handle: &str, input_schema: &str) -> ToolDescriptor {
        ToolDescriptor {
            handle: handle.to_string(),
            name: name.to_string(),
            component_id: "local:example".to_string(),
            export_name: "run".to_string(),
            description: Some(format!("does {name}")),
            input_schema: input_schema.to_string(),
            output_schema: None,
        }
    }

    const OBJECT: &str = r#"{"type":"object","properties":{}}"#;

    #[test]
    fn descriptors_become_openai_function_defs() {
        let schema =
            r#"{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}"#;
        let built = build(vec![descriptor("get-weather", "h1", schema)], &[]).unwrap();
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].handle, "h1");
        assert_eq!(built[0].name, "get-weather");
        assert_eq!(
            built[0].def,
            json!({
                "type": "function",
                "function": {
                    "name": "get-weather",
                    "description": "does get-weather",
                    "parameters": {
                        "type": "object",
                        "properties": {"city": {"type": "string"}},
                        "required": ["city"],
                    }
                }
            })
        );
    }

    #[test]
    fn colliding_names_are_qualified_not_dropped() {
        let mut second = descriptor("local:weather/get weather", "h2", OBJECT);
        second.component_id = "local:other".to_string();
        let built = build(
            vec![
                descriptor("local:weather/get weather", "h1", OBJECT),
                second,
            ],
            &[],
        )
        .unwrap();
        assert_eq!(built.len(), 2, "no entry may be dropped");
        assert_eq!(built[0].name, "local_weather_get_weather");
        assert!(
            built[1].name.starts_with("local_weather_get_weather_"),
            "{}",
            built[1].name
        );
        assert_eq!(built[1].handle, "h2");
    }

    #[test]
    fn reserved_and_empty_names_are_qualified_not_dropped() {
        let built = build(
            vec![
                descriptor("read_text_file", "h1", OBJECT),
                descriptor("///", "h2", OBJECT),
            ],
            &["read_text_file"],
        )
        .unwrap();
        assert_eq!(built.len(), 2);
        let reserved = built.iter().find(|tool| tool.handle == "h1").unwrap();
        let empty = built.iter().find(|tool| tool.handle == "h2").unwrap();
        assert!(
            reserved.name.starts_with("read_text_file_"),
            "{}",
            reserved.name
        );
        assert_ne!(reserved.name, "read_text_file");
        assert!(empty.name.starts_with("tool_"), "{}", empty.name);
    }

    #[test]
    fn qualified_names_are_deterministic_and_api_safe() {
        let catalog = || {
            vec![
                descriptor("dup", "h1", OBJECT),
                ToolDescriptor {
                    component_id: "local:other".to_string(),
                    ..descriptor("dup", "h2", OBJECT)
                },
            ]
        };
        let first = build(catalog(), &[]).unwrap();
        let second = build(catalog(), &[]).unwrap();
        assert_eq!(first, second, "the same catalog must yield the same names");
        let mut reversed = catalog();
        reversed.reverse();
        assert_eq!(
            first,
            build(reversed, &[]).unwrap(),
            "catalog iteration order must not change advertised names"
        );
        for tool in &first {
            assert!(tool.name.len() <= MAX_NAME, "{}", tool.name);
            assert!(
                tool.name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{}",
                tool.name
            );
        }
        // A different export under the same name gets a different suffix.
        let other = build(
            vec![
                descriptor("dup", "h1", OBJECT),
                ToolDescriptor {
                    export_name: "elsewhere".to_string(),
                    ..descriptor("dup", "h2", OBJECT)
                },
            ],
            &[],
        )
        .unwrap();
        assert_ne!(first[1].name, other[1].name);
    }

    #[test]
    fn unusable_schemas_fail_the_catalog_instead_of_defaulting() {
        for schema in [
            "",
            "not json",
            "[]",
            r#"{"type":"string"}"#,
            r#"{"properties":{}}"#,
            r#"{"type":"object"}"#,
            r#"{"type":"object","properties":[]}"#,
        ] {
            let error = build(vec![descriptor("t", "h", schema)], &[]).unwrap_err();
            assert!(
                error.contains("unusable input schema") && error.contains("local:example"),
                "{schema}: {error}"
            );
        }
    }

    #[test]
    fn results_carry_text_and_structured_output() {
        let result = ToolResult {
            tool_call_id: "c1".to_string(),
            text: "sunny".to_string(),
            structured: Some(r#"{"temp":21}"#.to_string()),
        };
        let text = describe_result(&result);
        assert!(
            text.contains("sunny") && text.contains(r#"{"temp":21}"#),
            "{text}"
        );
        assert_eq!(
            describe_result(&ToolResult {
                structured: None,
                ..result
            }),
            "sunny"
        );
    }

    #[test]
    fn errors_are_actionable_and_leak_no_grants() {
        let denied = describe_error(&ToolError::PermissionDenied);
        assert!(denied.contains("denied permission"), "{denied}");
        assert!(describe_error(&ToolError::Busy).contains("retry"));
        let invalid = describe_error(&ToolError::InvalidArguments("missing city".into()));
        assert!(
            invalid.contains("missing city") && invalid.contains("schema"),
            "{invalid}"
        );
        let policy = describe_error(&ToolError::PolicyDenied("storage write".into()));
        assert!(
            policy.contains("storage write") && policy.contains("Do not retry"),
            "{policy}"
        );
    }

    #[test]
    fn stale_and_missing_handles_report_that_nothing_was_substituted() {
        let stale = describe_error(&ToolError::Stale("revision changed".into()));
        assert!(stale.contains("revision changed"), "{stale}");
        assert!(stale.contains("NOT called in its place"), "{stale}");
        let missing = describe_error(&ToolError::NotFound("unknown handle".into()));
        assert!(missing.contains("was not run"), "{missing}");
    }
}
