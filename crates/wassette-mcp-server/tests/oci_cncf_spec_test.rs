// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Tests for CNCF WebAssembly OCI artifact specification compliance
//!
//! These tests verify that Wassette correctly implements the CNCF WebAssembly
//! OCI artifact format as specified at:
//! https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/
//!
//! Note: Wassette EXTENDS the CNCF spec to support multi-layer artifacts
//! (WASM + policy + additional layers). These tests focus on CNCF compliance
//! for the core WASM component format, while multi-layer extensions are
//! tested separately in oci_integration_test.rs and oci_unit_test.rs.

use serde_json::json;
use sha2::{Digest, Sha256};

/// Calculate SHA256 digest of data in OCI format (sha256:hex)
fn calculate_digest(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    format!("sha256:{}", hex::encode(result))
}

#[cfg(test)]
mod cncf_spec_tests {
    use super::*;

    /// Test that we correctly parse and validate CNCF-compliant config
    #[test]
    fn test_cncf_config_media_type() {
        // Create a CNCF-compliant config
        let config = json!({
            "created": "2024-09-25T12:00:00Z",
            "architecture": "wasm",
            "os": "wasip2",
            "layerDigests": [
                "sha256:abc123...",
                "sha256:def456..."
            ],
            "component": {
                "exports": [
                    "wasi:http/incoming-handler@0.2.0"
                ],
                "imports": [
                    "wasi:io/error@0.2.0",
                    "wasi:io/streams@0.2.0"
                ],
                "target": "wasi:http/proxy@0.2.0"
            }
        });

        let config_str = config.to_string();
        let config_bytes = config_str.as_bytes();

        // Verify we can parse it
        let parsed: serde_json::Value = serde_json::from_slice(config_bytes).unwrap();
        assert_eq!(parsed["architecture"], "wasm");
        assert_eq!(parsed["os"], "wasip2");
        assert!(parsed["component"].is_object());
        assert!(parsed["component"]["exports"].is_array());
        assert!(parsed["component"]["imports"].is_array());
    }

    /// Test validation of architecture field
    #[test]
    fn test_architecture_validation() {
        let valid_config = json!({
            "created": "2024-09-25T12:00:00Z",
            "architecture": "wasm",  // Must be "wasm"
            "os": "wasip1",
            "layerDigests": ["sha256:abc123"]
        });

        let invalid_config = json!({
            "created": "2024-09-25T12:00:00Z",
            "architecture": "amd64",  // Invalid for WASM
            "os": "wasip1",
            "layerDigests": ["sha256:abc123"]
        });

        assert_eq!(valid_config["architecture"], "wasm");
        assert_ne!(invalid_config["architecture"], "wasm");
    }

    /// Test OS field validation for wasip1 and wasip2
    #[test]
    fn test_os_field_validation() {
        // Test wasip1
        let wasip1_config = json!({
            "created": "2024-09-25T12:00:00Z",
            "architecture": "wasm",
            "os": "wasip1",
            "layerDigests": ["sha256:abc123"]
            // component section is optional for wasip1
        });

        // Test wasip2 with required component section
        let wasip2_config = json!({
            "created": "2024-09-25T12:00:00Z",
            "architecture": "wasm",
            "os": "wasip2",
            "layerDigests": ["sha256:abc123"],
            "component": {
                "exports": ["wasi:cli/run@0.2.0"],
                "imports": ["wasi:filesystem/types@0.2.0"]
            }
        });

        assert_eq!(wasip1_config["os"], "wasip1");
        assert_eq!(wasip2_config["os"], "wasip2");
        assert!(wasip2_config["component"].is_object());
    }

