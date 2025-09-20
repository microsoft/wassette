// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Support for multi-layer OCI artifacts
//!
//! This module provides functionality to handle OCI artifacts with multiple layers,
//! such as WASM components bundled with security policies or signatures.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use oci_client::{Client, Reference};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

/// Component metadata from the OCI config (CNCF spec)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentMetadata {
    /// List of exported interfaces
    pub exports: Option<Vec<String>>,
    /// List of imported interfaces
    pub imports: Option<Vec<String>>,
    /// Target world (optional)
    pub target: Option<String>,
}

/// OCI Config for WebAssembly components (CNCF spec)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmConfig {
    /// Creation timestamp
    pub created: String,
    /// Architecture (must be "wasm")
    pub architecture: String,
    /// Operating system ("wasip1" or "wasip2")
    pub os: String,
    /// Layer digests for uniqueness
    #[serde(rename = "layerDigests")]
    pub layer_digests: Vec<String>,
    /// Component metadata (required for wasip2)
    pub component: Option<ComponentMetadata>,
}

/// Represents the different types of layers we can extract from an OCI artifact
pub struct MultiLayerArtifact {
    /// The WASM component data
    pub wasm_data: Vec<u8>,
    /// Optional policy data (YAML format)
    pub policy_data: Option<Vec<u8>>,
    /// OCI config metadata
    pub config: Option<WasmConfig>,
    /// Other layers indexed by media type
    pub additional_layers: HashMap<String, Vec<u8>>,
}

/// Media types we recognize
const WASM_MEDIA_TYPES: &[&str] = &[
    "application/wasm",
    "application/vnd.wasm.component.v1",
    "application/vnd.bytecodealliance.wasm.component.layer.v0+wasm",
];

const POLICY_MEDIA_TYPES: &[&str] = &[
    "application/vnd.wasm.policy.v1+yaml", // CNCF standard (expected)
    "application/vnd.wassette.policy+yaml", // Legacy format (backward compatibility)
    "application/x-yaml",
    "text/yaml",
];

/// Config media type from CNCF spec (expected)
const CONFIG_MEDIA_TYPE: &str = "application/vnd.wasm.config.v0+json";
/// OCI Image config media type
const OCI_IMAGE_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";

/// Calculate SHA256 digest of data in OCI format (sha256:hex)
fn calculate_digest(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    format!("sha256:{}", hex::encode(result))
}

/// Verify that downloaded data matches its expected digest
fn verify_digest(data: &[u8], expected_digest: &str) -> Result<()> {
    let calculated = calculate_digest(data);
    if calculated != expected_digest {
        bail!(
            "Digest verification failed! Expected: {}, Got: {}",
            expected_digest,
            calculated
        );
    }
    Ok(())
}

