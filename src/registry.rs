// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Registry operations for searching and fetching components from component-registry.json

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Represents a component in the registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryComponent {
    pub name: String,
    pub description: String,
    pub uri: String,
}

/// Parse the component registry JSON
pub fn parse_registry(registry_json: &str) -> Result<Vec<RegistryComponent>> {
    serde_json::from_str(registry_json).context("Failed to parse component registry JSON")
}

/// Search for components matching a query string
pub fn search_components(
    components: &[RegistryComponent],
    query: Option<&str>,
) -> Vec<RegistryComponent> {
    match query {
        None => components.to_vec(),
        Some(q) => {
            let query_lower = q.to_lowercase();
            components
                .iter()
                .filter(|c| {
                    c.name.to_lowercase().contains(&query_lower)
                        || c.description.to_lowercase().contains(&query_lower)
                })
                .cloned()
                .collect()
        }
    }
}

/// Find a component by name or URI
pub fn find_component_by_name_or_uri(
    components: &[RegistryComponent],
    name_or_uri: &str,
) -> Option<RegistryComponent> {
    components
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name_or_uri) || c.uri == name_or_uri)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_registry() {
        let json = r#"[
            {
                "name": "Weather Server",
                "description": "A weather component",
                "uri": "oci://ghcr.io/microsoft/get-weather-js:latest"
            }
        ]"#;

        let components = parse_registry(json).unwrap();
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].name, "Weather Server");
    }

    #[test]
    fn test_search_components_no_query() {
        let components = vec![
            RegistryComponent {
                name: "Component A".to_string(),
                description: "Description A".to_string(),
                uri: "oci://example.com/a".to_string(),
            },
            RegistryComponent {
                name: "Component B".to_string(),
                description: "Description B".to_string(),
                uri: "oci://example.com/b".to_string(),
            },
        ];

        let results = search_components(&components, None);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_components_with_query() {
        let components = vec![
            RegistryComponent {
                name: "Weather Server".to_string(),
                description: "A weather component".to_string(),
                uri: "oci://example.com/weather".to_string(),
            },
            RegistryComponent {
                name: "Time Server".to_string(),
                description: "A time component".to_string(),
                uri: "oci://example.com/time".to_string(),
            },
        ];

        let results = search_components(&components, Some("weather"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Weather Server");
    }

    #[test]
    fn test_search_components_case_insensitive() {
        let components = vec![RegistryComponent {
            name: "Weather Server".to_string(),
            description: "A weather component".to_string(),
            uri: "oci://example.com/weather".to_string(),
        }];

        let results = search_components(&components, Some("WEATHER"));
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_find_component_by_name() {
        let components = vec![RegistryComponent {
            name: "Weather Server".to_string(),
            description: "A weather component".to_string(),
            uri: "oci://example.com/weather".to_string(),
        }];

        let result = find_component_by_name_or_uri(&components, "Weather Server");
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "Weather Server");
    }

    #[test]
    fn test_find_component_by_uri() {
        let components = vec![RegistryComponent {
            name: "Weather Server".to_string(),
            description: "A weather component".to_string(),
            uri: "oci://example.com/weather".to_string(),
        }];

        let result = find_component_by_name_or_uri(&components, "oci://example.com/weather");
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "Weather Server");
    }
}
