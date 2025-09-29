#![allow(clippy::all)]

#[allow(warnings)]
mod bindings;

use bindings::exports::local::context7::context7::{DocsResponse, Guest, LibraryResult, SearchResponse};
use bindings::wasi::cli::environment;
use bindings::wasi::http::outgoing_handler;
use bindings::wasi::http::types::{self, Fields, IncomingResponse, Method, OutgoingRequest, Scheme};
use bindings::wasi::io::streams::StreamError;
use serde_json::Value;
use urlencoding::encode;

const CONTEXT7_HOST: &str = "context7.com";
const BASE_PATH: &str = "/api";
const DEFAULT_MINIMUM_TOKENS: u32 = 10_000;
const DEFAULT_USER_AGENT: &str = "context7-wasm-component/1.0.0";
const NO_CONTENT_MESSAGES: [&str; 2] = ["No content available", "No context data available"];

struct Context7;

impl Guest for Context7 {
    fn resolve_library_id(library_name: String) -> SearchResponse {
        let trimmed = library_name.trim();
        if trimmed.is_empty() {
            return SearchResponse {
                success: false,
                results: Vec::new(),
                error: Some("Library name is required".to_string()),
            };
        }

        let encoded_query = encode(trimmed);
        let endpoint = format!("/v1/search?query={encoded_query}");
        match make_request(Method::Get, &endpoint, None, get_api_key().as_deref()) {
            Ok(body) => match parse_search_response(&body) {
                Ok(response) => response,
                Err(message) => SearchResponse {
                    success: false,
                    results: Vec::new(),
                    error: Some(message),
                },
            },
            Err(message) => SearchResponse {
                success: false,
                results: Vec::new(),
                error: Some(message),
            },
        }
    }

    fn get_library_docs(
        context7_compatible_library_id: String,
        topic: Option<String>,
        tokens: Option<u32>,
    ) -> DocsResponse {
        let trimmed = context7_compatible_library_id.trim();
        if trimmed.is_empty() {
            return DocsResponse {
                success: false,
                content: None,
                error: Some("Context7 compatible library ID is required".to_string()),
            };
        }

        let library_id = trimmed.trim_start_matches('/').to_string();
        if library_id.is_empty() {
            return DocsResponse {
                success: false,
                content: None,
                error: Some("Context7 compatible library ID is required".to_string()),
            };
        }

        let token_count = tokens
            .map(|value| value.max(DEFAULT_MINIMUM_TOKENS))
            .unwrap_or(DEFAULT_MINIMUM_TOKENS);

        let mut parameters = vec![format!("tokens={token_count}"), "type=txt".to_string()];

        if let Some(topic_value) = topic.as_ref().map(|value| value.trim()).filter(|v| !v.is_empty()) {
            parameters.push(format!("topic={}", encode(topic_value)));
        }

        let endpoint = format!("/v1/{}?{}", library_id, parameters.join("&"));
        match make_request(Method::Get, &endpoint, None, get_api_key().as_deref()) {
            Ok(body) => {
                let body_trimmed = body.trim();
                if body_trimmed.is_empty() || NO_CONTENT_MESSAGES.iter().any(|msg| body_trimmed == *msg) {
                    DocsResponse {
                        success: false,
                        content: None,
                        error: Some("No documentation available for this library".to_string()),
                    }
                } else {
                    DocsResponse {
                        success: true,
                        content: Some(body),
                        error: None,
                    }
                }
            }
            Err(message) => DocsResponse {
                success: false,
                content: None,
                error: Some(message),
            },
        }
    }
}

fn make_request(
    method: Method,
    endpoint: &str,
    body: Option<&str>,
    api_key: Option<&str>,
) -> Result<String, String> {
    let headers = create_headers(api_key, body.is_some())?;
    let request = OutgoingRequest::new(headers);

    request
        .set_method(&method)
        .map_err(|_| "Failed to set HTTP method".to_string())?;
    request
        .set_scheme(Some(&Scheme::Https))
        .map_err(|_| "Failed to set scheme".to_string())?;
    request
        .set_authority(Some(CONTEXT7_HOST))
        .map_err(|_| "Failed to set authority".to_string())?;

    let mut path = String::with_capacity(BASE_PATH.len() + endpoint.len() + 1);
    path.push_str(BASE_PATH);
    if endpoint.starts_with('/') {
        path.push_str(endpoint);
    } else {
        path.push('/');
        path.push_str(endpoint);
    }

    request
        .set_path_with_query(Some(&path))
        .map_err(|_| "Failed to set path".to_string())?;

    if let Some(payload) = body {
        let outgoing_body = request
            .body()
            .map_err(|_| "Failed to acquire request body".to_string())?;
        write_body(outgoing_body, payload.as_bytes())?;
    }

    let future = outgoing_handler::handle(request, None)
        .map_err(|error| format!("HTTP request failed: {error:?}"))?;
    let response = wait_for_response(&future)?;
    let status = response.status();

    let body = read_response_body(response)?;
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(if body.trim().is_empty() {
            format!("HTTP {status}")
        } else {
            format!("HTTP {status}: {}", body)
        })
    }
}

