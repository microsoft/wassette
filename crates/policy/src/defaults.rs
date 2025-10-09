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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_domains_not_empty() {
        let domains = get_default_domains();
        assert!(
            !domains.is_empty(),
            "Default domains list should not be empty"
        );
    }

    #[test]
    fn test_default_domains_include_common_registries() {
        let domains = get_default_domains();

        // Check for major package registries
        assert!(
            domains.contains(&"registry.npmjs.org"),
            "Should include npm registry"
        );
        assert!(
            domains.contains(&"pypi.org"),
            "Should include Python package index"
        );
        assert!(
            domains.contains(&"crates.io"),
            "Should include Rust crates registry"
        );
        assert!(
            domains.contains(&"nuget.org"),
            "Should include NuGet registry"
        );
    }

    #[test]
    fn test_default_domains_include_vcs() {
        let domains = get_default_domains();

        // Check for version control systems
        assert!(domains.contains(&"github.com"), "Should include GitHub");
        assert!(domains.contains(&"gitlab.com"), "Should include GitLab");
        assert!(
            domains.contains(&"bitbucket.org"),
            "Should include Bitbucket"
        );
    }

    #[test]
    fn test_default_domains_include_cloud_providers() {
        let domains = get_default_domains();

        // Check for major cloud providers
        assert!(domains.contains(&"*.amazonaws.com"), "Should include AWS");
        assert!(
            domains.contains(&"*.googleapis.com"),
            "Should include Google Cloud"
        );
        assert!(domains.contains(&"*.azure.com"), "Should include Azure");
    }

    #[test]
    fn test_default_domains_include_ai_apis() {
        let domains = get_default_domains();

        // Check for AI/ML APIs
        assert!(domains.contains(&"api.openai.com"), "Should include OpenAI");
        assert!(
            domains.contains(&"api.anthropic.com"),
            "Should include Anthropic"
        );
        assert!(
            domains.contains(&"huggingface.co"),
            "Should include Hugging Face"
        );
    }

    #[test]
    fn test_default_domains_no_duplicates() {
        let domains = get_default_domains();
        let mut unique_domains = std::collections::HashSet::new();

        for domain in domains {
            assert!(
                unique_domains.insert(domain),
                "Duplicate domain found: {}",
                domain
            );
        }
    }

    #[test]
    fn test_default_domains_all_lowercase() {
        let domains = get_default_domains();

        for domain in domains {
            assert_eq!(
                *domain,
                domain.to_lowercase(),
                "Domain {} should be lowercase",
                domain
            );
        }
    }
}