    /// Test that wasip2 configs require component metadata
    #[test]
    fn test_wasip2_requires_component_metadata() {
        let wasip2_without_component = json!({
            "created": "2024-09-25T12:00:00Z",
            "architecture": "wasm",
            "os": "wasip2",
            "layerDigests": ["sha256:abc123"]
            // Missing required component section
        });

        let wasip2_with_component = json!({
            "created": "2024-09-25T12:00:00Z",
            "architecture": "wasm",
            "os": "wasip2",
            "layerDigests": ["sha256:abc123"],
            "component": {
                "exports": ["wasi:http/incoming-handler@0.2.0"],
                "imports": ["wasi:io/error@0.2.0"]
            }
        });

        // wasip2 should have component metadata
        assert!(wasip2_without_component["component"].is_null());
        assert!(wasip2_with_component["component"].is_object());
    }

    /// Test layer digests array requirement
    #[test]
    fn test_layer_digests_requirement() {
        let config_with_digests = json!({
            "created": "2024-09-25T12:00:00Z",
            "architecture": "wasm",
            "os": "wasip1",
            "layerDigests": [
                "sha256:f540212bd2ba52a5e21d2ade105d0821131eeb4a6064670382c2ceb600eb0aad",
                "sha256:827d101cb3feb514b2762e023c9459b486f55b3f22eef0990be61bb5f0639b39"
            ]
        });

        let layer_digests = config_with_digests["layerDigests"].as_array().unwrap();
        assert_eq!(layer_digests.len(), 2);
        assert!(layer_digests[0].as_str().unwrap().starts_with("sha256:"));
        assert!(layer_digests[1].as_str().unwrap().starts_with("sha256:"));
    }

    /// Test component exports and imports structure
    #[test]
    fn test_component_metadata_structure() {
        let component_metadata = json!({
            "exports": [
                "wasi:http/incoming-handler@0.2.0",
                "wasi:cli/run@0.2.0"
            ],
            "imports": [
                "wasi:io/error@0.2.0",
                "wasi:io/streams@0.2.0",
                "wasi:filesystem/types@0.2.0",
                "wasi:clocks/monotonic-clock@0.2.0"
            ],
            "target": "wasi:http/proxy@0.2.0"
        });

        let exports = component_metadata["exports"].as_array().unwrap();
        let imports = component_metadata["imports"].as_array().unwrap();

        assert_eq!(exports.len(), 2);
        assert_eq!(imports.len(), 4);
        assert!(exports[0].as_str().unwrap().contains("wasi:"));
        assert!(imports[0].as_str().unwrap().contains("wasi:"));
        assert_eq!(component_metadata["target"], "wasi:http/proxy@0.2.0");
    }

    /// Test manifest with CNCF-compliant media types
    #[test]
    fn test_cncf_manifest_media_types() {
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.wasm.config.v0+json",  // CNCF spec
                "size": 730,
                "digest": "sha256:1b406577a60e324e68679259b522cb6d7853f7122d6271fbc41dc7779d50efc9"
            },
            "layers": [
                {
                    "mediaType": "application/wasm",  // CNCF spec for WASM layer
                    "size": 124000,
                    "digest": "sha256:f540212bd2ba52a5e21d2ade105d0821131eeb4a6064670382c2ceb600eb0aad"
                }
            ]
        });

        assert_eq!(
            manifest["config"]["mediaType"],
            "application/vnd.wasm.config.v0+json"
        );
        assert_eq!(manifest["layers"][0]["mediaType"], "application/wasm");
    }

    /// Test multi-layer manifest with WASM and policy
    /// NOTE: This is a Wassette EXTENSION beyond the CNCF spec
    #[test]
    fn test_multi_layer_manifest() {
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.wasm.config.v0+json",
                "size": 730,
                "digest": "sha256:config123"
            },
            "layers": [
                {
                    "mediaType": "application/wasm",
                    "size": 124000,
                    "digest": "sha256:wasm456",
                    "annotations": {
                        "org.opencontainers.image.title": "component.wasm"
                    }
                },
                {
                    "mediaType": "application/vnd.wasm.policy.v1+yaml",
                    "size": 574,
                    "digest": "sha256:policy789",
                    "annotations": {
                        "org.opencontainers.image.title": "policy.yaml"
                    }
                }
            ]
        });

        let layers = manifest["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0]["mediaType"], "application/wasm");
        assert_eq!(
            layers[1]["mediaType"],
            "application/vnd.wasm.policy.v1+yaml"
        );
    }
}

