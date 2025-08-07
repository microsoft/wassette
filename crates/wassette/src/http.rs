// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::collections::HashSet;

use anyhow::Result;
use tracing::{debug, warn};
use url::Url;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::{IoView, WasiView};
use wasmtime_wasi_http::bindings::http::types;
use wasmtime_wasi_http::types::{HostFutureIncomingResponse, OutgoingRequestConfig};
use wasmtime_wasi_http::{HttpResult, WasiHttpView};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AllowedHost {
    scheme: Option<String>,
    host: String,
}

impl AllowedHost {
    fn from_str(host_str: &str) -> Result<Self> {
        if let Ok(url) = Url::parse(host_str) {
            Ok(AllowedHost {
                scheme: Some(url.scheme().to_string()),
                host: url.host_str().unwrap_or("").to_string(),
            })
        } else if let Ok(url) = Url::parse(&format!("http://{host_str}")) {
            Ok(AllowedHost {
                scheme: None,
                host: url.host_str().unwrap_or("").to_string(),
            })
        } else {
            Err(anyhow::anyhow!("Invalid host format: {}", host_str))
        }
    }

    fn matches(&self, request_host: &str, request_scheme: Option<&str>) -> bool {
        if self.host != request_host {
            return false;
        }

        match (&self.scheme, request_scheme) {
            (Some(allowed_scheme), Some(req_scheme)) => allowed_scheme == req_scheme,
            _ => true,
        }
    }
}

/// WassetteWasiState is a wrapper around a WASI state that enforces network policies by filtering
/// outgoing HTTP requests based on a list of allowed hosts from the component's policy document.
pub struct WassetteWasiState<T> {
    /// The underlying WASI state
    pub inner: T,

    /// Set of allowed hosts for network requests (extracted from policy document)
    allowed_hosts: HashSet<AllowedHost>,
}

impl<T> WassetteWasiState<T> {
    /// Create a new WassetteWasiState with the given allowed hosts
    pub fn new(inner: T, allowed_hosts: HashSet<String>) -> Result<Self> {
        let mut parsed_hosts = HashSet::new();

        for host_str in allowed_hosts {
            match AllowedHost::from_str(&host_str) {
                Ok(parsed_host) => {
                    parsed_hosts.insert(parsed_host);
                }
                Err(e) => {
                    warn!("Failed to parse allowed host '{}': {}", host_str, e);
                    return Err(e);
                }
            }
        }

        Ok(Self {
            inner,
            allowed_hosts: parsed_hosts,
        })
    }

    /// Check if a host is allowed by the policy
    fn is_host_allowed(&self, uri: &hyper::Uri) -> bool {
        let request_host = if let Some(host) = uri.host() {
            host.to_string()
        } else {
            return false;
        };

        let request_scheme = uri.scheme().map(|s| s.as_str());

        let req = request_host.to_ascii_lowercase();
        for allowed_host in &self.allowed_hosts {
            if allowed_host.matches(&req, request_scheme) {
                return true;
            }
        }

        false
    }
}

impl<T: IoView> IoView for WassetteWasiState<T> {
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable {
        self.inner.table()
    }
}

impl<T: WasiView> WasiView for WassetteWasiState<T> {
    fn ctx(&mut self) -> &mut wasmtime_wasi::p2::WasiCtx {
        self.inner.ctx()
    }
}

impl<T: WasiHttpView> WasiHttpView for WassetteWasiState<T> {
    fn ctx(&mut self) -> &mut wasmtime_wasi_http::WasiHttpCtx {
        self.inner.ctx()
    }

    fn new_response_outparam(
        &mut self,
        result: tokio::sync::oneshot::Sender<
            Result<hyper::Response<wasmtime_wasi_http::body::HyperOutgoingBody>, types::ErrorCode>,
        >,
    ) -> wasmtime::Result<Resource<wasmtime_wasi_http::types::HostResponseOutparam>> {
        self.inner.new_response_outparam(result)
    }

    fn send_request(
        &mut self,
        request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        let uri = request.uri();

        if uri.host().is_none() {
            warn!("HTTP request missing host, blocking request");
            return Err(types::ErrorCode::HttpRequestUriInvalid.into());
        }

        if !self.is_host_allowed(uri) {
            warn!(
                uri = %uri,
                allowed_hosts = ?self.allowed_hosts,
                "HTTP request blocked by network policy"
            );
            return Err(types::ErrorCode::HttpRequestDenied.into());
        }

        debug!(uri = %uri, "HTTP request allowed by network policy");

        self.inner.send_request(request, config)
    }

    fn is_forbidden_header(&mut self, name: &hyper::header::HeaderName) -> bool {
        self.inner.is_forbidden_header(name)
    }

    fn outgoing_body_buffer_chunks(&mut self) -> usize {
        self.inner.outgoing_body_buffer_chunks()
    }

