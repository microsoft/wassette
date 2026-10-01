// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable classification of pre-install builder failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BuildErrorKind {
    Cancelled,
    Busy,
    DeadlineExceeded,
    InvalidRequest,
    InvalidWit,
    CompilationFailed,
    InvalidOutput,
    Unavailable,
    Internal,
}

/// A builder failure with log-safe formatting and separately disclosed details.
///
/// `Builder::build` retains its `anyhow::Result` signature. Adapters can use
/// `error.downcast_ref::<BuildError>()`, including through anyhow context, then
/// inspect [`Self::kind`] and return [`Self::diagnostic`] only to the authorized
/// requester. Do not log the diagnostic accessor. `Display` and `Debug` omit
/// diagnostic bodies and there is no underlying host/configuration error chain.
#[derive(Clone, PartialEq, Eq)]
pub struct BuildError {
    kind: BuildErrorKind,
    diagnostic: Option<String>,
    diagnostic_truncated: bool,
}

impl BuildError {
    /// Find a builder failure through anyhow context or another error's sources.
    pub fn from_error(error: &anyhow::Error) -> Option<&Self> {
        error.downcast_ref::<Self>().or_else(|| {
            error
                .chain()
                .take(32)
                .find_map(|cause| cause.downcast_ref::<Self>())
        })
    }

    pub fn kind(&self) -> BuildErrorKind {
        self.kind
    }

    /// Bounded, sanitized failure details for the authorized requester.
    /// Unavailable/internal failures never expose host or configuration details.
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }

    pub fn diagnostic_truncated(&self) -> bool {
        self.diagnostic_truncated
    }

    pub(crate) fn new(kind: BuildErrorKind) -> Self {
        Self {
            kind,
            diagnostic: None,
            diagnostic_truncated: false,
        }
    }

    pub(crate) fn with_diagnostic(kind: BuildErrorKind, text: &str, cap: usize) -> Self {
        if !matches!(
            kind,
            BuildErrorKind::InvalidRequest
                | BuildErrorKind::InvalidWit
                | BuildErrorKind::CompilationFailed
                | BuildErrorKind::InvalidOutput
        ) {
            return Self::new(kind);
        }
        let (diagnostic, diagnostic_truncated) = sanitize(text, cap.min(256 * 1024));
        Self {
            kind,
            diagnostic: (!diagnostic.is_empty()).then_some(diagnostic),
            diagnostic_truncated,
        }
    }

    pub(crate) fn preserve_or_redact(
        error: anyhow::Error,
        fallback: BuildErrorKind,
    ) -> anyhow::Error {
        if let Some(typed) = Self::from_error(&error) {
            typed.clone().into()
        } else {
            Self::new(fallback).into()
        }
    }

    pub(crate) fn explain(error: &anyhow::Error, kind: BuildErrorKind, cap: usize) -> Self {
        if let Some(typed) = Self::from_error(error) {
            return typed.clone();
        }
        use std::fmt::Write;
        let mut message = LimitedText {
            text: String::new(),
            cap: cap.min(256 * 1024),
            truncated: false,
        };
        for (index, cause) in error.chain().take(16).enumerate() {
            if index > 0 && message.write_str("\n").is_err() {
                break;
            }
            if write!(message, "{cause}").is_err() {
                break;
            }
        }
        let mut error = Self::with_diagnostic(kind, &message.text, cap);
        error.diagnostic_truncated |= message.truncated;
        error
    }

    pub(crate) fn set_truncated(&mut self, truncated: bool) {
        self.diagnostic_truncated |= truncated;
    }
}

struct LimitedText {
    text: String,
    cap: usize,
    truncated: bool,
}

impl fmt::Write for LimitedText {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut n = text.len().min(self.cap.saturating_sub(self.text.len()));
        while !text.is_char_boundary(n) {
            n -= 1;
        }
        self.text.push_str(&text[..n]);
        if n < text.len() {
            self.truncated = true;
            return Err(fmt::Error);
        }
        Ok(())
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind {
            BuildErrorKind::Cancelled => "build cancelled",
            BuildErrorKind::Busy => "builder busy; retry after an active build completes",
            BuildErrorKind::DeadlineExceeded => "build deadline exceeded",
            BuildErrorKind::InvalidRequest => "invalid build request",
            BuildErrorKind::InvalidWit => "invalid WIT",
            BuildErrorKind::CompilationFailed => "component compilation failed",
            BuildErrorKind::InvalidOutput => "invalid generated component",
            BuildErrorKind::Unavailable => {
                "builder unavailable (helper, profile, or hypervisor configuration)"
            }
            BuildErrorKind::Internal => "builder failed",
        })
    }
}

impl fmt::Debug for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BuildError")
            .field("kind", &self.kind)
            .field("diagnostic_present", &self.diagnostic.is_some())
            .field("diagnostic_truncated", &self.diagnostic_truncated)
            .finish()
    }
}

impl std::error::Error for BuildError {}