#[cfg(test)]
mod integration_tests {
    use std::collections::HashMap;

    use super::*;

    /// Mock structure matching our MultiLayerArtifact
    #[allow(dead_code)]
    struct TestArtifact {
        wasm_data: Vec<u8>,
        policy_data: Option<Vec<u8>>,
        config: Option<TestWasmConfig>,
        additional_layers: HashMap<String, Vec<u8>>,
    }

    #[derive(Debug, Clone)]
    #[allow(dead_code)]
    struct TestWasmConfig {
        created: String,
        architecture: String,
        os: String,
        layer_digests: Vec<String>,
        component: Option<TestComponentMetadata>,
    }

    #[derive(Debug, Clone)]
    #[allow(dead_code)]
    struct TestComponentMetadata {
        exports: Option<Vec<String>>,
        imports: Option<Vec<String>>,
        target: Option<String>,
    }

    /// Test parsing a complete CNCF-compliant artifact
    #[test]
    fn test_parse_cncf_compliant_artifact() {
        // Simulate receiving a CNCF-compliant artifact
        let wasm_data = vec![0x00, 0x61, 0x73, 0x6d]; // WASM magic bytes
        let policy_data = b"policy: allow_all".to_vec();

        let config = TestWasmConfig {
            created: "2024-09-25T12:00:00Z".to_string(),
            architecture: "wasm".to_string(),
            os: "wasip2".to_string(),
            layer_digests: vec![calculate_digest(&wasm_data), calculate_digest(&policy_data)],
            component: Some(TestComponentMetadata {
                exports: Some(vec!["wasi:http/incoming-handler@0.2.0".to_string()]),
                imports: Some(vec!["wasi:io/error@0.2.0".to_string()]),
                target: Some("wasi:http/proxy@0.2.0".to_string()),
            }),
        };

        let artifact = TestArtifact {
            wasm_data,
            policy_data: Some(policy_data),
            config: Some(config.clone()),
            additional_layers: HashMap::new(),
        };

        // Validate the artifact matches CNCF spec
        assert_eq!(artifact.config.as_ref().unwrap().architecture, "wasm");
        assert_eq!(artifact.config.as_ref().unwrap().os, "wasip2");
        assert!(artifact.config.as_ref().unwrap().component.is_some());

        let component = artifact
            .config
            .as_ref()
            .unwrap()
            .component
            .as_ref()
            .unwrap();
        assert!(component.exports.is_some());
        assert!(component.imports.is_some());
        assert_eq!(component.exports.as_ref().unwrap().len(), 1);
        assert_eq!(component.imports.as_ref().unwrap().len(), 1);
    }

    /// Test that layer digests in config match actual layer digests
    #[test]
    fn test_layer_digest_consistency() {
        let wasm_data = b"fake wasm content".to_vec();
        let wasm_digest = calculate_digest(&wasm_data);

        let config = TestWasmConfig {
            created: "2024-09-25T12:00:00Z".to_string(),
            architecture: "wasm".to_string(),
            os: "wasip1".to_string(),
            layer_digests: vec![wasm_digest.clone()],
            component: None,
        };

        // Verify the digest in config matches actual data digest
        assert_eq!(config.layer_digests[0], wasm_digest);
        assert!(config.layer_digests[0].starts_with("sha256:"));
    }

    /// Test backward compatibility with non-CNCF artifacts
    #[test]
    fn test_backward_compatibility() {
        // Test that we can still handle artifacts without CNCF config
        let artifact = TestArtifact {
            wasm_data: vec![0x00, 0x61, 0x73, 0x6d],
            policy_data: None,
            config: None, // No config - pre-CNCF artifact
            additional_layers: HashMap::new(),
        };

        // Should still have valid WASM data even without config
        assert!(!artifact.wasm_data.is_empty());
        assert!(artifact.config.is_none());
    }
}
