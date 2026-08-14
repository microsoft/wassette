// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Network policy enforcement for guest-initiated HTTP.
//!
//! `WasiCtx` gates raw sockets, but `wasi:http` outgoing requests are
//! serviced by the host's own client and never touch the guest's socket
//! permissions. Without a hook a policy-less component could still
//! `GET` anything. These hooks close that: every outbound request is
//! matched against the chain's allow-list (see
//! [`crate::sandbox::ChainSandbox::http_allowlist`]) and denied
//! otherwise, mirroring `wassette::WassetteWasiState`'s behaviour for
//! MCP components.
//!
//! `--allow-all` installs an unfiltered instance.

use std::collections::BTreeSet;
use std::future::Future;

use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use tracing::{debug, warn};
use wasmtime_wasi::TrappableError;
use wasmtime_wasi_http::p2::bindings::http::types;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::types::{HostFutureIncomingResponse, OutgoingRequestConfig};
use wasmtime_wasi_http::p2::{HttpResult, WasiHttpHooks, default_send_request};
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode as P3ErrorCode;

/// What `wasi:http@0.3`'s `send_request` hook resolves to: the response
/// plus a future carrying any error seen while streaming its body.
type P3Result = Result<
    (
        http::Response<UnsyncBoxBody<Bytes, P3ErrorCode>>,
        Box<dyn Future<Output = Result<(), P3ErrorCode>> + Send>,
    ),
    TrappableError<P3ErrorCode>,
>;

/// One entry of a policy's `permissions.network.allow` list, parsed into
/// an optional scheme and a host.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AllowedHost {
    /// Set when the policy pinned a scheme (`https://api.example.com`).
    scheme: Option<String>,
    host: String,
}

impl AllowedHost {
    /// Parse `https://api.example.com`, `api.example.com:8080` or
    /// `api.example.com` into a scheme/host pair.
    fn parse(entry: &str) -> Self {
        let (scheme, rest) = match entry.split_once("://") {
            Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
            None => (None, entry),
        };
        // Drop any path, then any port: policies name hosts, and the
        // port is not part of the identity being authorised.
        let host = rest
            .split('/')
            .next()
            .unwrap_or(rest)
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or_else(|| rest.split('/').next().unwrap_or(rest))
            .to_ascii_lowercase();
        Self { scheme, host }
    }

    fn matches(&self, host: &str, scheme: Option<&str>) -> bool {
        if self.host != host {
            return false;
        }
        match (&self.scheme, scheme) {
            (Some(allowed), Some(actual)) => allowed == actual,
            _ => true,
        }
    }
}

/// Outbound-HTTP policy for one chain's store.
pub struct HttpPolicyHooks {
    /// `None` under `--allow-all`: no filtering at all.
    allowed: Option<Vec<AllowedHost>>,
}

impl HttpPolicyHooks {
    /// Build hooks from a chain's allow-list. `None` disables filtering.
    pub fn new(allowed_hosts: Option<&BTreeSet<String>>) -> Self {
        Self {
            allowed: allowed_hosts
                .map(|hosts| hosts.iter().map(|h| AllowedHost::parse(h)).collect()),
        }
    }

    /// Whether `uri` is reachable under this chain's policy.
    fn is_allowed(&self, uri: &http::Uri) -> bool {
        let Some(allowed) = self.allowed.as_ref() else {
            return true;
        };
        let Some(host) = uri.host() else {
            return false;
        };
        let host = host.to_ascii_lowercase();
        let scheme = uri.scheme().map(|s| s.as_str());
        allowed.iter().any(|a| a.matches(&host, scheme))
    }

    /// Deny with `http-request-denied` unless the chain's policy allows
    /// the request's host.
    fn check(&self, uri: &http::Uri) -> HttpResult<()> {
        if self.is_allowed(uri) {
            debug!(%uri, "HTTP request allowed by policy");
            return Ok(());
        }
        warn!(
            %uri,
            "HTTP request blocked: the host is not in any chain policy's \
             `permissions.network.allow` list (use --allow-all to bypass)"
        );
        Err(types::ErrorCode::HttpRequestDenied.into())
    }
}

impl WasiHttpHooks for HttpPolicyHooks {
    fn send_request(
        &mut self,
        request: http::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        self.check(request.uri())?;
        Ok(default_send_request(request, config))
    }
}

/// The same filtering for `wasi:http@0.3`, which the host also links so
/// p3 guests are not an unpoliced side door. The signature is verbose
/// because the trait hands ownership of the request body and the
/// error-reporting futures across the hook boundary; the only behaviour
/// added is the `check` before delegating to the default sender.
impl wasmtime_wasi_http::p3::WasiHttpHooks for HttpPolicyHooks {
    fn send_request(
        &mut self,
        request: http::Request<UnsyncBoxBody<Bytes, P3ErrorCode>>,
        options: Option<wasmtime_wasi_http::p3::RequestOptions>,
        fut: Box<dyn Future<Output = Result<(), P3ErrorCode>> + Send>,
    ) -> Box<dyn Future<Output = P3Result> + Send> {
        if !self.is_allowed(request.uri()) {
            let uri = request.uri().clone();
            warn!(%uri, "HTTP request blocked by policy (wasi:http@0.3)");
            return Box::new(async move { Err(P3ErrorCode::HttpRequestDenied.into()) });
        }
        // The default implementation drops `fut` too: errors observed
        // while the guest consumes the response body are reported
        // through the returned future instead.
        let _ = fut;
        Box::new(async move {
            use http_body_util::BodyExt;

            let (res, io) = wasmtime_wasi_http::p3::default_send_request(request, options).await?;
            Ok((
                res.map(BodyExt::boxed_unsync),
                Box::new(io) as Box<dyn Future<Output = _> + Send>,
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks(hosts: &[&str]) -> HttpPolicyHooks {
        let set: BTreeSet<String> = hosts.iter().map(|h| h.to_string()).collect();
        HttpPolicyHooks::new(Some(&set))
    }

    #[test]
    fn empty_allowlist_denies_everything() {
        let h = hooks(&[]);
        assert!(!h.is_allowed(&"https://example.com/x".parse().unwrap()));
    }

    #[test]
    fn exact_host_is_allowed() {
        let h = hooks(&["api.example.com"]);
        assert!(h.is_allowed(&"https://api.example.com/v1".parse().unwrap()));
        assert!(!h.is_allowed(&"https://evil.example.com/v1".parse().unwrap()));
    }

    #[test]
    fn scheme_pin_is_honoured() {
        let h = hooks(&["https://api.example.com"]);
        assert!(h.is_allowed(&"https://api.example.com/v1".parse().unwrap()));
        assert!(!h.is_allowed(&"http://api.example.com/v1".parse().unwrap()));
    }

    #[test]
    fn ports_are_ignored_when_matching() {
        let h = hooks(&["http://localhost:11434"]);
        assert!(h.is_allowed(&"http://localhost:11434/api".parse().unwrap()));
        assert_eq!(
            AllowedHost::parse("http://localhost:11434"),
            AllowedHost {
                scheme: Some("http".into()),
                host: "localhost".into()
            }
        );
    }

    #[test]
    fn allow_all_skips_filtering() {
        let h = HttpPolicyHooks::new(None);
        assert!(h.is_allowed(&"https://anything.example/x".parse().unwrap()));
    }
}
