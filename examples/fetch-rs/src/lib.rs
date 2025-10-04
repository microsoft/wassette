// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use brotli::Decompressor;
use flate2::read::{GzDecoder, ZlibDecoder};
use http::StatusCode as HttpStatusCode;
use mime::Mime;
use serde::Serialize;
use serde_json::Value;
use spin_sdk::http::{Method, Request, Response};
use std::borrow::Cow;
use std::collections::HashSet;
use std::io::Read;
use std::time::Instant;
use url::Url;

mod bindings;

use bindings::Guest;

struct Component;

const DEFAULT_MAX_REDIRECTS: usize = 5;
const DEFAULT_MAX_TEXT_BYTES: usize = 128 * 1024;
const DEFAULT_MAX_BINARY_BYTES: usize = 256 * 1024;
const DEFAULT_USER_AGENT: &str = "mcp-fetch/1.0 (+https://github.com/microsoft/wassette)";
const DEFAULT_BROTLI_BUFFER_SIZE: usize = 4096;
const ENV_MAX_REDIRECTS: &str = "FETCH_MAX_REDIRECTS";
const ENV_MAX_TEXT_BYTES: &str = "FETCH_MAX_TEXT_BYTES";
const ENV_MAX_BINARY_BYTES: &str = "FETCH_MAX_BINARY_BYTES";
const ENV_TIMEOUT_MS: &str = "FETCH_TIMEOUT_MS";
const ENV_USER_AGENT: &str = "FETCH_USER_AGENT";

#[derive(Clone)]
struct FetchOptions {
    max_redirects: usize,
    max_text_bytes: usize,
    max_binary_bytes: usize,
    timeout_ms: Option<u64>,
    user_agent: Option<String>,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            max_redirects: DEFAULT_MAX_REDIRECTS,
            max_text_bytes: DEFAULT_MAX_TEXT_BYTES,
            max_binary_bytes: DEFAULT_MAX_BINARY_BYTES,
            timeout_ms: None,
            user_agent: None,
        }
    }
}

impl FetchOptions {
    fn from_env() -> Self {
        let mut options = Self::default();

        if let Ok(value) = std::env::var(ENV_MAX_REDIRECTS) {
            if let Ok(parsed) = value.parse::<usize>() {
                options.max_redirects = parsed;
            }
        }

        if let Ok(value) = std::env::var(ENV_MAX_TEXT_BYTES) {
            if let Ok(parsed) = value.parse::<usize>() {
                options.max_text_bytes = parsed;
            }
        }

        if let Ok(value) = std::env::var(ENV_MAX_BINARY_BYTES) {
            if let Ok(parsed) = value.parse::<usize>() {
                options.max_binary_bytes = parsed;
            }
        }

        if let Ok(value) = std::env::var(ENV_TIMEOUT_MS) {
            if let Ok(parsed) = value.parse::<u64>() {
                options.timeout_ms = Some(parsed);
            }
        }

        if let Ok(value) = std::env::var(ENV_USER_AGENT) {
            if !value.trim().is_empty() {
                options.user_agent = Some(value);
            }
        }

        options
    }

    fn max_redirects(&self) -> usize {
        self.max_redirects
    }

    fn user_agent(&self) -> &str {
        self.user_agent
            .as_deref()
            .unwrap_or(DEFAULT_USER_AGENT)
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }

    fn max_text_bytes(&self) -> usize {
        self.max_text_bytes
    }

    fn max_binary_bytes(&self) -> usize {
        self.max_binary_bytes
    }

    fn brotli_buffer(&self) -> usize {
        DEFAULT_BROTLI_BUFFER_SIZE
    }
}

impl Guest for Component {
    fn fetch(url: String) -> Result<String, String> {
        spin_executor::run(async move {
            let options = FetchOptions::from_env();
            match fetch_impl(url, options).await {
                Ok(success) => serde_json::to_string_pretty(&success).map_err(|e| e.to_string()),
                Err(error) => match serde_json::to_string_pretty(&error) {
                    Ok(json) => Err(json),
                    Err(serde_err) => Err(
                        serde_json::json!({
                            "error": format!("Failed to serialize fetch error: {}", serde_err)
                        })
                        .to_string(),
                    ),
                },
            }
        })
    }
}

