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

use tracing::{debug, warn};
use wasmtime_wasi_http::p2::bindings::http::types;
use wasmtime_wasi_http::{RequestOptions, WasiBody, WasiHttpHooks, default_send_request};

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
        let host_matches = if let Some(domain) = self.host.strip_prefix("*.") {
            host.strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1)
        } else {
            self.host == host
        };
        if !host_matches {
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
    fn check(&self, uri: &http::Uri) -> wasmtime_wasi_http::Result<()> {
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
        request: http::Request<WasiBody>,
        options: Option<RequestOptions>,
        _io: Box<dyn Future<Output = wasmtime_wasi_http::Result<()>> + Send>,
    ) -> Box<
        dyn Future<
                Output = wasmtime_wasi_http::Result<(
                    http::Response<WasiBody>,
                    Box<dyn Future<Output = wasmtime_wasi_http::Result<()>> + Send>,
                )>,
            > + Send,
    > {
        if let Err(error) = self.check(request.uri()) {
            return Box::new(async move { Err(error) });
        }

        Box::new(async move {
            use http_body_util::BodyExt;

            let (response, io) = default_send_request(request, options).await?;
            Ok((
                response.map(BodyExt::boxed_unsync),
                Box::new(io) as Box<dyn Future<Output = wasmtime_wasi_http::Result<()>> + Send>,
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
    fn wildcard_matches_subdomains_but_not_apex_or_other_suffixes() {
        let h = hooks(&["*.example.com"]);
        for host in ["api.example.com", "deep.api.example.com", "API.EXAMPLE.COM"] {
            assert!(h.is_allowed(&format!("https://{host}/v1").parse().unwrap()));
        }
        for host in [
            "example.com",
            "badexample.com",
            "api.example.com.evil",
            "evil-example.com",
        ] {
            assert!(!h.is_allowed(&format!("https://{host}/v1").parse().unwrap()));
        }
    }

    #[test]
    fn wildcard_respects_scheme_pin() {
        let h = hooks(&["https://*.example.com"]);
        assert!(h.is_allowed(&"https://api.example.com/v1".parse().unwrap()));
        assert!(!h.is_allowed(&"http://api.example.com/v1".parse().unwrap()));
        assert!(!h.is_allowed(&"https://example.com/v1".parse().unwrap()));
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