/// Redact path-bearing tokens, quoted paths, control characters and source
/// gutters. Callers select compiler/WIT messages before applying this filter.
pub(crate) fn sanitize(text: &str, cap: usize) -> (String, bool) {
    let mut output = String::new();
    let mut truncated = false;
    'lines: for line in text.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('|')
            || line.starts_with('^')
            || line
                .split_once('|')
                .is_some_and(|(n, _)| n.trim().bytes().all(|c| c.is_ascii_digit()))
        {
            continue;
        }
        if !output.is_empty() {
            if output.len() == cap {
                truncated = true;
                break;
            }
            output.push('\n');
        }
        let line = line.strip_prefix("--> ").unwrap_or(line);
        let normalized;
        let line = if let Some((label, remainder)) = [
            ("builder-input/component.rs:", "source:"),
            ("/input/component.rs:", "source:"),
            ("builder-input/bindings.rs:", "bindings:"),
            ("/input/bindings.rs:", "bindings:"),
            ("request.wit:", "WIT:"),
        ]
        .iter()
        .find_map(|(prefix, label)| line.strip_prefix(prefix).map(|rest| (*label, rest)))
        {
            normalized = format!("{label}{remainder}");
            &normalized
        } else {
            line
        };
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            let mut token = String::new();
            token.push(ch);
            if matches!(ch, '\'' | '"' | '`') {
                for next in chars.by_ref() {
                    token.push(next);
                    if next == ch {
                        break;
                    }
                }
            } else if !ch.is_whitespace() {
                while chars
                    .peek()
                    .is_some_and(|c| !c.is_whitespace() && !matches!(c, '\'' | '"' | '`'))
                {
                    token.push(chars.next().expect("peeked diagnostic character"));
                }
            }
            let token = if token.contains(['/', '\\'])
                && token.chars().any(char::is_alphanumeric)
                && !wit_identifier(&token)
            {
                "[path]"
            } else {
                &token
            };
            for ch in token.chars().filter(|c| !c.is_control()
                && !matches!(*c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
            {
                if ch.len_utf8() > cap.saturating_sub(output.len()) {
                    truncated = true;
                    break 'lines;
                }
                output.push(ch);
            }
        }
    }
    (output, truncated)
}

fn wit_identifier(token: &str) -> bool {
    let token = token.trim_matches(['\'', '"', '`', ',', ';']);
    let token = token
        .strip_prefix("[async-lift]")
        .or_else(|| token.strip_prefix("[async-lower]"))
        .unwrap_or(token);
    let (token, function) = token
        .split_once('#')
        .map_or((token, None), |(name, function)| (name, Some(function)));
    if function.is_some_and(|f| {
        f.is_empty()
            || !f
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._[]".contains(&b))
    }) {
        return false;
    }
    let Some((namespace, member)) = token.split_once(':') else {
        return false;
    };
    let Some((package, interface)) = member.split_once('/') else {
        return false;
    };
    let ident = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    ident(namespace)
        && ident(package)
        && !interface.is_empty()
        && interface
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-@.".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_accessor_discloses_diagnostic() {
        let error = BuildError::with_diagnostic(
            BuildErrorKind::CompilationFailed,
            "source:3:7: error[E0308]: PRIVATE_DIAGNOSTIC expected u32, found string",
            1024,
        );
        assert!(error.diagnostic().unwrap().contains("PRIVATE_DIAGNOSTIC"));
        for log in [
            format!("{error}"),
            format!("{error:?}"),
            format!("{error:#?}"),
        ] {
            assert!(!log.contains("PRIVATE_DIAGNOSTIC"));
        }
        let error = anyhow::Error::new(error).context("generation failed");
        assert!(!format!("{error:#}").contains("PRIVATE_DIAGNOSTIC"));
        assert_eq!(
            error.downcast_ref::<BuildError>().unwrap().kind(),
            BuildErrorKind::CompilationFailed
        );
    }

    #[test]
    fn details_are_bounded_redacted_and_never_configuration_errors() {
        let error = BuildError::with_diagnostic(
            BuildErrorKind::InvalidWit,
            "expected semicolon\n --> /Users/private/request.wit:2:3\n 2 | RAW_SOURCE_BODY\n ^\nfailed at `C:\\private path\\token`",
            80,
        );
        let text = error.diagnostic().unwrap();
        assert!(text.len() <= 80);
        for secret in ["Users", "private", "token", "RAW_SOURCE_BODY"] {
            assert!(!text.contains(secret));
        }
        assert!(
            BuildError::with_diagnostic(BuildErrorKind::Unavailable, "/private/config", 100)
                .diagnostic()
                .is_none()
        );
        let clipped = BuildError::with_diagnostic(BuildErrorKind::CompilationFailed, "abcdef", 3);
        assert_eq!(clipped.diagnostic(), Some("abc"));
        assert!(clipped.diagnostic_truncated());
        let exact = BuildError::with_diagnostic(BuildErrorKind::CompilationFailed, "abc", 3);
        assert!(!exact.diagnostic_truncated());
    }

    #[test]
    fn compiler_locations_and_wit_symbols_survive_redaction() {
        let (text, _) = sanitize(
            "builder-input/component.rs:4:7: error[E0308]: expected u32, found string\nundefined symbol `wassette:acp/agent@7.0.0#initialize`\nfailed at \"/Users/private path/config\"",
            1024,
        );
        assert!(text.contains("source:4:7: error[E0308]"));
        assert!(text.contains("wassette:acp/agent@7.0.0#initialize"));
        assert!(!text.contains("private path"));
        assert!(!text.contains("builder-input"));
    }
}