    fn outgoing_body_chunk_size(&mut self) -> usize {
        self.inner.outgoing_body_chunk_size()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn create_mock_wasi_state() -> MockWasiState {
        MockWasiState
    }

    struct MockWasiState;

    impl IoView for MockWasiState {
        fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable {
            unimplemented!("Mock for testing")
        }
    }

    impl WasiHttpView for MockWasiState {
        fn ctx(&mut self) -> &mut wasmtime_wasi_http::WasiHttpCtx {
            unimplemented!("Mock for testing")
        }

        fn new_response_outparam(
            &mut self,
            _result: tokio::sync::oneshot::Sender<
                Result<
                    hyper::Response<wasmtime_wasi_http::body::HyperOutgoingBody>,
                    types::ErrorCode,
                >,
            >,
        ) -> wasmtime::Result<Resource<wasmtime_wasi_http::types::HostResponseOutparam>> {
            unimplemented!("Mock for testing")
        }

        fn send_request(
            &mut self,
            _request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
            _config: OutgoingRequestConfig,
        ) -> HttpResult<HostFutureIncomingResponse> {
            unimplemented!("Mock for testing")
        }
    }

    #[test]
    fn test_host_allowed_exact_match() {
        let mut allowed_hosts = HashSet::new();
        allowed_hosts.insert("api.example.com".to_string());

        let state = WassetteWasiState::new(create_mock_wasi_state(), allowed_hosts).unwrap();

        let uri1: hyper::Uri = "http://api.example.com".parse().unwrap();
        let uri2: hyper::Uri = "http://other.example.com".parse().unwrap();
        let uri3: hyper::Uri = "http://malicious.com".parse().unwrap();

        assert!(state.is_host_allowed(&uri1));
        assert!(!state.is_host_allowed(&uri2));
        assert!(!state.is_host_allowed(&uri3));
    }

    #[test]
    fn test_host_allowed_with_protocol() {
        let mut allowed_hosts = HashSet::new();
        allowed_hosts.insert("https://api.example.com".to_string());

        let state = WassetteWasiState::new(create_mock_wasi_state(), allowed_hosts).unwrap();

        let uri1: hyper::Uri = "http://api.example.com".parse().unwrap();
        let uri2: hyper::Uri = "https://api.example.com".parse().unwrap();
        let uri3: hyper::Uri = "http://api.example.com".parse().unwrap();

        assert!(!state.is_host_allowed(&uri1));
        assert!(state.is_host_allowed(&uri2));
        assert!(!state.is_host_allowed(&uri3));
    }

    #[test]
    fn test_host_allowed_with_port() {
        let mut allowed_hosts = HashSet::new();
        allowed_hosts.insert("api.example.com".to_string());

        let state = WassetteWasiState::new(create_mock_wasi_state(), allowed_hosts).unwrap();

        let uri1: hyper::Uri = "http://api.example.com:8080".parse().unwrap();
        let uri2: hyper::Uri = "http://api.example.com:443".parse().unwrap();

        assert!(state.is_host_allowed(&uri1));
        assert!(state.is_host_allowed(&uri2));
    }

    #[test]
    fn test_scheme_specific_matching() {
        let mut allowed_hosts = HashSet::new();
        allowed_hosts.insert("https://secure.api.com".to_string());
        allowed_hosts.insert("api.example.com".to_string()); // scheme-agnostic

        let state = WassetteWasiState::new(create_mock_wasi_state(), allowed_hosts).unwrap();

        // Scheme-specific host should only match HTTPS
        let https_secure: hyper::Uri = "https://secure.api.com".parse().unwrap();
        let http_secure: hyper::Uri = "http://secure.api.com".parse().unwrap();

        assert!(state.is_host_allowed(&https_secure));
        assert!(!state.is_host_allowed(&http_secure));

        // Scheme-agnostic host should match both
        let https_example: hyper::Uri = "https://api.example.com".parse().unwrap();
        let http_example: hyper::Uri = "http://api.example.com".parse().unwrap();

        assert!(state.is_host_allowed(&https_example));
        assert!(state.is_host_allowed(&http_example));
    }

    #[test]
    fn test_new_with_invalid_host() {
        let mut allowed_hosts = HashSet::new();
        allowed_hosts.insert("http://".to_string());
        allowed_hosts.insert("".to_string());

        match WassetteWasiState::new(create_mock_wasi_state(), allowed_hosts) {
            Ok(_) => panic!("Expected error, got Ok"),
            Err(e) => assert!(e.to_string().contains("Invalid host format")),
        }
    }

    #[test]
    fn test_host_matching_is_case_insensitive() {
        let mut allowed_hosts = HashSet::new();
        allowed_hosts.insert("api.example.com".to_string());

        let state = WassetteWasiState::new(create_mock_wasi_state(), allowed_hosts).unwrap();

        let uri1: hyper::Uri = "http://api.example.com".parse().unwrap();
        let uri2: hyper::Uri = "http://API.EXAMPLE.COM".parse().unwrap();

        assert!(state.is_host_allowed(&uri1));
        assert!(state.is_host_allowed(&uri2));
    }
}