#[derive(Serialize)]
struct FetchSuccess {
    final_url: String,
    status: u16,
    status_text: Option<String>,
    headers: Vec<HeaderEntry>,
    content_type: Option<String>,
    content_encoding: Option<String>,
    redirect_chain: Vec<RedirectHop>,
    body: Body,
    warnings: Vec<String>,
    metrics: Metrics,
}

#[derive(Serialize)]
struct FetchError {
    error: String,
    url: String,
    status: Option<u16>,
    status_text: Option<String>,
    headers: Vec<HeaderEntry>,
    content_type: Option<String>,
    content_encoding: Option<String>,
    redirect_chain: Vec<RedirectHop>,
    body: Option<Body>,
    warnings: Vec<String>,
    metrics: Option<Metrics>,
}

#[derive(Serialize, Clone)]
struct HeaderEntry {
    name: String,
    value: String,
}

#[derive(Serialize, Clone)]
struct RedirectHop {
    url: String,
    status: u16,
    location: String,
}

#[derive(Serialize, Clone, Copy)]
struct Metrics {
    elapsed_ms: u128,
    decoded_body_bytes: usize,
}

#[derive(Serialize)]
#[serde(tag = "format", rename_all = "snake_case")]
enum Body {
    Empty,
    Json {
        size: usize,
        truncated: bool,
        value: Value,
    },
    Text {
        size: usize,
        truncated: bool,
        encoding: String,
        content: String,
    },
    Binary {
        size: usize,
        truncated: bool,
        encoding: String,
        base64: String,
    },
}

impl Metrics {
    fn from_elapsed(duration: std::time::Duration, decoded_body_bytes: usize) -> Self {
        Self {
            elapsed_ms: duration.as_millis(),
            decoded_body_bytes,
        }
    }
}

impl FetchError {
    fn invalid_url(url: String, cause: String) -> Self {
        Self {
            error: format!("Invalid URL: {}", cause),
            url,
            status: None,
            status_text: None,
            headers: Vec::new(),
            content_type: None,
            content_encoding: None,
            redirect_chain: Vec::new(),
            body: None,
            warnings: Vec::new(),
            metrics: None,
        }
    }

    fn network(url: String, redirect_chain: Vec<RedirectHop>, cause: String, metrics: Metrics) -> Self {
        Self {
            error: format!("Network error: {}", cause),
            url,
            status: None,
            status_text: None,
            headers: Vec::new(),
            content_type: None,
            content_encoding: None,
            redirect_chain,
            body: None,
            warnings: Vec::new(),
            metrics: Some(metrics),
        }
    }

    fn redirect_limit(
        url: String,
        mut redirect_chain: Vec<RedirectHop>,
        status: u16,
        status_text: Option<String>,
        headers: Vec<HeaderEntry>,
        metrics: Metrics,
        max_redirects: usize,
    ) -> Self {
        if let Some(last) = redirect_chain.last_mut() {
            last.status = status;
        }

        Self {
            error: format!("Redirect limit of {} exceeded", max_redirects),
            url,
            status: Some(status),
            status_text,
            headers,
            content_type: None,
            content_encoding: None,
            redirect_chain,
            body: None,
            warnings: vec!["Too many redirects".to_string()],
            metrics: Some(metrics),
        }
    }

    fn redirect_loop(
        url: String,
        redirect_chain: Vec<RedirectHop>,
        metrics: Metrics,
    ) -> Self {
        Self {
            error: "Detected redirect loop".to_string(),
            url,
            status: None,
            status_text: None,
            headers: Vec::new(),
            content_type: None,
            content_encoding: None,
            redirect_chain,
            body: None,
            warnings: vec!["Redirect loop detected".to_string()],
            metrics: Some(metrics),
        }
    }

    fn timeout(url: String, redirect_chain: Vec<RedirectHop>, metrics: Metrics) -> Self {
        Self {
            error: "Request timed out".to_string(),
            url,
            status: None,
            status_text: None,
            headers: Vec::new(),
            content_type: None,
            content_encoding: None,
            redirect_chain,
            body: None,
            warnings: Vec::new(),
            metrics: Some(metrics),
        }
    }