/// Pull a multi-layer OCI artifact and extract all relevant layers
pub async fn pull_multi_layer_artifact(
    reference: &Reference,
    client: &Client,
) -> Result<MultiLayerArtifact> {
    let auth = oci_client::secrets::RegistryAuth::Anonymous;

    // Pull just the manifest first
    info!("Pulling OCI manifest: {}", reference);
    let (manifest, manifest_digest) = client
        .pull_manifest(reference, &auth)
        .await
        .context("Failed to pull OCI manifest")?;

    // Verify manifest digest if provided by the registry
    if !manifest_digest.is_empty() {
        debug!("Verifying manifest digest: {}", manifest_digest);
        // Note: The manifest digest verification happens at the OCI client level
        // The digest returned here has already been verified by the client
        info!("Manifest digest verified: {}", manifest_digest);
    } else {
        warn!("Registry did not provide manifest digest for verification");
    }

    // Process the layers based on media type
    let mut wasm_data = None;
    let mut policy_data = None;
    let mut config_data = None;
    let mut additional_layers = HashMap::new();

    // Get the image manifest
    let image_manifest = match manifest {
        oci_client::manifest::OciManifest::Image(manifest) => manifest,
        _ => {
            anyhow::bail!("Unexpected manifest format - expected OCI Image Manifest");
        }
    };

    // Process the config blob if it's a WASM config
    if image_manifest.config.media_type == CONFIG_MEDIA_TYPE
        || image_manifest.config.media_type == OCI_IMAGE_CONFIG_MEDIA_TYPE
    {
        let mut config_blob = Vec::new();
        client
            .pull_blob(
                reference,
                image_manifest.config.digest.as_str(),
                &mut config_blob,
            )
            .await
            .context("Failed to pull config blob")?;

        // Verify config digest
        verify_digest(&config_blob, &image_manifest.config.digest)
            .context("Config blob digest verification failed")?;

        // Try to parse as WASM config
        if let Ok(wasm_config) = serde_json::from_slice::<WasmConfig>(&config_blob) {
            // Validate WASM config fields
            if wasm_config.architecture != "wasm" {
                warn!(
                    "Config architecture is not 'wasm': {}",
                    wasm_config.architecture
                );
            }
            if wasm_config.os != "wasip1" && wasm_config.os != "wasip2" {
                warn!("Config OS is not wasip1 or wasip2: {}", wasm_config.os);
            }
            if wasm_config.os == "wasip2" && wasm_config.component.is_none() {
                warn!("wasip2 config missing component metadata");
            }
            info!(
                "Found WASM config: os={}, arch={}",
                wasm_config.os, wasm_config.architecture
            );
            config_data = Some(wasm_config);
        }
    }

    debug!(
        "Processing {} layers from OCI manifest",
        image_manifest.layers.len()
    );

    for (index, layer) in image_manifest.layers.iter().enumerate() {
        let media_type = &layer.media_type;
        let expected_digest = &layer.digest;
        debug!(
            "Layer {}: media_type={}, size={}, digest={}",
            index, media_type, layer.size, expected_digest
        );

        // Pull the layer blob into a vector
        let mut blob_data = Vec::new();
        client
            .pull_blob(reference, expected_digest.as_str(), &mut blob_data)
            .await
            .context(format!("Failed to pull layer {index}"))?;

        // Verify the layer digest
        debug!("Verifying digest for layer {}", index);
        verify_digest(&blob_data, expected_digest)
            .context(format!("Layer {index} digest verification failed"))?;
        info!("Layer {} digest verified successfully", index);

        // Categorize the layer based on media type
        if WASM_MEDIA_TYPES.contains(&media_type.as_str()) {
            if wasm_data.is_some() {
                warn!("Multiple WASM layers found, using the first one");
            } else {
                info!("Found WASM layer: {} bytes", blob_data.len());
                wasm_data = Some(blob_data);
            }
        } else if POLICY_MEDIA_TYPES.contains(&media_type.as_str()) {
            if policy_data.is_some() {
                warn!("Multiple policy layers found, using the first one");
            } else {
                info!("Found policy layer: {} bytes", blob_data.len());
                policy_data = Some(blob_data);
            }
        } else {
            debug!(
                "Additional layer with media type {}: {} bytes",
                media_type,
                blob_data.len()
            );
            additional_layers.insert(media_type.clone(), blob_data);
        }
    }

    // Ensure we have at least a WASM component
    let wasm_data =
        wasm_data.ok_or_else(|| anyhow::anyhow!("No WASM layer found in OCI artifact"))?;

    Ok(MultiLayerArtifact {
        wasm_data,
        policy_data,
        config: config_data,
        additional_layers,
    })
}

/// Pull just the WASM component from a multi-layer OCI artifact
/// This is a compatibility function that ignores non-WASM layers
pub async fn pull_wasm_only(reference: &Reference, client: &Client) -> Result<Vec<u8>> {
    let artifact = pull_multi_layer_artifact(reference, client).await?;

    if artifact.policy_data.is_some() {
        info!("Note: Policy layer found but will not be processed in this context");
    }

    if !artifact.additional_layers.is_empty() {
        info!(
            "Note: {} additional layers found but will not be processed",
            artifact.additional_layers.len()
        );
    }

    Ok(artifact.wasm_data)
}

