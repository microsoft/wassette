// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Semantic component names and their separate, portable storage keys.

use thiserror::Error;

/// A source-derived logical component name, independent of producer metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ComponentId(String);

impl ComponentId {
    /// Validate a source/request-derived name, preserving its exact spelling.
    ///
    /// Returns an error for blank names or names containing control characters.
    pub fn from_name(name: &str) -> Result<Self, IdentityError> {
        validate_name(name)?;
        Ok(Self(name.to_owned()))
    }

    /// Derive a local identity from the visible filename, not its contents.
    pub fn from_local_path(path: &std::path::Path) -> anyhow::Result<Self> {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("component source has no UTF-8 filename"))?;
        let stem = filename.strip_suffix(".wasm").unwrap_or(filename);
        validate_name(stem)?;
        Ok(Self::from_name(&format!("local:{stem}"))?)
    }

    /// Return the logical name without filename sanitization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) fn validate_name(name: &str) -> Result<(), IdentityError> {
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err(IdentityError::InvalidName);
    }
    Ok(())
}

/// Name validation failures and cosmetic producer-name diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdentityError {
    /// No explicit name was declared in the root component-name metadata.
    #[error("missing cosmetic root component name")]
    Missing,
    /// More than one root name was declared, whether or not the names agree.
    #[error("ambiguous cosmetic root component name")]
    Ambiguous,
    /// The declared name is blank or contains control characters.
    #[error("component name must be nonblank and contain no control characters")]
    InvalidName,
}

/// A portable private filename stem, not a semantic component identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorageKey(String);

impl StorageKey {
    /// Validate a portable filename stem and its existing secret-file projection.
    ///
    /// Accepted spelling is preserved, and no arbitrary length limit is imposed.
    /// Filesystem path-size limits are still enforced by subsequent I/O.
    ///
    /// # Errors
    ///
    /// Returns an error if either stem has nonportable syntax, a trailing dot,
    /// or a reserved Windows device basename.
    pub fn parse(raw: &str) -> Result<Self, StorageKeyError> {
        validate_storage_stem(raw)?;
        let stem = legacy_secret_stem(raw);
        validate_storage_stem(&stem).map_err(|source| StorageKeyError::UnsafeSecretProjection {
            stem,
            source: Box::new(source),
        })?;
        Ok(Self(raw.to_owned()))
    }

    /// Return the validated storage key with its original spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Alias domains to reserve separately when assigning a storage key.
    ///
    /// Computing these keys does not establish uniqueness or permit replacement.
    pub fn collision_keys(&self) -> StorageCollisionKeys {
        StorageCollisionKeys {
            artifact: self.0.to_ascii_lowercase(),
            legacy_secrets: legacy_secret_stem(&self.0).to_ascii_lowercase(),
        }
    }
}

/// Separate alias domains for artifact filenames and legacy secret filenames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageCollisionKeys {
    /// ASCII-casefolded artifact filename stem.
    pub artifact: String,
    /// ASCII-casefolded legacy secret stem, including its 128-byte truncation.
    pub legacy_secrets: String,
}

/// A storage key or its legacy secret projection is not a portable filename stem.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StorageKeyError {
    /// The stem is empty, is a dot entry, or contains characters outside the grammar.
    #[error("storage key must be nonempty ASCII [A-Za-z0-9._-] and must not be '.' or '..'")]
    InvalidSyntax,
    /// The stem ends with a dot.
    #[error("storage key must not end with a dot")]
    TrailingDot,
    /// The basename before any extension is a reserved Windows device name.
    #[error("storage key must not use a reserved Windows device basename, including extensions")]
    ReservedDeviceName,
    /// The otherwise valid key produces an unsafe legacy secret filename stem.
    #[error("storage key projects to unsafe legacy secret stem {stem:?}: {source}")]
    UnsafeSecretProjection {
        /// The unchanged legacy projection that failed validation.
        stem: String,
        /// The portability rule violated by the projected stem.
        #[source]
        source: Box<StorageKeyError>,
    },
}

fn validate_storage_stem(stem: &str) -> Result<(), StorageKeyError> {
    if stem.is_empty()
        || matches!(stem, "." | "..")
        || !stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(StorageKeyError::InvalidSyntax);
    }
    if stem.ends_with('.') {
        return Err(StorageKeyError::TrailingDot);
    }
    let basename = stem.split('.').next().unwrap_or_default();
    let upper = basename.to_ascii_uppercase();
    let numbered_device = (upper.starts_with("COM") || upper.starts_with("LPT"))
        && upper.len() == 4
        && matches!(upper.as_bytes()[3], b'1'..=b'9');
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered_device {
        return Err(StorageKeyError::ReservedDeviceName);
    }
    Ok(())
}