fn create_headers(api_key: Option<&str>, has_body: bool) -> Result<Fields, String> {
    let headers = Fields::new();
    append_header(&headers, "user-agent", DEFAULT_USER_AGENT)?;
    append_header(&headers, "accept", "*/*")?;

    if has_body {
        append_header(&headers, "content-type", "application/json")?;
    }

    if let Some(key) = api_key.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }) {
        append_header(&headers, "context7-api-key", key)?;
    }

    Ok(headers)
}

fn append_header(headers: &Fields, key: &str, value: &str) -> Result<(), String> {
    let normalized_key = key.to_ascii_lowercase();
    let values = vec![value.as_bytes().to_vec()];
    headers
        .set(&normalized_key, &values)
        .map_err(|error| format!("Failed to set header {normalized_key}: {:?}", error))
}

fn write_body(body: types::OutgoingBody, bytes: &[u8]) -> Result<(), String> {
    let stream = body
        .write()
        .map_err(|_| "Failed to acquire request stream".to_string())?;
    stream
        .blocking_write_and_flush(bytes)
        .map_err(describe_stream_error)?;
    types::OutgoingBody::finish(body, None)
        .map_err(|error| format!("Failed to finalize request body: {error:?}"))
}

fn wait_for_response(future: &types::FutureIncomingResponse) -> Result<IncomingResponse, String> {
    loop {
        if let Some(result) = future.get() {
            return match result {
                Ok(Ok(response)) => Ok(response),
                Ok(Err(error)) => Err(format!("HTTP error: {error:?}")),
                Err(_) => Err("HTTP response already consumed".to_string()),
            };
        }

        let pollable = future.subscribe();
        pollable.block();
    }
}

fn wait_for_trailers(future: types::FutureTrailers) -> Result<(), String> {
    loop {
        if let Some(result) = future.get() {
            return match result {
                Ok(Ok(Some(trailers))) => {
                    drop(trailers);
                    Ok(())
                }
                Ok(Ok(None)) => Ok(()),
                Ok(Err(error)) => Err(format!("HTTP trailers error: {error:?}")),
                Err(_) => Err("HTTP trailers already consumed".to_string()),
            };
        }

        let pollable = future.subscribe();
        pollable.block();
    }
}

fn read_response_body(response: IncomingResponse) -> Result<String, String> {
    let incoming_body = response
        .consume()
        .map_err(|_| "Failed to consume response body".to_string())?;

    let mut bytes = Vec::new();
    {
        let stream = incoming_body
            .stream()
            .map_err(|_| "Failed to open response stream".to_string())?;

        loop {
            match stream.blocking_read(8 * 1024) {
                Ok(chunk) if chunk.is_empty() => break,
                Ok(chunk) => bytes.extend_from_slice(&chunk),
                Err(StreamError::Closed) => break,
                Err(StreamError::LastOperationFailed(error)) => {
                    let message = error.to_debug_string();
                    return Err(format!("Failed to read response body: {message}"));
                }
            }
        }
    }

    wait_for_trailers(types::IncomingBody::finish(incoming_body))?;

    String::from_utf8(bytes).map_err(|error| format!("Response body is not valid UTF-8: {error}"))
}

fn parse_search_response(body: &str) -> Result<SearchResponse, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| {
        format!("Failed to parse response ({} bytes): {error}", body.len())
    })?;
    let results = value
        .get("results")
        .and_then(|results| results.as_array())
        .ok_or_else(|| "Invalid response format from Context7 API".to_string())?;

    let mut mapped_results = Vec::with_capacity(results.len());
    for entry in results {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let name = entry
            .get("name")
            .or_else(|| entry.get("title"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let description = entry
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let code_snippets = entry
            .get("codeSnippets")
            .or_else(|| entry.get("totalSnippets"))
            .and_then(Value::as_u64)
            .and_then(|value| value.try_into().ok())
            .unwrap_or_default();
        let trust_score = entry
            .get("trustScore")
            .or_else(|| entry.get("trust_score"))
            .and_then(Value::as_u64)
            .and_then(|value| value.try_into().ok())
            .unwrap_or_default();

        let versions = entry
            .get("versions")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(ToString::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        mapped_results.push(LibraryResult {
            id,
            name,
            description,
            code_snippets,
            trust_score,
            versions,
        });
    }

    Ok(SearchResponse {
        success: true,
        results: mapped_results,
        error: None,
    })
}

fn get_api_key() -> Option<String> {
    environment::get_environment()
        .into_iter()
        .find(|(key, _)| key == "CONTEXT7_API_KEY")
        .map(|(_, value)| value)
}

fn describe_stream_error(error: StreamError) -> String {
    match error {
        StreamError::LastOperationFailed(inner) => {
            let message = inner.to_debug_string();
            format!("Stream error: {message}")
        }
        StreamError::Closed => "Stream closed".to_string(),
    }
}

bindings::export!(Context7 with_types_in bindings);