    fn redirect_resolution(
        url: String,
        mut redirect_chain: Vec<RedirectHop>,
        location: String,
        cause: String,
        metrics: Metrics,
    ) -> Self {
        if let Some(last) = redirect_chain.last_mut() {
            last.location = location.clone();
        }

        Self {
            error: format!("Failed to resolve redirect location '{}': {}", location, cause),
            url,
            status: None,
            status_text: None,
            headers: Vec::new(),
            content_type: None,
            content_encoding: None,
            redirect_chain,
            body: None,
            warnings: Vec::new(),
            metrics: Some(metrics),
        }
    }

    fn processing(
        url: String,
        redirect_chain: Vec<RedirectHop>,
        status: Option<u16>,
        status_text: Option<String>,
        headers: Vec<HeaderEntry>,
        content_type: Option<String>,
        content_encoding: Option<String>,
        cause: String,
        warnings: Vec<String>,
        metrics: Metrics,
    ) -> Self {
        Self {
            error: cause,
            url,
            status,
            status_text,
            headers,
            content_type,
            content_encoding,
            redirect_chain,
            body: None,
            warnings,
            metrics: Some(metrics),
        }
    }

    fn http(
        url: String,
        status: u16,
        status_text: Option<String>,
        headers: Vec<HeaderEntry>,
        content_type: Option<String>,
        content_encoding: Option<String>,
        redirect_chain: Vec<RedirectHop>,
        body: Body,
        warnings: Vec<String>,
        metrics: Metrics,
    ) -> Self {
        Self {
            error: format!("HTTP {} response", status),
            url,
            status: Some(status),
            status_text,
            headers,
            content_type,
            content_encoding,
            redirect_chain,
            body: Some(body),
            warnings,
            metrics: Some(metrics),
        }
    }
}

async fn fetch_impl(initial_url: String, options: FetchOptions) -> Result<FetchSuccess, FetchError> {
    let parsed_url = Url::parse(&initial_url)
        .map_err(|e| FetchError::invalid_url(initial_url.clone(), e.to_string()))?;

    if !matches!(parsed_url.scheme(), "http" | "https") {
        return Err(FetchError::invalid_url(
            initial_url,
            "Unsupported URL scheme (only http/https allowed)".to_string(),
        ));
    }

    let mut current_url = parsed_url;
    let mut redirect_chain = Vec::new();
    let mut redirect_count = 0usize;
    let mut visited = HashSet::new();
    visited.insert(current_url.to_string());

    let start = Instant::now();

    loop {
        let request = build_request(current_url.as_str(), &options);

        let response: Response = match spin_sdk::http::send(request).await {
            Ok(resp) => resp,
            Err(err) => {
                return Err(FetchError::network(
                    current_url.to_string(),
                    redirect_chain,
                    err.to_string(),
                    Metrics::from_elapsed(start.elapsed(), 0),
                ))
            }
        };

        let status_code = *response.status();
        let status_text = HttpStatusCode::from_u16(status_code)
            .ok()
            .and_then(|status| status.canonical_reason().map(|reason| reason.to_string()));
        let headers = collect_headers(&response);

        if is_redirect_status(status_code) {
            if let Some(location) = response.header("location").and_then(|h| h.as_str()) {
                if redirect_count >= options.max_redirects() {
                    return Err(FetchError::redirect_limit(
                        current_url.to_string(),
                        redirect_chain,
                        status_code,
                        status_text,
                        headers,
                        Metrics::from_elapsed(start.elapsed(), 0),
                        options.max_redirects(),
                    ));
                }

                let resolved = match resolve_redirect(&current_url, location) {
                    Ok(url) => url,
                    Err(err) => {
                        return Err(FetchError::redirect_resolution(
                            current_url.to_string(),
                            redirect_chain,
                            location.to_string(),
                            err.to_string(),
                            Metrics::from_elapsed(start.elapsed(), 0),
                        ))
                    }
                };

                if visited.contains(resolved.as_str()) {
                    return Err(FetchError::redirect_loop(
                        current_url.to_string(),
                        redirect_chain,
                        Metrics::from_elapsed(start.elapsed(), 0),
                    ));
                }

                redirect_chain.push(RedirectHop {
                    url: current_url.to_string(),
                    status: status_code,
                    location: location.to_string(),
                });

                current_url = resolved;
                redirect_count += 1;
                visited.insert(current_url.to_string());
                continue;
            }
        }

        let content_type = response
            .header("content-type")
            .and_then(|h| h.as_str())
            .map(|s| s.to_string());
        let content_encoding = response
            .header("content-encoding")
            .and_then(|h| h.as_str())
            .map(|s| s.to_string());

        let (decoded_body, mut decode_warnings) =
            decode_body(response.body(), content_encoding.as_deref(), options.brotli_buffer())
            .map_err(|cause| {
                FetchError::processing(
                    current_url.to_string(),
                    redirect_chain.clone(),
                    Some(status_code),
                    status_text.clone(),
                    headers.clone(),
                    content_type.clone(),
                    content_encoding.clone(),
                    format!("Failed to decode body: {}", cause),
                    Vec::new(),
                    Metrics::from_elapsed(start.elapsed(), 0),
                )
            })?;

        let (body, mut body_warnings) =
            build_body_representation(&decoded_body, content_type.as_deref(), &options);

        let mut warnings = Vec::new();
        warnings.append(&mut decode_warnings);
        warnings.append(&mut body_warnings);

        let metrics = Metrics::from_elapsed(start.elapsed(), decoded_body.len());

        if let Some(limit) = options.timeout_ms() {
            if metrics.elapsed_ms > limit as u128 {
                return Err(FetchError::timeout(
                    current_url.to_string(),
                    redirect_chain,
                    metrics,
                ));
            }
        }

        if !(200..300).contains(&status_code) {
            return Err(FetchError::http(
                current_url.to_string(),
                status_code,
                status_text,
                headers,
                content_type,
                content_encoding,
                redirect_chain,
                body,
                warnings,
                metrics,
            ));
        }

        return Ok(FetchSuccess {
            final_url: current_url.to_string(),
            status: status_code,
            status_text,
            headers,
            content_type,
            content_encoding,
            redirect_chain,
            body,
            warnings,
            metrics,
        });
    }
}