/// Preserve the existing secret-file projection, including its 128-byte aliases.
pub(crate) fn legacy_secret_stem(component_id: &str) -> String {
    let mut result = String::new();
    let mut last_was_underscore = false;

    for ch in component_id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
            result.push(ch);
            last_was_underscore = false;
        } else if !last_was_underscore {
            result.push('_');
            last_was_underscore = true;
        }
    }

    while result.starts_with('_') {
        result.remove(0);
    }
    while result.ends_with('_') {
        result.pop();
    }
    // Every output character is ASCII; truncation cannot split a UTF-8 sequence.
    result.truncate(128);
    if result.is_empty() {
        result = "unnamed".to_string();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_names_preserve_exact_spelling() {
        for name in [
            "example:Weather/東京@1.0.0",
            "é",
            "e\u{301}",
            " Weather ",
            "root:component",
            "NUL",
            "../not-a-storage-key",
        ] {
            assert_eq!(ComponentId::from_name(name).unwrap().as_str(), name);
        }
        assert_ne!(
            ComponentId::from_name("Weather"),
            ComponentId::from_name("weather")
        );
        assert_ne!(
            ComponentId::from_name("é"),
            ComponentId::from_name("e\u{301}")
        );
        let long = "名".repeat(200);
        assert_eq!(ComponentId::from_name(&long).unwrap().as_str(), long);
    }

    #[test]
    fn local_identity_preserves_filename_and_strips_only_wasm_suffix() {
        for (filename, expected) in [
            ("weather.wasm", "local:weather"),
            ("weather.v2.wasm", "local:weather.v2"),
            ("weather.wasm.wasm", "local:weather.wasm"),
            ("météo.wasm", "local:météo"),
        ] {
            assert_eq!(
                ComponentId::from_local_path(std::path::Path::new(filename))
                    .unwrap()
                    .as_str(),
                expected,
            );
        }
        assert!(ComponentId::from_local_path(std::path::Path::new(".wasm")).is_err());
    }

    #[test]
    fn invalid_component_names_are_typed_errors() {
        for name in [
            "", " ", "\u{2003}", "\t", "a\0b", "a\nb", "a\u{7f}b", "a\u{85}b",
        ] {
            assert_eq!(
                ComponentId::from_name(name),
                Err(IdentityError::InvalidName),
                "{name:?}"
            );
        }
    }

    #[test]
    fn storage_keys_preserve_portable_spelling_without_a_length_limit() {
        for key in [
            "Weather_1.0-rc",
            ".hidden",
            "_",
            "a..b",
            "COM0",
            "com10",
            "LPT0",
            "lpt10",
            "CONSOLE",
        ] {
            assert_eq!(StorageKey::parse(key).unwrap().as_str(), key);
        }
        let long = "a".repeat(1024);
        assert_eq!(StorageKey::parse(&long).unwrap().as_str(), long);
    }

    #[test]
    fn storage_keys_reject_nonportable_syntax() {
        for key in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "C:foo",
            "file:foo",
            "https://foo",
            "a b",
            "a\tb",
            "東京",
            "café",
            "a\0b",
            "a\u{7f}b",
        ] {
            assert_eq!(
                StorageKey::parse(key),
                Err(StorageKeyError::InvalidSyntax),
                "{key:?}"
            );
        }
        for key in ["a.", "...", "NUL."] {
            assert_eq!(StorageKey::parse(key), Err(StorageKeyError::TrailingDot));
        }
    }

    #[test]
    fn storage_keys_reject_device_basenames_and_extensions() {
        let mut devices = vec![
            "CON".to_string(),
            "PRN".to_string(),
            "AUX".to_string(),
            "NUL".to_string(),
        ];
        for index in 1..=9 {
            devices.push(format!("COM{index}"));
            devices.push(format!("LPT{index}"));
        }
        for device in devices {
            for key in [
                device.clone(),
                device.to_lowercase(),
                format!("{device}.extension"),
            ] {
                assert_eq!(
                    StorageKey::parse(&key),
                    Err(StorageKeyError::ReservedDeviceName),
                    "{key:?}"
                );
            }
        }
    }

    #[test]
    fn storage_keys_validate_the_projected_secret_stem() {
        for (key, stem, reason) in [
            ("NUL_", "NUL", StorageKeyError::ReservedDeviceName),
            ("_con.txt_", "con.txt", StorageKeyError::ReservedDeviceName),
            ("_LPT9_", "LPT9", StorageKeyError::ReservedDeviceName),
            ("_._", ".", StorageKeyError::InvalidSyntax),
            ("_.._", "..", StorageKeyError::InvalidSyntax),
            ("a._", "a.", StorageKeyError::TrailingDot),
        ] {
            assert_eq!(
                StorageKey::parse(key),
                Err(StorageKeyError::UnsafeSecretProjection {
                    stem: stem.to_string(),
                    source: Box::new(reason),
                })
            );
        }
        let key = format!("{}.suffix", "a".repeat(127));
        assert!(matches!(
            StorageKey::parse(&key),
            Err(StorageKeyError::UnsafeSecretProjection { source, .. })
                if *source == StorageKeyError::TrailingDot
        ));
    }

    #[test]
    fn collision_keys_cover_both_alias_domains() {
        let keys = |key: &str| StorageKey::parse(key).unwrap().collision_keys();
        assert_eq!(keys("Weather"), keys("weather"));
        for (left, right) in [("a__b", "a_b"), ("_a", "a"), ("_", "unnamed")] {
            assert_ne!(keys(left).artifact, keys(right).artifact);
            assert_eq!(keys(left).legacy_secrets, keys(right).legacy_secrets);
        }
        let prefix = "a".repeat(128);
        let left = keys(&format!("{prefix}x"));
        let right = keys(&format!("{prefix}y"));
        assert_ne!(left.artifact, right.artifact);
        assert_eq!(left.legacy_secrets, right.legacy_secrets);
        assert_eq!(left.legacy_secrets, prefix);
    }

    #[test]
    fn legacy_projection_is_unchanged() {
        for (input, expected) in [
            ("simple", "simple"),
            ("with-dashes.and.dots", "with-dashes.and.dots"),
            ("with_underscores", "with_underscores"),
            ("a___b", "a_b"),
            ("__a__", "a"),
            ("a:/東京\\ b", "a_b"),
            ("", "unnamed"),
            ("___", "unnamed"),
            ("東京", "unnamed"),
            ("NUL_", "NUL"),
            ("_._", "."),
        ] {
            assert_eq!(legacy_secret_stem(input), expected, "{input:?}");
        }
        assert_eq!(legacy_secret_stem(&"a".repeat(140)), "a".repeat(128));
        assert_eq!(
            legacy_secret_stem(&format!("{}__z", "a".repeat(127))),
            format!("{}_", "a".repeat(127))
        );
    }
}