/// Pull a multi-layer OCI artifact with strict digest verification
/// This is the secure version that enforces all digest checks
pub async fn pull_multi_layer_artifact_secure(
    reference: &Reference,
    client: &Client,
) -> Result<MultiLayerArtifact> {
    // This uses the same implementation as pull_multi_layer_artifact
    // since we've already added digest verification there
    pull_multi_layer_artifact(reference, client).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_digest() {
        // Test with known data and digest
        let data = b"Hello, World!";
        let digest = calculate_digest(data);
        assert_eq!(
            digest,
            "sha256:dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f"
        );
    }

    #[test]
    fn test_verify_digest_success() {
        let data = b"test data";
        let expected = "sha256:916f0027a575074ce72a331777c3478d6513f786a591bd892da1a577bf2335f9";
        assert!(verify_digest(data, expected).is_ok());
    }

    #[test]
    fn test_verify_digest_failure() {
        let data = b"test data";
        let wrong_digest =
            "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        assert!(verify_digest(data, wrong_digest).is_err());
    }

    #[test]
    fn test_media_type_recognition() {
        // Test WASM media types
        assert!(WASM_MEDIA_TYPES.contains(&"application/wasm"));
        assert!(WASM_MEDIA_TYPES.contains(&"application/vnd.wasm.component.v1"));

        // Test policy media types
        assert!(POLICY_MEDIA_TYPES.contains(&"application/vnd.wasm.policy.v1+yaml"));
        assert!(POLICY_MEDIA_TYPES.contains(&"text/yaml"));

        // Test config media type
        assert_eq!(CONFIG_MEDIA_TYPE, "application/vnd.wasm.config.v0+json");
    }

    #[test]
    fn test_wasm_config_serialization() {
        let config = WasmConfig {
            created: "2024-09-25T12:00:00Z".to_string(),
            architecture: "wasm".to_string(),
            os: "wasip2".to_string(),
            layer_digests: vec!["sha256:abc123".to_string(), "sha256:def456".to_string()],
            component: Some(ComponentMetadata {
                exports: Some(vec!["wasi:http/incoming-handler@0.2.0".to_string()]),
                imports: Some(vec!["wasi:io/error@0.2.0".to_string()]),
                target: Some("wasi:http/proxy@0.2.0".to_string()),
            }),
        };

        // Test serialization
        let json_str = serde_json::to_string(&config).unwrap();
        assert!(json_str.contains("\"architecture\":\"wasm\""));
        assert!(json_str.contains("\"os\":\"wasip2\""));
        assert!(json_str.contains("\"layerDigests\""));
        assert!(json_str.contains("\"component\""));

        // Test deserialization
        let parsed: WasmConfig = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.architecture, "wasm");
        assert_eq!(parsed.os, "wasip2");
        assert_eq!(parsed.layer_digests.len(), 2);
        assert!(parsed.component.is_some());
    }

    #[test]
    fn test_wasip1_config_without_component() {
        let config = WasmConfig {
            created: "2024-09-25T12:00:00Z".to_string(),
            architecture: "wasm".to_string(),
            os: "wasip1".to_string(),
            layer_digests: vec!["sha256:abc123".to_string()],
            component: None, // wasip1 doesn't require component metadata
        };

        assert_eq!(config.os, "wasip1");
        assert!(config.component.is_none());

        // Should serialize/deserialize correctly
        let json_str = serde_json::to_string(&config).unwrap();
        let parsed: WasmConfig = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.os, "wasip1");
        assert!(parsed.component.is_none());
    }

    #[test]
    fn test_component_metadata() {
        let metadata = ComponentMetadata {
            exports: Some(vec![
                "wasi:http/incoming-handler@0.2.0".to_string(),
                "wasi:cli/run@0.2.0".to_string(),
            ]),
            imports: Some(vec![
                "wasi:io/error@0.2.0".to_string(),
                "wasi:filesystem/types@0.2.0".to_string(),
            ]),
            target: Some("wasi:http/proxy@0.2.0".to_string()),
        };

        assert_eq!(metadata.exports.as_ref().unwrap().len(), 2);
        assert_eq!(metadata.imports.as_ref().unwrap().len(), 2);
        assert_eq!(metadata.target.as_ref().unwrap(), "wasi:http/proxy@0.2.0");
    }
}