fn build_request(url: &str, options: &FetchOptions) -> Request {
    let mut builder = Request::builder();
    builder
        .method(Method::Get)
        .uri(url)
        .header("User-Agent", options.user_agent())
        .header("Accept", "*/*")
        .header("Accept-Encoding", "gzip, deflate, br");
    builder.build()
}

fn is_redirect_status(status: u16) -> bool {
    matches!(status, 300..=399)
}

fn resolve_redirect(base: &Url, location: &str) -> Result<Url, url::ParseError> {
    Url::parse(location).or_else(|_| base.join(location))
}

fn collect_headers(response: &Response) -> Vec<HeaderEntry> {
    response
        .headers()
        .map(|(name, value)| HeaderEntry {
            name: name.to_string(),
            value: value
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| String::from_utf8_lossy(value.as_bytes()).into_owned()),
        })
        .collect()
}

fn decode_body(
    body: &[u8],
    content_encoding: Option<&str>,
    brotli_buffer: usize,
) -> Result<(Vec<u8>, Vec<String>), String> {
    let mut warnings = Vec::new();
    let mut data = body.to_vec();

    if let Some(header) = content_encoding {
        let encodings: Vec<String> = header
            .split(',')
            .map(|part| part.trim().to_ascii_lowercase())
            .filter(|part| !part.is_empty())
            .collect();

        for encoding in encodings.into_iter().rev() {
            match encoding.as_str() {
                "gzip" | "x-gzip" => {
                    let mut decoder = GzDecoder::new(data.as_slice());
                    let mut decoded = Vec::new();
                    decoder
                        .read_to_end(&mut decoded)
                        .map_err(|e| format!("Failed to decode gzip body: {}", e))?;
                    data = decoded;
                }
                "deflate" => {
                    let mut decoder = ZlibDecoder::new(data.as_slice());
                    let mut decoded = Vec::new();
                    decoder
                        .read_to_end(&mut decoded)
                        .map_err(|e| format!("Failed to decode deflate body: {}", e))?;
                    data = decoded;
                }
                "br" => {
                    let mut decoder = Decompressor::new(data.as_slice(), brotli_buffer);
                    let mut decoded = Vec::new();
                    decoder
                        .read_to_end(&mut decoded)
                        .map_err(|e| format!("Failed to decode brotli body: {}", e))?;
                    data = decoded;
                }
                "identity" | "" => {}
                other => warnings.push(format!(
                    "Unsupported content-encoding '{}'; returning raw body",
                    other
                )),
            }
        }
    }

    Ok((data, warnings))
}

