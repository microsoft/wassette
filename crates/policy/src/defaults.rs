// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Default HTTP domains for network permissions
//!
//! This module defines the default HTTP domains that are automatically included
//! when `defaults: true` is specified in network permissions.
//!
//! Based on: https://github.com/microsoft/policy-mcp/blob/main/DEFAULTS.md

/// The static list of default HTTP domains
static DEFAULT_DOMAINS: &[&str] = &[
    // Package Registries
    "registry.npmjs.org",
    "*.npmjs.com",
    "pypi.org",
    "*.pypi.org",
    "files.pythonhosted.org",
    "rubygems.org",
    "*.rubygems.org",
    "crates.io",
    "*.crates.io",
    "static.crates.io",
    "index.crates.io",
    "nuget.org",
    "*.nuget.org",
    "api.nuget.org",
    "repo.maven.apache.org",
    "repo1.maven.org",
    "central.maven.org",
    "search.maven.org",
    "registry.yarnpkg.com",
    // Version Control Systems
    "github.com",
    "*.github.com",
    "api.github.com",
    "raw.githubusercontent.com",
    "codeload.github.com",
    "gitlab.com",
    "*.gitlab.com",
    "bitbucket.org",
    "*.bitbucket.org",
    "api.bitbucket.org",
    // Cloud Service Providers
    "*.amazonaws.com",
    "s3.amazonaws.com",
    "*.s3.amazonaws.com",
    "*.googleapis.com",
    "storage.googleapis.com",
    "*.google.com",
    "*.azure.com",
    "*.azurewebsites.net",
    "*.blob.core.windows.net",
    "*.cloudflare.com",
    "cloudflare.com",
    // Container Registries
    "docker.io",
    "*.docker.io",
    "registry-1.docker.io",
    "index.docker.io",
    "quay.io",
    "*.quay.io",
    "ghcr.io",
    "*.pkg.dev",
    "gcr.io",
    "*.gcr.io",
    // AI and ML APIs
    "api.openai.com",
    "*.openai.com",
    "api.anthropic.com",
    "*.anthropic.com",
    "api.cohere.ai",
    "*.cohere.ai",
    "huggingface.co",
    "*.huggingface.co",
    "cdn-lfs.huggingface.co",
    // Content Delivery Networks (CDNs)
    "cdn.jsdelivr.net",
    "*.jsdelivr.net",
    "unpkg.com",
    "cdnjs.cloudflare.com",
    "*.fastly.net",
    "*.akamaized.net",
    "*.edgecastcdn.net",
    // Documentation and Learning Resources
    "docs.rs",
    "readthedocs.io",
    "*.readthedocs.io",
    "readthedocs.org",
    "*.readthedocs.org",
    // Build and CI/CD Services
    "circleci.com",
    "*.circleci.com",
    "actions.githubusercontent.com",
    "objects.githubusercontent.com",
];

/// Get the list of default HTTP domains
///
/// These domains include commonly used package registries, version control systems,
/// cloud service providers, container registries, AI/ML APIs, CDNs, documentation sites,
/// and build/CI/CD services.
pub fn get_default_domains() -> &'static [&'static str] {
    DEFAULT_DOMAINS
}


