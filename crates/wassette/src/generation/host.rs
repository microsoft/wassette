// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use super::*;
use crate::{WasiState, WassetteWasiState};

mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/component-generation",
        world: "imports",
        imports: { default: async },
    });
}

use bindings::wassette::component_generation::builder::{
    self, Disposition, GenerationError as WitError, GenerationReport,
};

const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn add_to_linker(
    linker: &mut wasmtime::component::Linker<WassetteWasiState<WasiState>>,
) -> Result<()> {
    builder::add_to_linker::<_, wasmtime::component::HasSelf<_>>(linker, |state| state)?;
    Ok(())
}

impl builder::Host for WassetteWasiState<WasiState> {
    async fn generate(
        &mut self,
        request_json: String,
    ) -> std::result::Result<GenerationReport, WitError> {
        let caller = self
            .generation_caller
            .clone()
            .ok_or(WitError::SessionNotBound)?;
        let service = caller.manager.generation_service().map_err(map_error)?;
        if request_json.len() > MAX_REQUEST_BYTES {
            return Err(WitError::InvalidRequest(
                "generation request exceeds 4 MiB".into(),
            ));
        }
        let request: GenerationRequest = serde_json::from_str(&request_json)
            .map_err(|_| WitError::InvalidRequest("invalid generation request JSON".into()))?;
        let target = request.target.clone();
        let name = request.build.component_name.clone();
        let permissions = service
            .permissions_for_caller(&caller, &name, &target)
            .await
            .map_err(map_error)?;
        require(permissions.can_build(), "build").map_err(map_error)?;
        require(permissions.can_install(), "install").map_err(map_error)?;
        let cancel = CancellationToken::new();
        let _cancel_on_drop = cancel.clone().drop_guard();
        let prepared = service
            .prepare(&caller.manager, request, permissions, cancel.clone())
            .await
            .map_err(map_error)?;
        let permissions = service
            .permissions_for_caller(&caller, &name, &target)
            .await
            .map_err(map_error)?;
        let outcome = prepared
            .install(permissions, cancel)
            .await
            .map_err(map_error)?;
        let disposition = if outcome.preview.kind == ComponentKind::AcpLayer {
            Disposition::LaterSelectionRequired
        } else if outcome.commit.entry.binding().kind == StoredArtifactKind::Tool {
            Disposition::ToolsEligible
        } else {
            Disposition::Installed
        };
        Ok(GenerationReport {
            report_json: outcome.report().to_string(),
            disposition,
            tool_handles: Vec::new(),
        })
    }
}

fn map_error(error: anyhow::Error) -> WitError {
    if let Some(error) = error.downcast_ref::<GenerationError>() {
        return match error {
            GenerationError::Disabled => WitError::Disabled,
            GenerationError::PermissionDenied(_) => WitError::PermissionDenied,
            GenerationError::Cancelled => WitError::Cancelled,
            GenerationError::CommitRecoveryRequired { .. } => WitError::RecoveryRequired(
                error
                    .recovery_report()
                    .expect("recovery variant")
                    .to_string(),
            ),
            GenerationError::CommittedButRefreshFailed { .. } => {
                WitError::Committed(GenerationReport {
                    report_json: error
                        .committed_report()
                        .expect("committed variant")
                        .to_string(),
                    disposition: Disposition::CommittedNotExposed,
                    tool_handles: Vec::new(),
                })
            }
        };
    }
    if matches!(
        error.downcast_ref::<StoreError>(),
        Some(StoreError::Conflict(_))
    ) {
        return WitError::Stale("generation target changed; obtain its current revision".into());
    }
    if let Some(failure) = BuildError::from_error(&error) {
        let diagnostic = failure
            .diagnostic()
            .map(bounded_diagnostic)
            .unwrap_or_else(|| failure.to_string());
        return match failure.kind() {
            BuildErrorKind::Cancelled => WitError::Cancelled,
            BuildErrorKind::Busy => WitError::Busy,
            BuildErrorKind::InvalidRequest | BuildErrorKind::InvalidWit => {
                WitError::InvalidRequest(diagnostic)
            }
            BuildErrorKind::DeadlineExceeded
            | BuildErrorKind::CompilationFailed
            | BuildErrorKind::InvalidOutput => WitError::BuildFailed(diagnostic),
            BuildErrorKind::Unavailable | BuildErrorKind::Internal => {
                WitError::Unavailable(diagnostic)
            }
            _ => WitError::Unavailable("unsupported builder failure category".into()),
        };
    }
    WitError::BuildFailed("generation or validation failed".into())
}

fn bounded_diagnostic(text: &str) -> String {
    const BUDGET: usize = 16 * 1024;
    const SUFFIX: &str = "\n[diagnostic truncated]";
    let mut encoded = 2 + SUFFIX.len() + 1;
    for (offset, character) in text.char_indices() {
        encoded += match character {
            '"' | '\\' | '\n' | '\r' | '\t' => 2,
            character if character.is_control() => 6,
            character => character.len_utf8(),
        };
        if encoded > BUDGET {
            return format!("{}{SUFFIX}", &text[..offset]);
        }
    }
    text.to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[tokio::test]
    async fn an_unbound_ordinary_store_cannot_request_generation() {
        let state = crate::WasiStateTemplate::default().build().unwrap();
        let mut state = WassetteWasiState::new(state, HashSet::new()).unwrap();
        let result = builder::Host::generate(&mut state, "{}".into()).await;
        assert!(matches!(result, Err(WitError::SessionNotBound)));
    }

    #[test]
    fn permission_errors_do_not_disclose_request_or_host_data() {
        assert!(matches!(
            map_error(GenerationError::PermissionDenied("install").into()),
            WitError::PermissionDenied,
        ));
        let error = map_error(anyhow!("untrusted source contents and private host path"));
        let WitError::BuildFailed(message) = error else {
            panic!("expected build failure")
        };
        assert!(!message.contains("untrusted"));
        assert!(!message.contains("private host"));
    }

    #[test]
    fn authorized_diagnostics_have_a_serialized_bound() {
        for text in [
            "\0".repeat(100_000),
            "\"\\\n".repeat(100_000),
            "\u{1f600}".repeat(100_000),
        ] {
            let bounded = bounded_diagnostic(&text);
            assert!(serde_json::to_vec(&bounded).unwrap().len() <= 16 * 1024);
            assert!(bounded.ends_with("[diagnostic truncated]"));
        }
    }
}