fn build_body_representation(
    body: &[u8],
    content_type: Option<&str>,
    options: &FetchOptions,
) -> (Body, Vec<String>) {
    if body.is_empty() {
        return (Body::Empty, Vec::new());
    }

    let mut warnings = Vec::new();
    let size = body.len();
    let mime = content_type.and_then(|ct| ct.parse::<Mime>().ok());

    if let Some(mime) = mime.as_ref() {
        if is_json_mime(mime) {
            if let Ok(value) = serde_json::from_slice::<Value>(body) {
                return (Body::Json { size, truncated: false, value }, warnings);
            }

            if let Ok(text) = std::str::from_utf8(body) {
                let mut values = Vec::new();
                for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                    match serde_json::from_str::<Value>(line) {
                        Ok(value) => values.push(value),
                        Err(_) => {
                            values.clear();
                            break;
                        }
                    }
                }

                if !values.is_empty() {
                    warnings.push("Interpreted body as newline-delimited JSON".to_string());
                    return (Body::Json { size, truncated: false, value: Value::Array(values) }, warnings);
                }
            }

            warnings.push("Failed to parse response body as JSON; returning as text".to_string());
        }

        if is_text_mime(mime) {
            return (
                build_text_body(body, size, options.max_text_bytes()),
                warnings,
            );
        }
    }

    if looks_like_json(body) {
        match serde_json::from_slice::<Value>(body) {
            Ok(value) => return (Body::Json { size, truncated: false, value }, warnings),
            Err(err) => warnings.push(format!("Failed to parse JSON content: {}", err)),
        }
    }

    if std::str::from_utf8(body).is_ok() {
        return (
            build_text_body(body, size, options.max_text_bytes()),
            warnings,
        );
    }

    warnings.push("Response treated as binary data".to_string());
    (
        build_binary_body(body, size, options.max_binary_bytes()),
        warnings,
    )
}

fn build_text_body(body: &[u8], size: usize, limit: usize) -> Body {
    let (clipped, truncated) = clip_bytes(body, limit);
    let cow = String::from_utf8_lossy(&clipped);
    let (content, encoding) = match cow {
        Cow::Borrowed(_) => (cow.into_owned(), "utf-8".to_string()),
        Cow::Owned(s) => (s, "lossy-utf-8".to_string()),
    };

    Body::Text {
        size,
        truncated,
        encoding,
        content,
    }
}

fn build_binary_body(body: &[u8], size: usize, limit: usize) -> Body {
    let (clipped, truncated) = clip_bytes(body, limit);
    let base64 = BASE64.encode(clipped);
    Body::Binary {
        size,
        truncated,
        encoding: "base64".to_string(),
        base64,
    }
}

fn clip_bytes(data: &[u8], limit: usize) -> (Vec<u8>, bool) {
    if data.len() > limit {
        (data[..limit].to_vec(), true)
    } else {
        (data.to_vec(), false)
    }
}

fn is_json_mime(mime: &Mime) -> bool {
    if mime.type_() == mime::APPLICATION && mime.subtype() == mime::JSON {
        return true;
    }

    if let Some(suffix) = mime.suffix() {
        if suffix == mime::JSON {
            return true;
        }
    }

    mime.subtype().as_str().ends_with("+json")
}

fn is_text_mime(mime: &Mime) -> bool {
    if mime.type_() == mime::TEXT {
        return true;
    }

    if let Some(suffix) = mime.suffix() {
        if matches!(suffix, mime::XML | mime::JSON) {
            return true;
        }
    }

    matches!(
        mime.subtype().as_str(),
        "xml" | "json" | "javascript" | "x-javascript" | "x-www-form-urlencoded" | "csv" | "plain" | "html"
    )
}

fn looks_like_json(body: &[u8]) -> bool {
    body
        .iter()
        .copied()
        .skip_while(|b| b.is_ascii_whitespace())
        .next()
        .map_or(false, |b| matches!(b, b'{' | b'['))
}

bindings::export!(Component with_types_in bindings);
